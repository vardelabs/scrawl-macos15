//! A program of the user's choice as the transcriber (`"transcriber": "command"`). Scrawl runs
//! `transcribeCommand` on the recording's 16 kHz mono WAV and reads one JSON object from its
//! stdout: `version` (1), `text`, and optional `model`, `segments`, `words` and `estimated`, with
//! times in integer milliseconds from the start of the WAV, the same shape as `scrawl.json`.
//!
//! The program's stderr is discarded and its stdout is never logged, so nothing the user said
//! reaches the log. A failure surfaces as a short reason (`timeout`, `words[12] is past the end
//! of the audio`) and the caller falls back to Apple Speech for that recording.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::time::{Duration, Instant};

use anyhow::{Result, bail, ensure};
use serde::Deserialize;

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

pub const AUDIO_PLACEHOLDER: &str = "{audio}";
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const MIN_TIMEOUT: Duration = Duration::from_secs(300);
const EXIT_POLL: Duration = Duration::from_millis(50);
/// A configured timeout past this is a mistake, and a huge one would overflow the deadline.
const MAX_TIMEOUT_SECONDS: u64 = 24 * 60 * 60;
/// A span may end this far past the audio before its whole array is distrusted.
const END_SLACK_MS: u64 = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub command: Vec<String>,
    pub timeout: Option<Duration>,
}

/// The usable form of the config keys: a non-empty argv whose first entry is an absolute path
/// (the app runs without the user's shell `PATH`), and a timeout only when it is positive,
/// capped at a day.
pub fn settings(command: Option<&[String]>, timeout_seconds: Option<u64>) -> Option<Settings> {
    let command = command?.to_vec();
    if !command.first().is_some_and(|program| program.starts_with('/')) {
        return None;
    }
    Some(Settings {
        command,
        timeout: timeout_seconds
            .filter(|&seconds| seconds > 0)
            .map(|seconds| Duration::from_secs(seconds.min(MAX_TIMEOUT_SECONDS))),
    })
}

/// `{audio}` inside any argument becomes the WAV path; without it, the path goes last.
pub fn argv(command: &[String], audio: &Path) -> Vec<OsString> {
    let audio = audio.as_os_str().to_string_lossy();
    let mut argv: Vec<OsString> = command
        .iter()
        .map(|arg| arg.replace(AUDIO_PLACEHOLDER, &audio).into())
        .collect();
    if !command.iter().any(|arg| arg.contains(AUDIO_PLACEHOLDER)) {
        argv.push(audio.as_ref().into());
    }
    argv
}

/// The configured timeout, else `max(300, 60 + 10 × audio seconds)` seconds: model loading
/// counts, and a slow model on a long recording must still get its turn.
pub fn timeout(configured: Option<Duration>, audio_ms: u64) -> Duration {
    configured.unwrap_or_else(|| MIN_TIMEOUT.max(Duration::from_secs(60 + audio_ms / 100)))
}

/// `segments` and `words` stay raw JSON so a malformed array costs only that array, never the
/// text beside it.
#[derive(Deserialize)]
struct Output {
    version: Option<i64>,
    model: Option<String>,
    text: Option<String>,
    segments: Option<serde_json::Value>,
    words: Option<serde_json::Value>,
    estimated: Option<bool>,
}

#[derive(Deserialize)]
struct Span {
    t0: i64,
    t1: i64,
    text: String,
}

/// Clamps `spans` to the audio, or names the first span that is negative, reversed or too far
/// past the end, which makes the whole array unusable.
fn check_spans(spans: &[Span], name: &str, audio_ms: u64) -> Result<Vec<(u64, u64, String)>, String> {
    let limit = (audio_ms + END_SLACK_MS) as i64;
    spans
        .iter()
        .enumerate()
        .map(|(index, span)| {
            if span.t0 < 0 || span.t1 < 0 {
                return Err(format!("{name}[{index}] is negative"));
            }
            if span.t1 < span.t0 {
                return Err(format!("{name}[{index}] is reversed"));
            }
            if span.t1 > limit || span.t0 > limit {
                return Err(format!("{name}[{index}] is past the end of the audio"));
            }
            let clamp = |time: i64| (time as u64).min(audio_ms);
            Ok((clamp(span.t0), clamp(span.t1), span.text.trim().to_owned()))
        })
        .collect()
}

/// An absent or `null` array is `None`; one of the wrong shape (a float time, a span without
/// text) is unusable the same way a bad timestamp is.
fn spans(
    value: Option<&serde_json::Value>,
    name: &str,
    audio_ms: u64,
) -> Option<Result<Vec<(u64, u64, String)>, String>> {
    let value = value.filter(|value| !value.is_null())?;
    let spans: Vec<Span> = match serde_json::from_value(value.clone()) {
        Ok(spans) => spans,
        Err(_) => return Some(Err(format!("{name} is malformed"))),
    };
    Some(check_spans(&spans, name, audio_ms))
}

/// Sentences cut where a word ends one, as `hex::spread_words` does for estimated text.
fn segments_from_words(words: &[WordTiming]) -> Vec<TranscriptSegment> {
    let mut segments = Vec::new();
    let mut start = 0;
    for (index, word) in words.iter().enumerate() {
        if crate::hex::ends_sentence(&word.text) || index + 1 == words.len() {
            let sentence = &words[start..=index];
            segments.push(TranscriptSegment {
                t0: sentence[0].t0,
                t1: sentence[sentence.len() - 1].t1,
                text: sentence
                    .iter()
                    .map(|word| word.text.as_str())
                    .collect::<Vec<_>>()
                    .join(" "),
            });
            start = index + 1;
        }
    }
    segments
}

/// Words spread over each segment, or over the whole recording when there are none.
fn spread(text: &str, segments: &[TranscriptSegment], audio_ms: u64) -> (Vec<TranscriptSegment>, Vec<WordTiming>) {
    if segments.is_empty() {
        return crate::hex::spread_words(text, 0, audio_ms);
    }
    let mut words = Vec::new();
    for segment in segments {
        let (_, spread) = crate::hex::spread_words(&segment.text, segment.t0, segment.t1);
        words.extend(spread);
    }
    (segments.to_vec(), words)
}

/// The program's stdout as a transcript. Bad timing never throws away the text: unusable words
/// are estimated over the segments (or the recording), unusable segments are dropped.
pub fn parse_output(stdout: &[u8], audio_ms: u64) -> Result<TranscriptData> {
    let Ok(output) = serde_json::from_slice::<Output>(stdout) else {
        bail!("invalid JSON");
    };
    ensure!(output.version == Some(1), "version must be 1");
    let Some(text) = output.text else {
        bail!("text is missing");
    };
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    let mut estimated = output.estimated.unwrap_or(true);
    let segments: Vec<TranscriptSegment> = match spans(output.segments.as_ref(), "segments", audio_ms) {
        Some(Ok(spans)) => spans
            .into_iter()
            .filter(|(_, _, text)| !text.is_empty())
            .map(|(t0, t1, text)| TranscriptSegment { t0, t1, text })
            .collect(),
        Some(Err(reason)) => {
            crate::capture::scrawl_log(format!("Command transcriber: {reason}; dropping segments"));
            Vec::new()
        }
        None => Vec::new(),
    };
    let words: Option<Vec<WordTiming>> = match spans(output.words.as_ref(), "words", audio_ms) {
        Some(Ok(spans)) => Some(
            spans
                .into_iter()
                .filter(|(_, _, text)| !text.is_empty())
                .map(|(t0, t1, text)| WordTiming { t0, t1, text })
                .collect(),
        ),
        Some(Err(reason)) => {
            crate::capture::scrawl_log(format!("Command transcriber: {reason}; estimating word times"));
            None
        }
        None => None,
    };
    let (segments, words) = match words {
        Some(words) if text.is_empty() || !words.is_empty() => {
            let segments = if segments.is_empty() {
                segments_from_words(&words)
            } else {
                segments
            };
            (segments, words)
        }
        _ => {
            estimated = true;
            spread(&text, &segments, audio_ms)
        }
    };
    let model = output
        .model
        .map(|model| model.trim().to_owned())
        .filter(|model| !model.is_empty());
    Ok(TranscriptData {
        source: match &model {
            Some(model) => format!("command:{model}"),
            None => "command".into(),
        },
        model,
        text,
        segments,
        words,
        estimated,
        missing: None,
    })
}

/// Runs the program on `wav` in its own process group and reads its transcript. The group is
/// killed on timeout, and again once the program exits, so a wrapper's leftover model process
/// dies with it and the deadline bounds the whole run either way.
pub fn transcribe(settings: &Settings, wav: &Path, audio_ms: u64) -> Result<TranscriptData> {
    use std::os::unix::process::CommandExt;
    let argv = argv(&settings.command, wav);
    let mut child = Command::new(&argv[0])
        .args(&argv[1..])
        .env("SCRAWL_TRANSCRIBE_VERSION", "1")
        .env("SCRAWL_AUDIO", wav)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()
        .map_err(|error| anyhow::anyhow!("could not start the program ({})", error.kind()))?;
    let stdout = child.stdout.take().expect("stdout was piped");
    let pid = child.id() as libc::pid_t;
    let deadline = Instant::now() + timeout(settings.timeout, audio_ms);
    let (sender, receiver) = mpsc::channel();
    let stop = Arc::new(AtomicBool::new(false));
    let reader_stop = Arc::clone(&stop);
    std::thread::spawn(move || {
        let _ = sender.send(read_until(stdout, deadline, &reader_stop));
    });
    // Nothing wakes this thread when the program exits, so the exit is polled. The group is
    // killed before the leader is reaped: its zombie keeps the group id reserved, so the signal
    // cannot land on a group another process claimed since.
    let mut delivered: Option<Vec<u8>> = None;
    let status = loop {
        let exited = match has_exited(pid) {
            Ok(exited) => exited,
            Err(error) => {
                kill_group(pid);
                let _ = child.wait();
                return Err(error.into());
            }
        };
        if exited {
            kill_group(pid);
            break child.wait()?;
        }
        // The reader hands over early only past the output cap; a program that floods and then
        // idles is rejected for its output, not reported as a timeout later.
        if delivered.is_none()
            && let Ok(out) = receiver.try_recv()
        {
            if out.len() > OUTPUT_LIMIT {
                kill_group(pid);
                let _ = child.wait();
                bail!("output too large");
            }
            delivered = Some(out);
        }
        let now = Instant::now();
        if now >= deadline {
            kill_group(pid);
            let _ = child.wait();
            bail!("timeout");
        }
        std::thread::sleep(EXIT_POLL.min(deadline - now));
    };
    // Everything the program wrote is in the pipe once it has exited, so the reader drains it
    // and stops: a descendant that left the group (setsid) and still holds the pipe can neither
    // add to the transcript nor pin the thread and descriptor, since our end closes here.
    stop.store(true, Ordering::Release);
    let out = match delivered {
        Some(out) => out,
        None => {
            let Ok(out) = receiver.recv_timeout(deadline.saturating_duration_since(Instant::now()))
            else {
                bail!("timeout");
            };
            out
        }
    };
    ensure!(out.len() <= OUTPUT_LIMIT, "output too large");
    ensure!(
        status.success(),
        "exit status {}",
        status.code().unwrap_or(-1)
    );
    parse_output(&out, audio_ms)
}

/// Reads stdout until EOF, one byte past the output cap, the deadline, or `stop` with the pipe
/// drained. Without the last two, a descendant that outlives the program and keeps the pipe
/// would pin this thread and the descriptor for as long as it lived.
fn read_until(mut stdout: std::process::ChildStdout, deadline: Instant, stop: &AtomicBool) -> Vec<u8> {
    use std::os::fd::AsRawFd;
    let fd = stdout.as_raw_fd();
    // SAFETY: fcntl and poll on a descriptor this thread owns for its whole life.
    unsafe {
        libc::fcntl(fd, libc::F_SETFL, libc::fcntl(fd, libc::F_GETFL) | libc::O_NONBLOCK);
    }
    let mut out = Vec::new();
    let mut chunk = [0_u8; 64 * 1024];
    while out.len() <= OUTPUT_LIMIT {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            break;
        }
        let stopping = stop.load(Ordering::Acquire);
        let mut poll = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        let wait_ms = if stopping {
            0
        } else {
            remaining.min(EXIT_POLL).as_millis() as i32
        };
        let ready = unsafe { libc::poll(&mut poll, 1, wait_ms) };
        if ready < 0 && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted {
            continue;
        }
        if ready < 0 || (ready == 0 && stopping) {
            break;
        }
        if ready == 0 {
            continue;
        }
        match stdout.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => out.extend_from_slice(&chunk[..read]),
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if stopping {
                    break;
                }
            }
            Err(_) => break,
        }
    }
    out
}

/// Whether the program has exited, without reaping it: `Child::wait` still collects it after.
fn has_exited(pid: libc::pid_t) -> std::io::Result<bool> {
    // SAFETY: a zeroed siginfo_t is the documented input, and WNOWAIT leaves the child in place.
    let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
    let rc = unsafe {
        libc::waitid(
            libc::P_PID,
            pid as libc::id_t,
            &mut info,
            libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
        )
    };
    if rc != 0 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(info.si_pid != 0)
}

/// The child is the group leader, so a negative pid reaches everything it started. Called only
/// while the leader is alive or still unreaped, when the group id is still its own.
fn kill_group(pid: libc::pid_t) {
    // SAFETY: a plain signal to a process group this process created and still holds.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|arg| (*arg).to_owned()).collect()
    }

    fn sh(script: &str) -> Settings {
        Settings {
            command: strings(&["/bin/sh", "-c", script]),
            timeout: Some(Duration::from_secs(10)),
        }
    }

    /// Removed when the test's value drops, so parallel tests never share a file.
    struct Wav(std::path::PathBuf);

    impl Drop for Wav {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }

    fn wav() -> Wav {
        static COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!(
            "scrawl-command-{}-{}.wav",
            std::process::id(),
            COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        std::fs::write(&path, b"RIFF").unwrap();
        Wav(path)
    }

    #[test]
    fn settings_need_an_absolute_program_and_a_positive_timeout() {
        assert_eq!(settings(None, None), None);
        assert_eq!(settings(Some(&[]), None), None);
        assert_eq!(settings(Some(&strings(&["whisper"])), None), None);
        let ok = settings(Some(&strings(&["/usr/local/bin/whisper", "--fast"])), Some(0)).unwrap();
        assert_eq!(ok.command, strings(&["/usr/local/bin/whisper", "--fast"]));
        assert_eq!(ok.timeout, None);
        let timed = settings(Some(&strings(&["/bin/true"])), Some(90)).unwrap();
        assert_eq!(timed.timeout, Some(Duration::from_secs(90)));
        let absurd = settings(Some(&strings(&["/bin/true"])), Some(u64::MAX)).unwrap();
        assert_eq!(absurd.timeout, Some(Duration::from_secs(MAX_TIMEOUT_SECONDS)));
        assert!(Instant::now().checked_add(absurd.timeout.unwrap()).is_some());
    }

    #[test]
    fn audio_path_replaces_the_placeholder_or_goes_last() {
        let audio = Path::new("/tmp/a b.wav");
        assert_eq!(
            argv(&strings(&["/bin/x", "--in={audio}", "-v"]), audio),
            vec![OsString::from("/bin/x"), "--in=/tmp/a b.wav".into(), "-v".into()]
        );
        assert_eq!(
            argv(&strings(&["/bin/x", "-v"]), audio),
            vec![OsString::from("/bin/x"), "-v".into(), "/tmp/a b.wav".into()]
        );
    }

    #[test]
    fn default_timeout_grows_with_the_recording_from_five_minutes() {
        assert_eq!(timeout(None, 10_000), Duration::from_secs(300));
        assert_eq!(timeout(None, 60_000), Duration::from_secs(660));
        assert_eq!(timeout(Some(Duration::from_secs(7)), 60_000), Duration::from_secs(7));
    }

    #[test]
    fn measured_words_are_kept_and_segments_are_cut_at_sentences() {
        let out = br#"{"version":1,"model":"parakeet","text":"Hi there. Look now","estimated":false,
            "words":[{"t0":0,"t1":200,"text":"Hi"},{"t0":200,"t1":500,"text":"there."},
                     {"t0":900,"t1":1100,"text":"Look"},{"t0":1100,"t1":1400,"text":"now"}],"extra":true}"#;
        let transcript = parse_output(out, 1_500).unwrap();
        assert_eq!(transcript.source, "command:parakeet");
        assert_eq!(transcript.model.as_deref(), Some("parakeet"));
        assert!(!transcript.estimated);
        assert_eq!(transcript.words.len(), 4);
        assert_eq!(transcript.segments.len(), 2);
        assert_eq!((transcript.segments[0].t0, transcript.segments[0].t1), (0, 500));
        assert_eq!(transcript.segments[1].text, "Look now");
    }

    #[test]
    fn text_alone_is_spread_over_the_recording_and_estimated() {
        let transcript = parse_output(br#"{"version":1,"text":"  one   two "}"#, 2_000).unwrap();
        assert_eq!(transcript.source, "command");
        assert_eq!(transcript.text, "one two");
        assert!(transcript.estimated);
        assert_eq!(transcript.words.len(), 2);
        assert_eq!((transcript.words[0].t0, transcript.words[1].t1), (0, 2_000));
        let empty = parse_output(br#"{"version":1,"text":""}"#, 2_000).unwrap();
        assert!(empty.text.is_empty() && empty.words.is_empty());
    }

    #[test]
    fn bad_word_times_are_estimated_over_the_segments() {
        let out = br#"{"version":1,"text":"a b c d","estimated":false,
            "segments":[{"t0":0,"t1":1000,"text":"a b"},{"t0":2000,"t1":3000,"text":"c d"}],
            "words":[{"t0":0,"t1":100,"text":"a"},{"t0":100,"t1":9000,"text":"b"}]}"#;
        let transcript = parse_output(out, 3_000).unwrap();
        assert!(transcript.estimated);
        assert_eq!(transcript.segments.len(), 2);
        assert_eq!(transcript.words.len(), 4);
        assert!(transcript.words[..2].iter().all(|w| w.t1 <= 1_000));
        assert!(transcript.words[2..].iter().all(|w| w.t0 >= 2_000));
    }

    #[test]
    fn ends_near_the_audio_are_clamped_and_bad_segments_are_dropped() {
        let out = br#"{"version":1,"text":"a","segments":[{"t0":-1,"t1":10,"text":"a"}],
            "words":[{"t0":900,"t1":1400,"text":"a"}],"estimated":false}"#;
        let transcript = parse_output(out, 1_000).unwrap();
        assert_eq!(transcript.words[0].t1, 1_000);
        assert_eq!(transcript.segments, vec![TranscriptSegment { t0: 900, t1: 1_000, text: "a".into() }]);
    }

    #[test]
    fn malformed_spans_cost_only_their_array() {
        let out = br#"{"version":1,"text":"a b","estimated":false,
            "segments":[{"t0":0,"t1":1000,"text":"a b"}],
            "words":[{"t0":0.5,"t1":100,"text":"a"},{"t0":100,"text":"b"}]}"#;
        let transcript = parse_output(out, 1_000).unwrap();
        assert_eq!(transcript.text, "a b");
        assert!(transcript.estimated);
        assert_eq!(transcript.words.len(), 2);
        assert_eq!(transcript.segments.len(), 1);
        let nulls = parse_output(br#"{"version":1,"text":"a","segments":null,"words":null}"#, 1_000).unwrap();
        assert_eq!(nulls.words.len(), 1);
    }

    #[test]
    fn wrong_version_missing_text_and_invalid_json_fail_without_echoing_the_text() {
        let spoken = "my secret";
        let version = parse_output(format!(r#"{{"version":2,"text":"{spoken}"}}"#).as_bytes(), 1_000).unwrap_err();
        let no_text = parse_output(format!(r#"{{"version":1,"model":"{spoken}"}}"#).as_bytes(), 1_000).unwrap_err();
        let invalid = parse_output(format!("{spoken} not json").as_bytes(), 1_000).unwrap_err();
        for error in [version, no_text, invalid] {
            assert!(!format!("{error:#}").contains(spoken), "{error:#}");
        }
    }

    /// macOS has no `setsid` binary, so perl's POSIX binding moves the descendant out of the
    /// group; it inherits stdout and outlives the program, which exits at once with its
    /// transcript already written.
    #[test]
    fn a_descendant_outside_the_group_neither_blocks_nor_loses_the_transcript() {
        let wav = wav();
        let escaped = sh(
            r#"/usr/bin/perl -e 'use POSIX; POSIX::setsid(); sleep 3' & printf '{"version":1,"text":"done"}'"#,
        );
        let started = Instant::now();
        assert_eq!(transcribe(&escaped, &wav.0, 1_000).unwrap().text, "done");
        assert!(started.elapsed() < Duration::from_secs(2));
    }

    #[test]
    fn a_child_left_holding_stdout_does_not_hold_up_the_transcript() {
        let wav = wav();
        let started = Instant::now();
        let transcript = transcribe(
            &sh(r#"sleep 3 & printf '{"version":1,"text":"done"}'"#),
            &wav.0,
            1_000,
        )
        .unwrap();
        assert_eq!(transcript.text, "done");
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn a_program_gets_the_audio_and_its_json_comes_back() {
        let wav = wav();
        let settings = sh(r#"test "$SCRAWL_TRANSCRIBE_VERSION" = 1 && test "$SCRAWL_AUDIO" = "$1" && printf '{"version":1,"model":"m","text":"ok"}'"#);
        let settings = Settings {
            command: [settings.command, strings(&["sh", "{audio}"])].concat(),
            ..settings
        };
        let transcript = transcribe(&settings, &wav.0, 1_000).unwrap();
        assert_eq!(transcript.source, "command:m");
        assert_eq!(transcript.text, "ok");
    }

    #[test]
    fn failures_are_short_reasons_without_program_output() {
        let wav = wav();
        let crash = transcribe(&sh("echo spoken words; exit 3"), &wav.0, 1_000).unwrap_err();
        assert_eq!(format!("{crash:#}"), "exit status 3");
        let hang = Settings {
            timeout: Some(Duration::from_millis(300)),
            ..sh("sleep 30")
        };
        let started = Instant::now();
        assert_eq!(format!("{:#}", transcribe(&hang, &wav.0, 1_000).unwrap_err()), "timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
        let flood = transcribe(&sh("yes spoken"), &wav.0, 1_000).unwrap_err();
        assert_eq!(format!("{flood:#}"), "output too large");
        let started = Instant::now();
        let flood_then_idle = transcribe(
            &sh("yes spoken | head -c 9000000; sleep 30"),
            &wav.0,
            1_000,
        )
        .unwrap_err();
        assert_eq!(format!("{flood_then_idle:#}"), "output too large");
        assert!(started.elapsed() < Duration::from_secs(5));
        let missing = transcribe(
            &Settings {
                command: strings(&["/nonexistent/transcriber"]),
                timeout: None,
            },
            &wav.0,
            1_000,
        )
        .unwrap_err();
        assert!(format!("{missing:#}").starts_with("could not start"));
    }
}

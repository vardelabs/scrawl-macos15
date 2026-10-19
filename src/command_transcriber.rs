//! A program of the user's choice as the transcriber (`"transcriber": "command"`). Scrawl runs
//! `transcribeCommand` on the recording's 16 kHz mono WAV and reads one JSON object from its
//! stdout: `version` (1), `text`, and optional `model`, `segments`, `words` and `estimated`, with
//! times in integer milliseconds from the start of the WAV, the same shape as `scrawl.json`.
//!
//! The program's stderr is discarded and its stdout is never logged, so nothing the user said
//! reaches the log. A failure surfaces as a short reason (`timeout`, `words[12].t1 out of range`)
//! and the caller falls back to Apple Speech for that recording.

use std::ffi::OsString;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use anyhow::{Result, bail, ensure};
use serde::Deserialize;

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

pub const AUDIO_PLACEHOLDER: &str = "{audio}";
const OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
const MIN_TIMEOUT: Duration = Duration::from_secs(300);
/// A span may end this far past the audio before its whole array is distrusted.
const END_SLACK_MS: u64 = 500;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Settings {
    pub command: Vec<String>,
    pub timeout: Option<Duration>,
}

/// The usable form of the config keys: a non-empty argv whose first entry is an absolute path
/// (the app runs without the user's shell `PATH`), and a timeout only when it is positive.
pub fn settings(command: Option<&[String]>, timeout_seconds: Option<u64>) -> Option<Settings> {
    let command = command?.to_vec();
    if !command.first().is_some_and(|program| program.starts_with('/')) {
        return None;
    }
    Some(Settings {
        command,
        timeout: timeout_seconds
            .filter(|&seconds| seconds > 0)
            .map(Duration::from_secs),
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

#[derive(Deserialize)]
struct Output {
    version: Option<i64>,
    model: Option<String>,
    text: Option<String>,
    segments: Option<Vec<Span>>,
    words: Option<Vec<Span>>,
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
                return Err(format!("{name}[{index}].t1 out of range"));
            }
            let clamp = |time: i64| (time as u64).min(audio_ms);
            Ok((clamp(span.t0), clamp(span.t1), span.text.trim().to_owned()))
        })
        .collect()
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
    let text = output
        .text
        .unwrap_or_default()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let mut estimated = output.estimated.unwrap_or(true);
    let segments: Vec<TranscriptSegment> = match output.segments.as_deref().map(|spans| check_spans(spans, "segments", audio_ms)) {
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
    let words: Option<Vec<WordTiming>> = match output.words.as_deref().map(|spans| check_spans(spans, "words", audio_ms)) {
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
    })
}

/// Runs the program on `wav` in its own process group and reads its transcript. The group is
/// killed on timeout so a wrapper script never leaves a model process behind.
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
    let caller = std::thread::current();
    let eof = Arc::new(AtomicBool::new(false));
    let reader_eof = Arc::clone(&eof);
    let reader = std::thread::spawn(move || {
        let mut out = Vec::new();
        let _ = stdout.take((OUTPUT_LIMIT + 1) as u64).read_to_end(&mut out);
        reader_eof.store(true, Ordering::Release);
        caller.unpark();
        out
    });
    let deadline = Instant::now() + timeout(settings.timeout, audio_ms);
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if eof.load(Ordering::Acquire) {
            kill_group(&mut child);
            break child.wait()?;
        }
        let now = Instant::now();
        if now >= deadline {
            kill_group(&mut child);
            let _ = child.wait();
            bail!("timeout");
        }
        std::thread::park_timeout(deadline - now);
    };
    let out = reader.join().unwrap_or_default();
    ensure!(out.len() <= OUTPUT_LIMIT, "output too large");
    ensure!(
        status.success(),
        "exit status {}",
        status.code().unwrap_or(-1)
    );
    parse_output(&out, audio_ms)
}

/// The child is the group leader, so a negative pid reaches everything it started.
fn kill_group(child: &mut std::process::Child) {
    let pid = child.id() as i32;
    // SAFETY: a plain signal to a process group this process created and still owns.
    unsafe {
        libc::kill(-pid, libc::SIGKILL);
    }
    let _ = child.kill();
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

    fn wav() -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!("scrawl-command-{}.wav", std::process::id()));
        std::fs::write(&path, b"RIFF").unwrap();
        path
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
    fn wrong_version_and_invalid_json_fail_without_echoing_the_text() {
        let spoken = "my secret";
        let version = parse_output(format!(r#"{{"version":2,"text":"{spoken}"}}"#).as_bytes(), 1_000).unwrap_err();
        let invalid = parse_output(format!("{spoken} not json").as_bytes(), 1_000).unwrap_err();
        for error in [version, invalid] {
            assert!(!format!("{error:#}").contains(spoken), "{error:#}");
        }
    }

    #[test]
    fn a_program_gets_the_audio_and_its_json_comes_back() {
        let wav = wav();
        let settings = sh(r#"test "$SCRAWL_TRANSCRIBE_VERSION" = 1 && test "$SCRAWL_AUDIO" = "$1" && printf '{"version":1,"model":"m","text":"ok"}'"#);
        let settings = Settings {
            command: [settings.command, strings(&["sh", "{audio}"])].concat(),
            ..settings
        };
        let transcript = transcribe(&settings, &wav, 1_000).unwrap();
        assert_eq!(transcript.source, "command:m");
        assert_eq!(transcript.text, "ok");
    }

    #[test]
    fn failures_are_short_reasons_without_program_output() {
        let wav = wav();
        let crash = transcribe(&sh("echo spoken words; exit 3"), &wav, 1_000).unwrap_err();
        assert_eq!(format!("{crash:#}"), "exit status 3");
        let hang = Settings {
            timeout: Some(Duration::from_millis(300)),
            ..sh("sleep 30")
        };
        let started = Instant::now();
        assert_eq!(format!("{:#}", transcribe(&hang, &wav, 1_000).unwrap_err()), "timeout");
        assert!(started.elapsed() < Duration::from_secs(5));
        let flood = transcribe(&sh("yes spoken"), &wav, 1_000).unwrap_err();
        assert_eq!(format!("{flood:#}"), "output too large");
        let missing = transcribe(
            &Settings {
                command: strings(&["/nonexistent/transcriber"]),
                timeout: None,
            },
            &wav,
            1_000,
        )
        .unwrap_err();
        assert!(format!("{missing:#}").starts_with("could not start"));
    }
}

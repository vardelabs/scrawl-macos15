//! The local formatter model (`formatCommand`, `scripts/scrawl-format.py`), started once a
//! dictation has run a few seconds so its model loads while the user talks. `format` hands it the text and waits
//! a short while; dropping it unused (a short or cancelled dictation) kills its process group.
//! Like the command transcriber, its stderr is discarded and its stdout never logged. After each
//! use a fresh one is parked (`crate::park`), so the next dictation finds its model loaded.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The most a formatter's reply may be; a long dictation's layout is well under this.
const OUTPUT_LIMIT: u64 = 1024 * 1024;

pub struct Formatter {
    child: Option<Child>,
    command: Vec<String>,
    /// When the learned layouts it read at start were last saved, so a parked one that predates
    /// a newly learned layout isn't used.
    examples: Option<std::time::SystemTime>,
}

static PARKED: crate::park::Park<Formatter> = crate::park::Park::new();

fn examples_saved() -> Option<std::time::SystemTime> {
    std::fs::metadata(super::layout::default_path()).and_then(|meta| meta.modified()).ok()
}

#[derive(serde::Deserialize)]
struct Reply {
    version: u32,
    text: String,
}

impl Formatter {
    /// Starts `command` (an argv whose first entry is an absolute path) with `SCRAWL_PRESTART=1`
    /// and `SCRAWL_FORMAT_EXAMPLES`, the layouts learned from the user's edits (`layout`), which
    /// it reads while the user talks.
    pub fn prestart(command: &[String]) -> anyhow::Result<Self> {
        let parked = PARKED.take().filter(|parked| {
            parked.command == command && parked.examples == examples_saved() && parked.alive()
        });
        match parked {
            Some(parked) => Ok(parked),
            None => Self::start(command),
        }
    }

    fn start(command: &[String]) -> anyhow::Result<Self> {
        let examples = examples_saved();
        let (program, args) = command.split_first().ok_or_else(|| anyhow::anyhow!("formatCommand is empty"))?;
        let child = Command::new(program)
            .args(args)
            .env("SCRAWL_PRESTART", "1")
            .env("SCRAWL_FORMAT_EXAMPLES", super::layout::default_path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|error| anyhow::anyhow!("could not start formatCommand ({})", error.kind()))?;
        Ok(Self { child: Some(child), command: command.to_vec(), examples })
    }

    /// Whether its process is still running, checked without reaping it.
    fn alive(&self) -> bool {
        self.child.as_ref().is_some_and(|child| crate::command_transcriber::running(child.id() as libc::pid_t))
    }

    /// Puts a formatter a dictation didn't need back, for the next one.
    pub fn park(self) {
        PARKED.put(self, crate::park::PARK_FOR);
    }

    /// Starts and parks the next dictation's formatter, so its model loads now.
    fn park_next(command: Vec<String>) {
        std::thread::spawn(move || match Self::start(&command) {
            Ok(formatter) => formatter.park(),
            Err(error) => crate::capture::scrawl_log(format!("Dictation: parking the formatter failed: {error:#}")),
        });
    }

    /// The laid-out text, or `None` when the formatter fails, answers badly or takes longer than
    /// `wait` (counted from now, while its model is already loading or loaded).
    pub fn format(mut self, text: &str, pauses: &[usize], wait: Duration) -> Option<String> {
        let mut child = self.child.take()?;
        let request = serde_json::json!({ "text": text, "pauses": pauses }).to_string();
        let mut stdin = child.stdin.take()?;
        let mut stdout = child.stdout.take()?;
        let (done, reply) = flume::bounded(1);
        // The write is on this thread too: a long text fills the pipe until the formatter reads
        // it, which must not hold up the paste past `wait`.
        std::thread::spawn(move || {
            let sent = writeln!(stdin, "{request}").is_ok();
            drop(stdin);
            let mut out = String::new();
            let read = stdout.by_ref().take(OUTPUT_LIMIT).read_to_string(&mut out).is_ok();
            let _ = done.send((sent && read).then_some(out));
        });
        let out = reply.recv_timeout(wait).ok().flatten();
        end(child);
        Self::park_next(std::mem::take(&mut self.command));
        let reply: Reply = serde_json::from_str(&out?).ok()?;
        (reply.version == 1).then_some(reply.text)
    }
}

impl Drop for Formatter {
    fn drop(&mut self) {
        if let Some(child) = self.child.take() {
            end(child);
        }
    }
}

/// Kills the formatter's group (its model process with it) and reaps it off this thread.
fn end(mut child: Child) {
    crate::command_transcriber::kill_group(child.id() as libc::pid_t);
    std::thread::spawn(move || {
        let _ = child.wait();
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    /// A formatter script in a temp dir, run with /bin/sh.
    fn script(name: &str, body: &str) -> Vec<String> {
        let dir = std::env::temp_dir().join(format!("scrawl-formatter-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        vec!["/bin/sh".into(), path.display().to_string()]
    }

    #[test]
    fn a_formatter_that_answers_gives_its_text() {
        let echo = script("echo.sh", "read line\nprintf '{\"version\": 1, \"text\": \"laid out\"}'\n");
        let formatter = Formatter::prestart(&echo).unwrap();
        assert_eq!(formatter.format("some text", &[], Duration::from_secs(5)).as_deref(), Some("laid out"));
    }

    #[test]
    fn a_silent_slow_or_broken_formatter_gives_nothing_within_the_wait() {
        for (name, body) in [
            ("silent.sh", "sleep 30\n"),
            ("broken.sh", "read line\necho not json\n"),
            ("old.sh", "read line\nprintf '{\"version\": 2, \"text\": \"x\"}'\n"),
            ("quits.sh", "exit 1\n"),
        ] {
            let formatter = Formatter::prestart(&script(name, body)).unwrap();
            let started = Instant::now();
            let long = "word ".repeat(40_000);
            assert_eq!(formatter.format(&long, &[1], Duration::from_millis(500)), None, "{name}");
            assert!(started.elapsed() < Duration::from_secs(3), "{name} held the paste");
        }
    }
}

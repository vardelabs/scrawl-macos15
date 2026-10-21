//! The local formatter model (`formatCommand`, `scripts/scrawl-format.py`), started when a
//! dictation shows so its model loads while the user talks. `format` hands it the text and waits
//! a short while; dropping it unused (a short or cancelled dictation) kills its process group.
//! Like the command transcriber, its stderr is discarded and its stdout never logged.

use std::io::{Read, Write};
use std::os::unix::process::CommandExt;
use std::process::{Child, Command, Stdio};
use std::time::Duration;

/// The most a formatter's reply may be; a long dictation's layout is well under this.
const OUTPUT_LIMIT: u64 = 1024 * 1024;

pub struct Formatter {
    child: Option<Child>,
}

#[derive(serde::Deserialize)]
struct Reply {
    version: u32,
    text: String,
}

impl Formatter {
    /// Starts `command` (an argv whose first entry is an absolute path) with `SCRAWL_PRESTART=1`.
    pub fn prestart(command: &[String]) -> anyhow::Result<Self> {
        let (program, args) = command.split_first().ok_or_else(|| anyhow::anyhow!("formatCommand is empty"))?;
        let child = Command::new(program)
            .args(args)
            .env("SCRAWL_PRESTART", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()
            .map_err(|error| anyhow::anyhow!("could not start formatCommand ({})", error.kind()))?;
        Ok(Self { child: Some(child) })
    }

    /// The laid-out text, or `None` when the formatter fails, answers badly or takes longer than
    /// `wait` (counted from now, while its model is already loading or loaded).
    pub fn format(mut self, text: &str, pauses: &[usize], wait: Duration) -> Option<String> {
        let mut child = self.child.take()?;
        let request = serde_json::json!({ "text": text, "pauses": pauses });
        let mut stdin = child.stdin.take()?;
        let sent = writeln!(stdin, "{request}").is_ok();
        drop(stdin);
        let mut stdout = child.stdout.take()?;
        let (done, reply) = flume::bounded(1);
        std::thread::spawn(move || {
            let mut out = String::new();
            let read = stdout.by_ref().take(OUTPUT_LIMIT).read_to_string(&mut out).is_ok();
            let _ = done.send(read.then_some(out));
        });
        let out = reply.recv_timeout(wait).ok().flatten();
        end(child);
        let reply: Reply = serde_json::from_str(&out.filter(|_| sent)?).ok()?;
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

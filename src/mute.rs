//! Mutes the Mac's sound output while a recording runs, so the narration isn't recorded over
//! music or a video, and unmutes it when the recording finishes or is cancelled, the way
//! dictation apps do. Only Scrawl's own mute is undone: output that was already muted stays
//! muted. Switching what is recorded keeps it muted. Scrawl records the microphone, never the
//! system's output, so muting costs the capture nothing. Off with `"muteAudio": false` in
//! `config.json`.

use std::process::Command;
use std::sync::OnceLock;
use std::sync::mpsc::{Sender, channel};

#[derive(Clone, Copy)]
enum Op {
    Mute,
    Restore,
}

/// One worker runs every mute and restore in the order they were asked for, so a quick start
/// and stop can never unmute before the mute has landed.
fn worker() -> Option<&'static Sender<Op>> {
    static WORKER: OnceLock<Option<Sender<Op>>> = OnceLock::new();
    WORKER
        .get_or_init(|| {
            let (sender, receiver) = channel::<Op>();
            std::thread::Builder::new()
                .name("scrawl-mute".into())
                .spawn(move || {
                    let mut muted_by_us = false;
                    for op in receiver {
                        muted_by_us = apply(op, muted_by_us);
                    }
                })
                .ok()
                .map(|_| sender)
        })
        .as_ref()
}

/// Runs one op and returns whether Scrawl's mute is still in place afterwards. A failed
/// unmute keeps it, so the next restore tries again.
fn apply(op: Op, muted_by_us: bool) -> bool {
    match op {
        Op::Mute => {
            let state = osascript(READ_MUTED);
            if !should_mute(muted_by_us, state.as_deref()) || osascript(MUTE).is_none() {
                return muted_by_us;
            }
            crate::capture::scrawl_log("Mute: sound output muted for the recording".to_owned());
            true
        }
        Op::Restore if muted_by_us => {
            if osascript(UNMUTE).is_none() {
                return true;
            }
            crate::capture::scrawl_log("Mute: sound output restored".to_owned());
            false
        }
        Op::Restore => false,
    }
}

const READ_MUTED: &str = "output muted of (get volume settings)";
const MUTE: &str = "set volume with output muted";
const UNMUTE: &str = "set volume without output muted";

fn osascript(script: &str) -> Option<String> {
    let output = Command::new("/usr/bin/osascript").args(["-e", script]).output().ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// `true` when the output should be muted now: it is playing sound and Scrawl hasn't muted it.
fn should_mute(already_muted_by_us: bool, output_muted: Option<&str>) -> bool {
    !already_muted_by_us && output_muted == Some("false")
}

/// At a recording's start, off the main thread.
pub fn mute() {
    if !crate::handoff::mute_audio() {
        return;
    }
    if let Some(worker) = worker() {
        let _ = worker.send(Op::Mute);
    }
}

/// When a recording finishes or is cancelled: unmutes only what `mute` muted.
pub fn restore() {
    if let Some(worker) = worker() {
        let _ = worker.send(Op::Restore);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutes_only_sound_that_is_playing_and_not_already_ours() {
        assert!(should_mute(false, Some("false")));
        assert!(!should_mute(false, Some("true")), "a muted Mac stays the user's to unmute");
        assert!(!should_mute(true, Some("false")), "a second start mutes nothing more");
        assert!(!should_mute(false, None), "an unreadable state is left alone");
    }
}

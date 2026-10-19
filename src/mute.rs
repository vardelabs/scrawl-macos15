//! Mutes the Mac's sound output while a recording runs, so the narration isn't recorded over
//! music or a video, and unmutes it when the recording finishes or is cancelled, the way
//! dictation apps do. Only Scrawl's own mute is undone: output that was already muted stays
//! muted. Switching what is recorded keeps it muted. Scrawl records the microphone, never the
//! system's output, so muting costs the capture nothing. Off with `"muteAudio": false` in
//! `config.json`.

use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

/// Scrawl muted the output and has not unmuted it yet.
static MUTED_BY_US: AtomicBool = AtomicBool::new(false);
/// Mute and unmute run in order, so a quick start and stop can't unmute before the mute lands.
static ORDER: Mutex<()> = Mutex::new(());

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
    let _ = std::thread::Builder::new().name("scrawl-mute".into()).spawn(|| {
        let _order = ORDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let state = osascript(READ_MUTED);
        if !should_mute(MUTED_BY_US.load(Ordering::Acquire), state.as_deref()) {
            return;
        }
        if osascript(MUTE).is_some() {
            MUTED_BY_US.store(true, Ordering::Release);
            crate::capture::scrawl_log("Mute: sound output muted for the recording".to_owned());
        }
    });
}

/// When a recording finishes or is cancelled: unmutes only what `mute` muted.
pub fn restore() {
    let _ = std::thread::Builder::new().name("scrawl-unmute".into()).spawn(|| {
        let _order = ORDER.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if MUTED_BY_US.swap(false, Ordering::AcqRel) && osascript(UNMUTE).is_some() {
            crate::capture::scrawl_log("Mute: sound output restored".to_owned());
        }
    });
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

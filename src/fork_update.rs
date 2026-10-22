//! Check for Updates… in this fork: Sparkle reads Kit's update feed and would replace the fork
//! with his build, so the menu item and the Settings button run `scrawl-helper fork-update`
//! (`helper/ForkUpdate.swift`), which installs this repository's own GitHub releases.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::capture::scrawl_log;

/// The repository whose releases the fork installs.
const REPO: &str = "vardelabs/scrawl-macos15";

/// One updater window at a time: a second click while one is open does nothing.
static OPEN: AtomicBool = AtomicBool::new(false);

/// Whether this copy can update itself: only `~/Applications/Scrawl.app`, the copy the release's
/// installer replaces (it quits every Scrawl by name, so another copy would close, not update).
pub fn available() -> bool {
    let installed = std::env::var_os("HOME")
        .map(|home| std::path::Path::new(&home).join("Applications/Scrawl.app/Contents/MacOS"));
    let exe = std::env::current_exe().and_then(std::fs::canonicalize);
    matches!((installed, exe), (Some(dir), Ok(exe)) if exe.parent() == Some(dir.as_path()))
        && crate::helper::path().is_ok()
}

/// Opens the updater window; what it finds and does goes to the log as one line per step.
pub fn check() {
    if !available() || OPEN.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = crate::helper::path().and_then(|helper| {
        Command::new(helper)
            .args(["fork-update", "--repo", REPO])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(anyhow::Error::from)
    });
    let mut child = match spawned {
        Ok(child) => child,
        Err(error) => {
            scrawl_log(format!("Fork update: could not open the updater: {error:#}"));
            OPEN.store(false, Ordering::Release);
            return;
        }
    };
    let stdout = child.stdout.take();
    std::thread::spawn(move || {
        for line in stdout.into_iter().flat_map(|out| BufReader::new(out).lines()) {
            match line {
                Ok(line) => scrawl_log(format!("Fork update: {line}")),
                Err(_) => break,
            }
        }
        let _ = child.wait();
        OPEN.store(false, Ordering::Release);
    });
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn the_helper_parses_releases_and_checksums_and_decides_what_is_newer() {
        let helper = crate::helper::path().unwrap();
        let output = std::process::Command::new(helper)
            .args(["fork-update", "--self-test"])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        assert!(String::from_utf8_lossy(&output.stdout).contains(r#""selfTest":"ok""#));
    }
}

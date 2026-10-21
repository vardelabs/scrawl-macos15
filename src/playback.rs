//! A warm helper handles volume ramps (one second, or a quick one for dictation) without blocking recording or the UI.
//! Pipe EOF also restores volume when the parent exits unexpectedly.
use std::io::Write;
use std::process::{Child, Command, Stdio};
use std::sync::{Mutex, OnceLock};

pub struct Guard;
static HELPER: OnceLock<Mutex<Option<Child>>> = OnceLock::new();

fn level(mode: Option<&str>, percent: Option<u8>) -> Option<u8> {
    match mode {
        Some("mute") => Some(0),
        Some("lower") => Some(percent.unwrap_or(20).min(100)),
        _ => None,
    }
}

fn command(line: Option<&str>) -> anyhow::Result<()> {
    let mut helper = HELPER.get_or_init(|| Mutex::new(None)).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(child) = helper.as_mut()
        && child.try_wait()?.is_some()
    {
        helper.take();
    }
    if helper.is_none() {
        *helper = Some(Command::new(crate::helper::path()?)
            .args(["playback-volume", "--server"])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()?);
    }
    if let Some(line) = line {
        let stdin = helper.as_mut().and_then(|child| child.stdin.as_mut())
            .ok_or_else(|| anyhow::anyhow!("Playback helper stdin missing"))?;
        writeln!(stdin, "{line}")?;
        stdin.flush()?;
    }
    Ok(())
}

pub fn warm_up() {
    if let Err(error) = command(None) {
        crate::capture::scrawl_log(format!("Playback helper: {error:#}"));
    }
}

/// Dictation's ramp: playback is quiet before the first word, not a second later.
const QUICK_RAMP_S: f32 = 0.12;

impl Guard {
    pub fn start() -> Option<Self> {
        Self::start_with(None)
    }

    pub fn start_quick() -> Option<Self> {
        Self::start_with(Some(QUICK_RAMP_S))
    }

    fn start_with(seconds: Option<f32>) -> Option<Self> {
        let config = crate::handoff::load_config_from(&crate::handoff::default_config_path());
        let percent = level(Some(crate::handoff::playback_mode_in(&config)), config.playback_level)?;
        let line = match seconds {
            Some(seconds) => format!("start {percent} {seconds}"),
            None => format!("start {percent}"),
        };
        match command(Some(&line)) {
            Ok(()) => Some(Self),
            Err(error) => {
                crate::capture::scrawl_log(format!("Playback volume: {error:#}"));
                None
            }
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        if let Err(error) = command(Some("stop")) {
            crate::capture::scrawl_log(format!("Restoring playback volume: {error:#}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn only_mute_and_lower_change_playback_and_levels_are_bounded() {
        assert_eq!(level(None, Some(0)), None);
        assert_eq!(level(Some("unknown"), Some(0)), None);
        assert_eq!(level(Some("unchanged"), Some(0)), None);
        assert_eq!(level(Some("mute"), Some(80)), Some(0));
        assert_eq!(level(Some("lower"), None), Some(20));
        assert_eq!(level(Some("lower"), Some(0)), Some(0));
        assert_eq!(level(Some("lower"), Some(255)), Some(100));
    }
}

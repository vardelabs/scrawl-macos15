//! Voice-only dictation: hold (or double tap) a modifier chord, talk, and the text is pasted into
//! the app in front. No screen is recorded. See `docs/dictation.md`.

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod chord;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod context;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod format;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod history;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod layout;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod learn;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod notices;
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub mod spacing;

#[cfg(target_os = "macos")]
mod formatter;
#[cfg(target_os = "macos")]
mod session;
#[cfg(target_os = "macos")]
pub use session::{Dictation, PillInput};

/// The default chord, as Wispr Flow binds it.
pub const DEFAULT_SHORTCUT: &str = "⌃⌥";

/// The chord's modifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Modifiers {
    pub control: bool,
    pub option: bool,
    pub shift: bool,
    pub command: bool,
}

/// `⌃⌥`-style symbols, in any order. One modifier alone would fire while typing, so a chord
/// needs two or more; anything else is not a chord.
pub fn parse_shortcut(text: &str) -> Option<Modifiers> {
    let mut mods = Modifiers {
        control: false,
        option: false,
        shift: false,
        command: false,
    };
    for ch in text.trim().chars() {
        let slot = match ch {
            '⌃' => &mut mods.control,
            '⌥' => &mut mods.option,
            '⇧' => &mut mods.shift,
            '⌘' => &mut mods.command,
            _ => return None,
        };
        if std::mem::replace(slot, true) {
            return None;
        }
    }
    let count = [mods.control, mods.option, mods.shift, mods.command]
        .into_iter()
        .filter(|on| *on)
        .count();
    (count >= 2).then_some(mods)
}

/// Dictation on other platforms: never on, so the coordinator's code is the same everywhere.
#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct Dictation;

#[cfg(not(target_os = "macos"))]
#[derive(Default)]
pub struct PillInput {
    pub hovered: bool,
    pub clicks: Vec<crate::native::pill::PillAction>,
    pub shown: bool,
}

#[cfg(not(target_os = "macos"))]
impl Dictation {
    pub fn is_busy(&self) -> bool {
        false
    }

    pub fn cede(&mut self) {}

    pub fn tick(
        &mut self,
        _telemetry: &crate::capture::SharedCaptureTelemetry,
        _capture_busy: bool,
        _pill: PillInput,
    ) -> crate::native::pill::model::Pres {
        crate::native::pill::model::Pres::Hidden
    }

    pub fn clock(&self) -> String {
        String::new()
    }

    pub fn is_live(&self) -> bool {
        false
    }

    pub fn take_open(&mut self) -> Option<crate::settings::Focus> {
        None
    }

    pub fn mic_rms(&self) -> Option<f32> {
        None
    }

    pub fn copy_recent(&mut self, _at_ms: u64) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chords_need_two_distinct_modifiers() {
        let ctrl_opt = parse_shortcut(DEFAULT_SHORTCUT).unwrap();
        assert!(ctrl_opt.control && ctrl_opt.option && !ctrl_opt.shift && !ctrl_opt.command);
        assert_eq!(parse_shortcut(" ⌥⌃ "), Some(ctrl_opt));
        assert!(parse_shortcut("⌃⌥⇧⌘").is_some());
        assert_eq!(parse_shortcut("⌥"), None);
        assert_eq!(parse_shortcut("⌥⌥"), None);
        assert_eq!(parse_shortcut("⌃S"), None);
        assert_eq!(parse_shortcut(""), None);
    }
}

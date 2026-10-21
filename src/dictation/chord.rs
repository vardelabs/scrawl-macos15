//! When the dictation chord means "talk": hold to talk, double tap to latch, and anything that
//! makes the chord part of another shortcut means nothing. Pure, so it is tested without a Mac.

/// Held longer than this, the chord is hold-to-talk; shorter, a tap.
pub const TAP_MS: u64 = 250;
/// A second tap this soon after the first latches.
pub const DOUBLE_TAP_MS: u64 = 350;
/// A forgotten latched dictation stops itself.
pub const MAX_MS: u64 = 10 * 60 * 1_000;

/// The chord's modifiers as polled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mods {
    /// Not all of the chord's modifiers are down.
    Up,
    /// Exactly the chord's modifiers.
    Exact,
    /// The chord's modifiers and another one.
    Extra,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    None,
    /// Start the microphone, before anything is shown, so the first word is kept.
    Start,
    /// It is dictation: show the pill (with ✕ and ✓ when latched).
    Show { latched: bool },
    Finish,
    /// Not dictation after all, or cancelled: drop the recording.
    Discard,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum State {
    #[default]
    Idle,
    Pressed { since_ms: u64 },
    Holding,
    Tapped { at_ms: u64 },
    /// The second tap is down; latched once it comes up.
    Arming,
    Latched,
    /// The chord is down again while latched; released cleanly, it confirms.
    Confirming,
}

#[derive(Clone, Copy)]
struct Input {
    now_ms: u64,
    mods: Mods,
    down: bool,
    pressed: bool,
    typed: bool,
}

#[derive(Debug, Default)]
pub struct Chord {
    state: State,
    started_ms: u64,
    /// The key tap's key-down count at the poll before the chord last went down, so a key
    /// pressed in the same poll interval as the modifiers still counts as typed.
    keys: u64,
    /// The count at the last poll.
    seen: u64,
    was_down: bool,
}

impl Chord {
    pub fn is_active(&self) -> bool {
        self.state != State::Idle
    }

    pub fn is_latched(&self) -> bool {
        matches!(self.state, State::Latched | State::Confirming)
    }

    /// Polled every few ms with the modifiers and the key tap's key-down count.
    pub fn tick(&mut self, now_ms: u64, mods: Mods, key_downs: u64) -> Action {
        let down = mods != Mods::Up;
        let pressed = down && !self.was_down;
        self.was_down = down;
        if pressed {
            self.keys = self.seen;
        }
        self.seen = key_downs;
        let typed = key_downs != self.keys;
        if self.is_active() && now_ms.saturating_sub(self.started_ms) >= MAX_MS {
            return self.end(Action::Finish);
        }
        let input = Input { now_ms, mods, down, pressed, typed };
        match self.state {
            State::Idle => self.idle(input),
            State::Pressed { since_ms } => self.pressed(input, since_ms),
            State::Holding | State::Arming if mods == Mods::Extra || typed => {
                self.end(Action::Discard)
            }
            State::Holding if !down => self.end(Action::Finish),
            State::Tapped { at_ms } => self.tapped(input, at_ms),
            State::Arming if !down => {
                self.state = State::Latched;
                Action::None
            }
            State::Latched if pressed => {
                self.state = State::Confirming;
                Action::None
            }
            State::Confirming => self.confirming(input),
            _ => Action::None,
        }
    }

    fn idle(&mut self, input: Input) -> Action {
        if !input.pressed || input.mods != Mods::Exact {
            return Action::None;
        }
        self.started_ms = input.now_ms;
        self.state = State::Pressed { since_ms: input.now_ms };
        Action::Start
    }

    fn pressed(&mut self, input: Input, since_ms: u64) -> Action {
        let held_ms = input.now_ms.saturating_sub(since_ms);
        if input.mods == Mods::Extra || input.typed {
            self.end(Action::Discard)
        } else if !input.down && held_ms < TAP_MS {
            self.state = State::Tapped { at_ms: input.now_ms };
            Action::None
        } else if !input.down {
            self.end(Action::Finish)
        } else if held_ms >= TAP_MS {
            self.state = State::Holding;
            Action::Show { latched: false }
        } else {
            Action::None
        }
    }

    fn tapped(&mut self, input: Input, at_ms: u64) -> Action {
        if input.pressed && input.mods == Mods::Exact {
            self.state = State::Arming;
            Action::Show { latched: true }
        } else if input.pressed
            || input.typed
            || input.now_ms.saturating_sub(at_ms) >= DOUBLE_TAP_MS
        {
            self.end(Action::Discard)
        } else {
            Action::None
        }
    }

    fn confirming(&mut self, input: Input) -> Action {
        if input.mods == Mods::Extra || input.typed {
            // Still down, so only a fresh press after this can confirm.
            self.state = State::Latched;
            Action::None
        } else if !input.down {
            self.end(Action::Finish)
        } else {
            Action::None
        }
    }

    /// Esc or ✕.
    pub fn cancel(&mut self) -> Action {
        if self.is_active() { self.end(Action::Discard) } else { Action::None }
    }

    /// Straight to hands-free, as a click on the idle pill starts it. False if already active.
    pub fn latch(&mut self, now_ms: u64) -> bool {
        if self.is_active() {
            return false;
        }
        self.started_ms = now_ms;
        self.state = State::Latched;
        true
    }

    /// ✓ on the pill.
    pub fn confirm(&mut self) -> Action {
        if self.is_active() { self.end(Action::Finish) } else { Action::None }
    }

    fn end(&mut self, action: Action) -> Action {
        self.state = State::Idle;
        action
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Mods::{Exact, Extra, Up};

    #[test]
    fn holding_the_chord_talks_and_releasing_it_finishes() {
        let mut c = Chord::default();
        assert_eq!(c.tick(0, Exact, 0), Action::Start);
        assert_eq!(c.tick(100, Exact, 0), Action::None);
        assert_eq!(c.tick(TAP_MS, Exact, 0), Action::Show { latched: false });
        assert_eq!(c.tick(2_000, Exact, 0), Action::None);
        assert_eq!(c.tick(2_008, Up, 0), Action::Finish);
        assert!(!c.is_active());
    }

    #[test]
    fn a_double_tap_latches_until_the_chord_again() {
        let mut c = Chord::default();
        assert_eq!(c.tick(0, Exact, 0), Action::Start);
        assert_eq!(c.tick(80, Up, 0), Action::None);
        assert_eq!(c.tick(200, Exact, 0), Action::Show { latched: true });
        assert_eq!(c.tick(260, Up, 0), Action::None);
        assert!(c.is_latched());
        // Typing while latched is fine; time passes.
        assert_eq!(c.tick(5_000, Up, 7), Action::None);
        assert_eq!(c.tick(9_000, Exact, 7), Action::None);
        assert_eq!(c.tick(9_090, Up, 7), Action::Finish);
        assert!(!c.is_active());
    }

    #[test]
    fn a_shortcut_pressed_while_latched_does_not_confirm() {
        let mut c = Chord::default();
        c.tick(0, Exact, 0);
        c.tick(80, Up, 0);
        c.tick(200, Exact, 0);
        c.tick(260, Up, 0);
        assert_eq!(c.tick(1_000, Exact, 0), Action::None);
        // ⌃⌥← in the editor.
        assert_eq!(c.tick(1_050, Exact, 1), Action::None);
        assert_eq!(c.tick(1_100, Up, 1), Action::None);
        assert!(c.is_latched());
        assert_eq!(c.tick(2_000, Exact, 1), Action::None);
        assert_eq!(c.tick(2_050, Extra, 1), Action::None);
        assert_eq!(c.tick(2_100, Up, 1), Action::None);
        assert!(c.is_latched());
        assert_eq!(c.confirm(), Action::Finish);
    }

    #[test]
    fn a_key_or_another_modifier_during_the_chord_discards() {
        let mut c = Chord::default();
        assert_eq!(c.tick(0, Exact, 4), Action::Start);
        assert_eq!(c.tick(40, Exact, 5), Action::Discard);
        assert_eq!(c.tick(300, Exact, 5), Action::None);
        assert_eq!(c.tick(400, Up, 5), Action::None);

        assert_eq!(c.tick(1_000, Exact, 5), Action::Start);
        assert_eq!(c.tick(1_400, Exact, 5), Action::Show { latched: false });
        assert_eq!(c.tick(1_500, Extra, 5), Action::Discard);
        assert_eq!(c.tick(1_600, Up, 5), Action::None);
    }

    #[test]
    fn a_key_in_the_same_poll_as_the_chord_still_discards() {
        let mut c = Chord::default();
        assert_eq!(c.tick(0, Up, 3), Action::None);
        // ⌃⌥ and ← both landed between this poll and the last.
        assert_eq!(c.tick(8, Exact, 4), Action::Start);
        assert_eq!(c.tick(16, Exact, 4), Action::Discard);
    }

    #[test]
    fn the_chord_joined_to_another_modifier_never_starts() {
        let mut c = Chord::default();
        assert_eq!(c.tick(0, Extra, 0), Action::None);
        // ⌘ comes up while ⌃⌥ stay down: no fresh press.
        assert_eq!(c.tick(100, Exact, 0), Action::None);
        assert!(!c.is_active());
    }

    #[test]
    fn a_lone_tap_is_discarded_after_the_double_tap_window() {
        let mut c = Chord::default();
        c.tick(0, Exact, 0);
        assert_eq!(c.tick(100, Up, 0), Action::None);
        assert_eq!(c.tick(100 + DOUBLE_TAP_MS - 1, Up, 0), Action::None);
        assert_eq!(c.tick(100 + DOUBLE_TAP_MS, Up, 0), Action::Discard);
        // Typing after a tap is not a double tap either.
        c.tick(1_000, Exact, 0);
        c.tick(1_050, Up, 0);
        assert_eq!(c.tick(1_100, Up, 1), Action::Discard);
    }

    #[test]
    fn a_click_latches_and_the_chord_then_confirms() {
        let mut c = Chord::default();
        assert!(c.latch(0));
        assert!(c.is_latched());
        assert!(!c.latch(10));
        assert_eq!(c.tick(500, Exact, 0), Action::None);
        assert_eq!(c.tick(560, Up, 0), Action::Finish);
        assert!(!c.is_active());
    }

    #[test]
    fn esc_cancels_and_a_forgotten_latch_stops_itself() {
        let mut c = Chord::default();
        c.tick(0, Exact, 0);
        assert_eq!(c.cancel(), Action::Discard);
        assert_eq!(c.cancel(), Action::None);
        assert_eq!(c.tick(50, Up, 0), Action::None);

        c.tick(1_000, Exact, 0);
        c.tick(1_050, Up, 0);
        c.tick(1_200, Exact, 0);
        c.tick(1_250, Up, 0);
        assert_eq!(c.tick(1_000 + MAX_MS - 1, Up, 0), Action::None);
        assert_eq!(c.tick(1_000 + MAX_MS, Up, 0), Action::Finish);
    }
}

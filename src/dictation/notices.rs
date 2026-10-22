//! What the pill tells the user after a dictation, as Wispr Flow does: where the text went ("Copied
//! to the clipboard"), how it was laid out ("Formatted as a list"), and what was learned from an
//! edit ("Added “Klose” to your dictionary"), with Undo for what was learned. Pure, so it is tested
//! without a pill.
//!
//! Notices queue and show one at a time. A notice's clock starts when it first shows, so one
//! queued while the user dictates or records waits until they finish.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// How long a notice stays in the pill once it shows.
pub const NOTICE_FOR: Duration = Duration::from_millis(3_500);
/// Notices past this many waiting are dropped, so a burst never queues a minute of them.
const MAX_WAITING: usize = 6;

/// What a notice's Undo takes back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Undo {
    Word(String),
    /// The layout learned, and the example of the same dictation it replaced.
    Layout(super::layout::Example, Option<super::layout::Example>),
}

#[derive(Debug)]
pub struct Notice {
    pub text: String,
    pub undo: Option<Undo>,
    /// When it goes; `None` until it first shows.
    until: Option<Instant>,
}

#[derive(Default)]
pub struct Notices {
    queue: VecDeque<Notice>,
    /// When the last notice went, so a click meant for it isn't taken for the pill behind it.
    gone: Option<Instant>,
}

impl Notices {
    pub fn push(&mut self, text: impl Into<String>, undo: Option<Undo>) {
        if self.queue.len() < MAX_WAITING {
            self.queue.push_back(Notice { text: text.into(), undo, until: None });
        }
    }

    /// Puts where a dictation went ("Copied to the clipboard") ahead of what earlier edits
    /// taught, past the cap: it is about the text the user is looking for now. A notice it
    /// displaces shows again in full afterwards.
    pub fn push_first(&mut self, text: impl Into<String>) {
        if let Some(front) = self.queue.front_mut() {
            front.until = None;
        }
        self.queue.push_front(Notice { text: text.into(), undo: None, until: None });
    }

    /// The notice the pill shows now, when `visible` (nothing else holds the pill), starting its
    /// clock the first time it shows; an expired one goes first. While hidden, a notice that
    /// already showed and has no Undo goes, since the user moved on from what it reported, and one
    /// with Undo waits to show again in full.
    pub fn current(&mut self, visible: bool, now: Instant) -> Option<&Notice> {
        if self.queue.front().is_some_and(|n| n.until.is_some_and(|until| until <= now)) {
            self.dismiss(now);
        }
        if !visible {
            if self.queue.front().is_some_and(|n| n.until.is_some() && n.undo.is_none()) {
                self.dismiss(now);
            }
            if let Some(front) = self.queue.front_mut() {
                front.until = None;
            }
            return None;
        }
        let front = self.queue.front_mut()?;
        front.until.get_or_insert(now + NOTICE_FOR);
        Some(front)
    }

    /// Whether a notice is on the pill now.
    pub fn showing(&self) -> bool {
        self.queue.front().is_some_and(|n| n.until.is_some())
    }

    /// Takes the notice on the pill off it, returning it.
    pub fn dismiss(&mut self, now: Instant) -> Option<Notice> {
        let notice = self.queue.pop_front();
        if notice.is_some() {
            self.gone = Some(now);
        }
        notice
    }

    pub fn gone(&self) -> Option<Instant> {
        self.gone
    }
}

/// "Added “Klose” to your dictionary".
pub fn added_word(word: &str) -> String {
    format!("Added “{word}” to your dictionary")
}

pub const LEARNED_LAYOUT: &str = "Learned your formatting";

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(notices: &mut Notices, visible: bool, at: Instant) -> Option<String> {
        notices.current(visible, at).map(|n| n.text.clone())
    }

    #[test]
    fn notices_show_one_at_a_time_for_their_full_time() {
        let t0 = Instant::now();
        let mut notices = Notices::default();
        notices.push("one", None);
        notices.push("two", Some(Undo::Word("Klose".into())));
        assert_eq!(texts(&mut notices, true, t0).as_deref(), Some("one"));
        assert!(notices.showing());
        assert_eq!(texts(&mut notices, true, t0 + NOTICE_FOR / 2).as_deref(), Some("one"));
        assert_eq!(texts(&mut notices, true, t0 + NOTICE_FOR).as_deref(), Some("two"));
        let two = t0 + NOTICE_FOR;
        assert_eq!(texts(&mut notices, true, two + NOTICE_FOR - Duration::from_millis(1)).as_deref(), Some("two"));
        assert_eq!(texts(&mut notices, true, two + NOTICE_FOR), None);
        assert_eq!(notices.gone(), Some(two + NOTICE_FOR));
    }

    #[test]
    fn a_notice_waits_while_the_pill_is_busy() {
        let t0 = Instant::now();
        let mut notices = Notices::default();
        notices.push("learned", Some(Undo::Word("Klose".into())));
        assert_eq!(texts(&mut notices, false, t0), None);
        assert!(!notices.showing());
        let later = t0 + 10 * NOTICE_FOR;
        assert_eq!(texts(&mut notices, true, later).as_deref(), Some("learned"), "its clock starts when it shows");
        assert_eq!(texts(&mut notices, false, later + Duration::from_secs(1)), None);
        let back = later + 10 * NOTICE_FOR;
        assert_eq!(texts(&mut notices, true, back).as_deref(), Some("learned"), "hidden partway, it shows again in full");
        assert_eq!(texts(&mut notices, true, back + NOTICE_FOR - Duration::from_millis(1)).as_deref(), Some("learned"));
    }

    #[test]
    fn a_shown_notice_without_undo_goes_when_the_pill_is_taken() {
        let t0 = Instant::now();
        let mut notices = Notices::default();
        notices.push("Copied to the clipboard", None);
        notices.push("next", None);
        texts(&mut notices, true, t0);
        assert_eq!(texts(&mut notices, false, t0 + Duration::from_secs(1)), None);
        assert_eq!(texts(&mut notices, true, t0 + Duration::from_secs(2)).as_deref(), Some("next"));
    }

    #[test]
    fn dismissing_takes_the_shown_notice_and_its_undo() {
        let t0 = Instant::now();
        let mut notices = Notices::default();
        notices.push(LEARNED_LAYOUT, Some(Undo::Word("Nova".into())));
        texts(&mut notices, true, t0);
        assert_eq!(notices.dismiss(t0).and_then(|n| n.undo), Some(Undo::Word("Nova".into())));
        assert!(!notices.showing());
        assert!(notices.dismiss(t0).is_none());
    }

    #[test]
    fn where_the_text_went_jumps_the_queue_and_the_cap() {
        let t0 = Instant::now();
        let mut notices = Notices::default();
        for n in 0..MAX_WAITING {
            notices.push(added_word(&n.to_string()), Some(Undo::Word(n.to_string())));
        }
        texts(&mut notices, true, t0);
        notices.push_first("Copied to the clipboard");
        assert_eq!(texts(&mut notices, true, t0 + Duration::from_secs(1)).as_deref(), Some("Copied to the clipboard"));
        let after = t0 + Duration::from_secs(1) + NOTICE_FOR;
        assert_eq!(texts(&mut notices, true, after).as_deref(), Some("Added “0” to your dictionary"), "the displaced notice comes back");
        assert_eq!(
            texts(&mut notices, true, after + NOTICE_FOR - Duration::from_millis(1)).as_deref(),
            Some("Added “0” to your dictionary"),
            "in full"
        );
    }

    #[test]
    fn a_burst_is_capped() {
        let mut notices = Notices::default();
        for n in 0..20 {
            notices.push(added_word(&n.to_string()), None);
        }
        assert_eq!(notices.queue.len(), MAX_WAITING);
        assert_eq!(notices.queue[0].text, "Added “0” to your dictionary");
    }
}

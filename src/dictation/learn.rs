//! Words learned from the user's edits to text dictation just pasted, as Wispr Flow adds them:
//! "close" changed to "Klose" puts Klose in the dictionary. Pure, so it is tested without a Mac;
//! `session` reads the field over Accessibility.
//!
//! `learned` proposes short swaps that change letters, not just case, with no digits, in a paste
//! that is still mostly as dictated, and never a stretched word ("heyyy"), a vowel-dropped
//! shortcut ("abt") or an email address. `session` keeps only those the system spell checker
//! doesn't know and can't correct (a name such as Klose or Pipedrive), so ordinary editing ("The"
//! to "This", "follow up" to "follow-up", "Jon" to "John") and typos ("calender") are never
//! learned.

/// Text kept on each side of the paste to find it again after edits.
const ANCHOR_CHARS: usize = 40;
/// The longest runs of words in a swap that count as a correction: a spoken term takes more
/// words ("blog dot example labs") than the one it becomes.
const MAX_HEARD_WORDS: usize = 5;
const MAX_TERM_WORDS: usize = 3;
/// Most words learned in one check of the watchers' proposals, after `session`'s spell checks.
pub const MAX_LEARNED: usize = 3;
/// The share of the paste's words still in place, or respelled, for the span to count as the
/// same text; less is a different text (a chat composer's next message after the paste was sent).
const MIN_KEPT: f32 = 0.6;
/// A paste longer than this, or a span far longer than its paste, is not compared.
const MAX_WORDS: usize = 300;
/// A swap is a correction when its spellings are this close: edits over the longer length.
/// Loose, since a misheard name shares sounds more than letters ("close" for "Klose").
const MAX_DISTANCE_RATIO: f32 = 0.6;

/// Where a paste sits in a field: the text around it, to find the span again after edits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Paste {
    /// Where the paste started, to pick the right copy of a prefix that repeats.
    at: usize,
    prefix: String,
    suffix: String,
    pub text: String,
}

/// Finds `pasted` in the field's `value` (its last occurrence, since a paste usually lands at the
/// end of what is there).
pub fn locate(value: &str, pasted: &str) -> Option<Paste> {
    let start = value.rfind(pasted)?;
    let before = &value[..start];
    let after = &value[start + pasted.len()..];
    let prefix_start = before.char_indices().rev().nth(ANCHOR_CHARS - 1).map_or(0, |(i, _)| i);
    let suffix_end = after.char_indices().nth(ANCHOR_CHARS).map_or(after.len(), |(i, _)| i);
    Some(Paste {
        at: start,
        prefix: before[prefix_start..].to_owned(),
        suffix: after[..suffix_end].to_owned(),
        text: pasted.to_owned(),
    })
}

/// The paste's span in the field's later `value`, between its anchors; `None` once the text
/// around it changed too, since the span can't be told apart then.
pub fn edited_span<'a>(paste: &Paste, value: &'a str) -> Option<&'a str> {
    let start = if paste.prefix.is_empty() {
        0
    } else {
        value
            .match_indices(paste.prefix.as_str())
            .map(|(i, p)| i + p.len())
            .min_by_key(|end| end.abs_diff(paste.at))?
    };
    let rest = &value[start..];
    let end = if paste.suffix.is_empty() { rest.len() } else { rest.find(&paste.suffix)? };
    Some(&rest[..end])
}

/// A word the user put in place of what dictation wrote: (what was pasted, what is there now).
pub type Respelling = (String, String);

/// The words the user put in place of words dictation got wrong, for `session` to check against
/// the spell checker.
pub fn learned(pasted: &str, edited: &str) -> Vec<Respelling> {
    let old: Vec<&str> = pasted.split_whitespace().collect();
    let new: Vec<&str> = edited.split_whitespace().collect();
    if old.is_empty() || old.len() > MAX_WORDS || new.len() > 2 * old.len() + MAX_WORDS / 6 {
        return Vec::new();
    }
    let (swaps, mut kept) = swaps(&old, &new);
    let mut out: Vec<Respelling> = Vec::new();
    for (was, now) in swaps {
        // Words after the respelling are text typed after the paste ("Pipedrive today"), so the
        // term is the run of new words that best matches what was heard.
        let best = (1..=now.len().min(was.len()).min(MAX_TERM_WORDS))
            .min_by_key(|&k| crate::dictionary::edit_distance(&squash(&joined(&was)), &squash(&joined(&now[..k]))));
        let sized = (1..=MAX_HEARD_WORDS).contains(&was.len());
        if let Some(term) = best.filter(|_| sized).and_then(|k| correction(&was, &now[..k])) {
            kept += was.len();
            if !out.iter().any(|(_, t)| t.eq_ignore_ascii_case(&term)) {
                out.push((without_possessive(&joined(&was)).to_owned(), term));
            }
        }
    }
    if (kept as f32) < MIN_KEPT * old.len() as f32 {
        return Vec::new();
    }
    out
}

/// The span to learn from, settled over the watch: the field is read once a second, and a span
/// counts only once two reads in a row agree, so a word caught mid-typing (a chat message sent
/// straight after the edit) is never learned.
#[derive(Default)]
pub struct Settle {
    last: Option<String>,
    /// Reads in a row that matched `last`.
    agreeing: usize,
    /// The span vanished (a chat message sent), so the last edit may never have been read.
    gone: bool,
}

/// Reads in a row a span needs when the watch ends with it gone: a word finished and sent within
/// one read of a pause ("Klo", then "Klose" and Enter) was never read whole.
const AGREEING_WHEN_GONE: usize = 3;
/// Reads in a row that let a span teach while the field still has focus: long enough that a pause
/// inside a word ("Pipedr…") rarely reaches it, short enough that the user sees what was learned
/// without leaving the field.
const AGREEING_WHILE_FOCUSED: usize = 4;

impl Settle {
    /// Takes the next read of the span; `false` once it is gone or empty, ending the watch.
    pub fn read(&mut self, span: Option<&str>) -> bool {
        let Some(span) = span.filter(|s| !s.trim().is_empty()) else {
            self.gone = true;
            return false;
        };
        if self.last.as_deref() == Some(span) {
            self.agreeing += 1;
        } else {
            self.last = Some(span.to_owned());
            self.agreeing = 1;
        }
        true
    }

    /// The span while the watch goes on, once it has stayed the same for `AGREEING_WHILE_FOCUSED`
    /// reads.
    pub fn quiet(&self) -> Option<&str> {
        self.last.as_deref().filter(|_| !self.gone && self.agreeing >= AGREEING_WHILE_FOCUSED)
    }

    /// The last span, once reads agree on it: two in a row, or `AGREEING_WHEN_GONE` when the span
    /// vanished.
    pub fn settled(self) -> Option<String> {
        let needed = if self.gone { AGREEING_WHEN_GONE } else { 2 };
        self.last.filter(|_| self.agreeing >= needed)
    }
}

/// Whether the spell checker's correction of a respelled word back to what was pasted still leaves
/// it a name: the user changed its first letter ("close" to "Klose"). A slip that keeps the first
/// letter ("schedule" retyped as "schedyle") is a typo of the pasted word.
pub fn chose_name_over(was: &str, now: &str) -> bool {
    let first = |s: &str| s.chars().find(|c| c.is_alphanumeric()).map(|c| c.to_lowercase().collect::<String>());
    first(was) != first(now)
}

/// The new words, trimmed of sentence punctuation, when they respell the old ones: different
/// letters (or the same letters joined, "pipe drive" to "Pipedrive"), spelled alike, no digits.
fn correction(was: &[&str], now: &[&str]) -> Option<String> {
    let (was, now) = (joined(was), joined(now));
    let now = without_possessive(&now).to_owned();
    if now.is_empty() || was.to_lowercase() == now.to_lowercase() || now.chars().any(|c| c.is_ascii_digit() || "@/".contains(c)) {
        return None;
    }
    let (a, b) = (squash(&was), squash(&now));
    if (a != b && collapse_runs(&a) == collapse_runs(&b)) || vowels_dropped(&a, &b) {
        return None;
    }
    let close = a.chars().count().min(b.chars().count()) >= 3
        && crate::dictionary::edit_distance(&a, &b) as f32
            <= MAX_DISTANCE_RATIO * a.chars().count().max(b.chars().count()) as f32;
    close.then_some(now)
}

/// "close's" → "close", so the spell checker's correction compares with the word itself.
fn without_possessive(word: &str) -> &str {
    ["'s", "’s"].iter().find_map(|s| word.strip_suffix(s)).unwrap_or(word)
}

fn joined(words: &[&str]) -> String {
    words.iter().map(|w| trim(w)).collect::<Vec<_>>().join(" ")
}

/// A word without sentence punctuation, quotes or Markdown wrapping (`` ` ``, `*`, `_`, `~`), so
/// backticking or bolding a word is no change and the markup is never learned.
fn trim(word: &str) -> &str {
    word.trim_matches(|c: char| ",;:!?\"'()[]“”‘’`*_~…–—".contains(c)).trim_end_matches('.')
}

/// "heyyy" → "hey", so a stretched word compares as the word.
fn collapse_runs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if !out.ends_with(c) {
            out.push(c);
        }
    }
    out
}

/// `short` is `long` with only vowels taken out ("abt" from "about", "btwn" from "between").
fn vowels_dropped(long: &str, short: &str) -> bool {
    let mut rest = short.chars().peekable();
    for c in long.chars() {
        if rest.peek() == Some(&c) {
            rest.next();
        } else if !"aeiouy".contains(c) {
            return false;
        }
    }
    rest.peek().is_none() && short.len() < long.len()
}

fn squash(text: &str) -> String {
    text.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

type Swap<'a> = (Vec<&'a str>, Vec<&'a str>);

/// The runs that differ between `old` and `new`, aligned on their longest common subsequence of
/// words (compared without case or punctuation), and how many words that subsequence kept.
fn swaps<'a>(old: &[&'a str], new: &[&'a str]) -> (Vec<Swap<'a>>, usize) {
    let same = |a: &str, b: &str| trim(a).to_lowercase() == trim(b).to_lowercase();
    let (n, m) = (old.len(), new.len());
    let mut lcs = vec![vec![0u16; m + 1]; n + 1];
    for i in (0..n).rev() {
        for j in (0..m).rev() {
            lcs[i][j] = if same(old[i], new[j]) { lcs[i + 1][j + 1] + 1 } else { lcs[i + 1][j].max(lcs[i][j + 1]) };
        }
    }
    let (mut i, mut j) = (0, 0);
    let mut out = Vec::new();
    let (mut was, mut now) = (Vec::new(), Vec::new());
    while i < n || j < m {
        if i < n && j < m && same(old[i], new[j]) {
            if !was.is_empty() || !now.is_empty() {
                out.push((std::mem::take(&mut was), std::mem::take(&mut now)));
            }
            i += 1;
            j += 1;
        } else if j < m && (i == n || lcs[i][j + 1] >= lcs[i + 1][j]) {
            now.push(new[j]);
            j += 1;
        } else {
            was.push(old[i]);
            i += 1;
        }
    }
    if !was.is_empty() || !now.is_empty() {
        out.push((was, now));
    }
    (out, usize::from(lcs[0][0]))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms(pasted: &str, edited: &str) -> Vec<String> {
        learned(pasted, edited).into_iter().map(|(_, now)| now).collect()
    }

    #[test]
    fn a_corrected_name_is_learned() {
        assert_eq!(terms("I met close at the shop.", "I met Klose at the shop."), ["Klose"]);
        assert_eq!(terms("we use pipe drive for it", "we use Pipedrive for it"), ["Pipedrive"]);
        // Words are aligned without case, so only the respelled word is proposed.
        assert_eq!(terms("ask ada kraft today", "ask Ada Craft today"), ["Craft"]);
        assert_eq!(terms("see the blog dot example labs page", "see the blog.examplelabs.com page"), ["blog.examplelabs.com"]);
        assert_eq!(terms("open obs bot", "open Obsbot,"), ["Obsbot"]);
    }

    #[test]
    fn stretches_shortcuts_emails_and_possessives_are_not_proposed_as_written() {
        for (pasted, edited) in [
            ("hey there friend", "heyyy there friend"),
            ("thanks a lot", "thanksss a lot"),
            ("see you all soon", "see yall soon"),
            ("talk about it", "talk abt it"),
            ("in between them", "in btwn them"),
            ("ask ada at work now", "ask ada@work now"),
        ] {
            assert!(terms(pasted, edited).is_empty(), "{pasted:?} -> {edited:?}");
        }
        assert_eq!(terms("ask close's team", "ask Klose's team"), ["Klose"]);
        assert_eq!(learned("ask close's team", "ask Klose's team")[0].0, "close", "the pasted word without its possessive");
        assert_eq!(terms("I met close", "I met Klose…"), ["Klose"]);
        for (pasted, edited) in [
            ("run npm install", "run `npm` install"),
            ("we use pipedrive", "we use **Pipedrive**"),
            ("open the main file", "open the src/main.rs file"),
            ("we use pipe drive", "we use Pipedrive today"),
        ] {
            assert!(terms(pasted, edited).iter().all(|t| !t.contains(['`', '*', '/', ' '])), "{pasted:?} -> {edited:?}");
        }
        assert_eq!(terms("we use pipe drive", "we use Pipedrive today"), ["Pipedrive"]);
        assert_eq!(learned("I met close", "I met Klose"), [("close".to_owned(), "Klose".to_owned())]);
    }

    #[test]
    fn a_new_first_letter_is_a_name_and_a_slip_is_a_typo() {
        assert!(chose_name_over("close", "Klose"));
        assert!(!chose_name_over("schedule", "schedyle"));
        assert!(!chose_name_over("important", "importat"));
        assert!(!chose_name_over("web hook", "webhook"));
    }

    #[test]
    fn only_a_span_read_the_same_twice_is_learned_from() {
        let mut settle = Settle::default();
        for read in ["I met close", "I met Klo", "I met Klose", "I met Klose"] {
            assert!(settle.read(Some(read)));
        }
        // Focus moved on: two agreeing reads are enough.
        assert_eq!(settle.settled().as_deref(), Some("I met Klose"));
        let mut sent_mid_word = Settle::default();
        for read in ["I met close", "I met close", "I met Kl", "I met Kloo"] {
            sent_mid_word.read(Some(read));
        }
        assert!(!sent_mid_word.read(None));
        assert_eq!(sent_mid_word.settled(), None, "the span changed after it settled");
        let mut paused = Settle::default();
        for read in ["we use Pipedr", "we use Pipedr", "we use Pipedrive"] {
            paused.read(Some(read));
        }
        assert_eq!(paused.settled(), None);
        let mut finished_and_sent = Settle::default();
        for read in ["I met close", "I met Klo", "I met Klo"] {
            finished_and_sent.read(Some(read));
        }
        assert!(!finished_and_sent.read(None));
        assert_eq!(finished_and_sent.settled(), None, "the finished word was never read");
        let mut left_alone = Settle::default();
        for read in ["I met Klose", "I met Klose", "I met Klose"] {
            left_alone.read(Some(read));
        }
        left_alone.read(None);
        assert_eq!(left_alone.settled().as_deref(), Some("I met Klose"));
    }

    #[test]
    fn a_span_left_alone_teaches_while_the_field_has_focus() {
        let mut settle = Settle::default();
        for read in ["I met close", "I met Klose", "I met Klose", "I met Klose"] {
            settle.read(Some(read));
            assert_eq!(settle.quiet(), None, "not yet quiet");
        }
        settle.read(Some("I met Klose"));
        assert_eq!(settle.quiet(), Some("I met Klose"));
        settle.read(Some("I met Klose."));
        assert_eq!(settle.quiet(), None, "a new edit starts over");
    }

    #[test]
    fn case_changes_numbers_rewording_and_other_texts_are_not_proposed() {
        for (pasted, edited) in [
            ("the cat sat on it", "a Dog sat on it"),
            ("close the door now", "Close the door now"),
            ("meet at 10 am today", "meet at 10:00 AM today"),
            ("send it now", "send it now. Then Call Morgan about the Pipedrive renewal"),
            ("I think so", "I don't think so at all"),
            ("same text", "same text"),
            ("Lunch at noon works for me", "Can you review the deck tonight"),
            ("- buy milk", "Something else entirely now"),
        ] {
            assert!(terms(pasted, edited).is_empty(), "{pasted:?} -> {edited:?}");
        }
    }

    #[test]
    fn the_paste_is_found_again_after_edits_around_it() {
        let paste = locate("Hi team, I met close today. Bye", "I met close today.").unwrap();
        assert_eq!(edited_span(&paste, "Hi team, I met Klose today. Bye"), Some("I met Klose today."));
        let list = locate("- buy milk", "buy milk").unwrap();
        assert_eq!(edited_span(&list, "- buy milk\n- Buy Milk and eggs"), Some("buy milk\n- Buy Milk and eggs"));
        let at_end = locate("Notes: I met close today.", "I met close today.").unwrap();
        assert_eq!(edited_span(&at_end, "Notes: I met Klose today. More typing"), Some("I met Klose today. More typing"));
        assert_eq!(edited_span(&paste, "Something else entirely"), None);
        assert_eq!(locate("nothing here", "I met close"), None);
    }
}

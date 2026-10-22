//! Paragraphs and lists for a long dictation, as Wispr Flow lays one out. A local model
//! (`formatCommand`, `scripts/scrawl-format.py`) does the layout; this module decides when to ask
//! it, hints where the speaker paused, and checks that it changed only the layout. Pure, so it is
//! tested without the model.
//!
//! The model may add paragraph breaks, list markers, line breaks and punctuation, and drop the
//! "and" or "or" before a list's last item. `same_words` holds it to that: the words in order,
//! ignoring punctuation, list markers and a first letter's case, must match what was dictated, or the dictation is
//! pasted as it was.

use crate::model::WordTiming;

/// Fewer words than this, or fewer sentences than `MIN_SENTENCES`, need no layout.
const MIN_WORDS: usize = 40;
const MIN_SENTENCES: usize = 3;
/// A silence this long between words, at a sentence's end, hints at a new paragraph.
const PARAGRAPH_PAUSE_MS: u64 = 1_200;

/// Whether the text is long enough to lay out; shorter dictations skip the model.
pub fn worth_formatting(text: &str) -> bool {
    text.split_whitespace().count() >= MIN_WORDS && sentence_ends(text).len() >= MIN_SENTENCES
}

/// Closing quotes and brackets that may follow a sentence's end (`He said "stop."`).
const CLOSERS: &[char] = &['"', '\'', ')', ']', '”', '’'];

/// Byte offsets just past each sentence's end: `.`, `!` or `?`, then any closing quotes, then a
/// space or the end, so `design.md` and `1.5` don't count, and neither does a list marker's "1."
/// at a line's start.
fn sentence_ends(text: &str) -> Vec<usize> {
    let list_marker = |i: usize| {
        let line_start = text[..i].rfind('\n').map_or(0, |n| n + 1);
        let before = text[line_start..i].trim_start();
        (1..=3).contains(&before.len()) && before.bytes().all(|b| b.is_ascii_digit())
    };
    text.char_indices()
        .filter(|&(i, c)| matches!(c, '.' | '!' | '?') && !list_marker(i))
        .filter_map(|(i, _)| {
            let rest = &text[i + 1..];
            let closed = rest.trim_start_matches(CLOSERS);
            let end = i + 1 + (rest.len() - closed.len());
            closed.chars().next().is_none_or(char::is_whitespace).then_some(end)
        })
        .collect()
}

/// The sentences (numbered from 1) after which the speaker paused for `PARAGRAPH_PAUSE_MS`,
/// read from the transcriber's word timings. The later passes (the dictionary, filenames) rarely
/// move a sentence end; when one does, a break lands a sentence off, which changes layout only.
pub fn pause_hints(words: &[WordTiming]) -> Vec<usize> {
    let mut sentence = 0;
    let mut hints = Vec::new();
    for (word, next) in words.iter().zip(words.iter().skip(1)) {
        if word.text.trim_end_matches(CLOSERS).ends_with(['.', '!', '?']) {
            sentence += 1;
            if next.t0.saturating_sub(word.t1) >= PARAGRAPH_PAUSE_MS {
                hints.push(sentence);
            }
        }
    }
    hints
}

/// `text` with a paragraph break after each sentence in `pauses` (numbered from 1), for when the
/// model returned the text unchanged: a long pause at a sentence's end is the speaker's. Any line
/// break means the model laid the text out, and may have moved sentence ends ("trip." to
/// "trip:"), so the dictated sentence numbers no longer apply.
pub fn break_at_pauses(text: &str, pauses: &[usize]) -> String {
    if text.contains('\n') {
        return text.to_owned();
    }
    let ends = sentence_ends(text);
    let mut out = String::with_capacity(text.len() + 2 * pauses.len());
    let mut from = 0;
    for &sentence in pauses {
        let Some(&end) = sentence.checked_sub(1).and_then(|i| ends.get(i)) else { continue };
        if end <= from || end >= text.trim_end().len() {
            continue;
        }
        out.push_str(text[from..end].trim_start());
        out.push_str("\n\n");
        from = end;
    }
    out.push_str(text[from..].trim_start());
    out
}

/// What the pill says of a layout: a list, or paragraphs; `None` for text on one line.
pub fn describe(laid_out: &str) -> Option<&'static str> {
    if laid_out.lines().any(|line| strip_list_marker(line).len() < line.trim_start().len()) {
        Some("Formatted as a list")
    } else if laid_out.trim().contains('\n') {
        Some("Split into paragraphs")
    } else {
        None
    }
}

/// Whether `formatted` has the words of `dictated`, in order: compared without the punctuation
/// layout may change around a word (`,` `.` `:` `;`, double quotes, brackets) or the list markers
/// ("1.", "-", "•") that start a line. A word's first letter may change case only where a
/// sentence, line or list item starts on either side, so a name or "I" keeps its capital
/// elsewhere; code in backticks must match exactly. Markdown the model adds (`**`, `_`,
/// backticks, a fence) counts as a change. Characters inside a word (`1.5`, `$50`, `We're`,
/// `C++`), apostrophes, and a word's `?` or `!` must match. A dictated "and" or "or" may be
/// missing only right before a list's last item.
pub fn same_words(dictated: &str, formatted: &str) -> bool {
    if formatted.trim().is_empty() {
        return false;
    }
    let spoken = tokens(dictated);
    let mut laid_out = tokens(formatted).into_iter().peekable();
    for said in &spoken {
        if laid_out.peek().is_some_and(|next| same_word(said, next)) {
            laid_out.next();
        } else if !(matches!(said.word.to_lowercase().as_str(), "and" | "or") && laid_out.peek().is_some_and(|next| next.last_item)) {
            return false;
        }
    }
    laid_out.next().is_none()
}

struct Token {
    /// The word, trimmed of layout punctuation.
    word: String,
    /// It starts a list's last item (an item line not followed by another).
    last_item: bool,
    /// It starts a line or follows a sentence end or a colon, where a capital may come or go.
    starts: bool,
}

/// Equal, or equal but for the case of a first letter that isn't code, where either starts.
fn same_word(said: &Token, laid_out: &Token) -> bool {
    if said.word == laid_out.word {
        return true;
    }
    let (mut a, mut b) = (said.word.chars(), laid_out.word.chars());
    match (a.next(), b.next()) {
        (Some(x), Some(y)) if x != '`' && (said.starts || laid_out.starts) => x.to_lowercase().eq(y.to_lowercase()) && a.eq(b),
        _ => false,
    }
}

fn tokens(text: &str) -> Vec<Token> {
    let lines: Vec<&str> = text.lines().filter(|line| !line.trim().is_empty()).collect();
    let is_item = |line: &str| strip_list_marker(line).len() < line.trim_start().len();
    let mut out = Vec::new();
    for (n, line) in lines.iter().enumerate() {
        let content = strip_list_marker(line);
        let mut last_item = is_item(line) && !lines.get(n + 1).is_some_and(|next| is_item(next));
        let mut starts = true;
        for raw in content.split_whitespace() {
            let word = raw
                .trim_start_matches(|c: char| "\"([“".contains(c))
                .trim_end_matches(|c: char| ",.;:\")]”…".contains(c))
                .to_string();
            if !word.is_empty() {
                out.push(Token { word, last_item, starts });
                last_item = false;
            }
            starts = raw.trim_end_matches(CLOSERS).ends_with(['.', '!', '?', ':']);
        }
    }
    out
}

/// A line without its leading list marker: "1. ", "2) ", "- ", "* ", "• ".
fn strip_list_marker(line: &str) -> &str {
    let line = line.trim_start();
    if let Some(rest) = line.strip_prefix(['-', '*', '•']).filter(|rest| rest.starts_with(' ')) {
        return rest;
    }
    let digits = line.chars().take_while(char::is_ascii_digit).count();
    match line[digits..].strip_prefix(['.', ')']) {
        Some(rest) if digits > 0 && digits <= 3 && rest.starts_with(' ') => rest,
        _ => line,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_layout_is_described_as_a_list_or_paragraphs() {
        assert_eq!(describe("We need:\n\n1. Tents.\n2. Maps."), Some("Formatted as a list"));
        assert_eq!(describe("We need:\n- Tents.\n- Maps."), Some("Formatted as a list"));
        assert_eq!(describe("One topic.\n\nAnother topic."), Some("Split into paragraphs"));
        assert_eq!(describe("All on one line. Still one."), None);
        assert_eq!(describe("Version 1.5 shipped.\n\nThen 2.0."), Some("Split into paragraphs"), "a number is not a marker");
    }

    const LONG: &str = "Here is the plan for the garden this spring. First we clear the old beds and turn \
        the soil. Then we plant the tomatoes along the south fence where they get the most sun. \
        After that we set up the drip lines and a timer. Separately, the shed roof needs new \
        shingles before the rain starts, so I will call the roofer on Monday.";

    fn word(text: &str, t0: u64, t1: u64) -> WordTiming {
        WordTiming { t0, t1, text: text.into() }
    }

    #[test]
    fn only_long_dictations_are_laid_out() {
        assert!(worth_formatting(LONG));
        assert!(!worth_formatting("Send the notes to the team before lunch."));
        assert!(!worth_formatting(&"word ".repeat(60)), "many words but no sentences");
    }

    #[test]
    fn a_layout_that_keeps_every_word_passes() {
        let formatted = "Here is the plan for the garden this spring:\n\n\
            1. First we clear the old beds and turn the soil.\n\
            2. Then we plant the tomatoes along the south fence where they get the most sun.\n\
            3. After that we set up the drip lines and a timer.\n\n\
            Separately, the shed roof needs new shingles before the rain starts, so I will call the roofer on Monday.";
        assert!(same_words(LONG, formatted));
        let bullets = "Bring:\n- tents\n* stoves\n• `trail-map.pdf`";
        assert!(same_words("Bring tents, stoves, `trail-map.pdf`.", bullets));
        assert!(same_words("Buy milk, eggs and bread.", "Buy:\n- Milk\n- Eggs\n- Bread"), "an item may start with a capital");
        assert!(same_words("Here is the plan. First we pack.", "Here is the plan: first we pack."), "a sentence start may lose it");
        let list = "We need a few things:\n\n1. The venue needs a deposit.\n2. Catering wants a headcount.\n3. Someone has to book the bus.";
        assert!(same_words("We need a few things. The venue needs a deposit, catering wants a headcount, and someone has to book the bus.", list));
    }

    #[test]
    fn a_reworded_dropped_or_reordered_word_fails() {
        assert!(!same_words(LONG, &LONG.replace("Then we plant", "Next, we plant")));
        assert!(!same_words(LONG, &LONG.replace(" old", "")));
        assert!(!same_words("We met Monday and Tuesday.", "We met Tuesday and Monday."));
        assert!(!same_words(LONG, ""));
        // A number the speaker said is a word, not a marker, when it isn't at a line's start.
        assert!(!same_words("We need 3 chairs.", "We need chairs."));
        // "and" may go only right before a list item, not from prose.
        assert!(!same_words("Salt and pepper.", "Salt pepper."));
        assert!(!same_words("I can and will go. Bring tents.", "I can will go.\n- Bring tents."));
        let two_dropped = "Pack:\n- tents\n- stoves\n- maps";
        assert!(!same_words("Pack tents and stoves and maps.", two_dropped), "only the last item's conjunction");
        assert!(same_words("Pack tents, stoves and maps.", two_dropped));
        for (dictated, formatted) in [
            ("Take 1.5 mg daily.", "Take 15 mg daily."),
            ("It costs $50 now.", "It costs 50 now."),
            ("We're shipping it.", "Were shipping it."),
            ("Learn C++ first.", "Learn C first."),
            ("Should we ship it?", "Should we ship it."),
            ("Open `snake_case.rs` now.", "Open snake-case.rs now."),
            ("Open `README.md` now.", "Open `readme.md` now."),
            ("Open `Cargo.toml` now.", "Open `cargo.toml` now."),
            ("Ship it to the US team.", "Ship it to the us team."),
            ("Walk the dogs' leads.", "Walk the dogs leads."),
            ("Ask Klose today.", "Ask klose today."),
            ("Then I said no.", "Then i said no."),
            ("The venue needs a deposit.", "The **venue** needs a deposit."),
            ("The venue needs a deposit.", "```\nThe venue needs a deposit.\n```"),
        ] {
            assert!(!same_words(dictated, formatted), "{dictated:?} -> {formatted:?}");
        }
    }

    #[test]
    fn pauses_break_paragraphs_when_the_model_made_none() {
        let text = "One thing here. Another thing. A new topic now. Last line.";
        assert_eq!(break_at_pauses(text, &[2]), "One thing here. Another thing.\n\nA new topic now. Last line.");
        assert_eq!(break_at_pauses(text, &[4, 9]), text, "no break after the last sentence or past the end");
        let laid_out = "Intro:\n\n1. One.\n2. Two.";
        assert_eq!(break_at_pauses(laid_out, &[1]), laid_out);
        let single_newline_list = "Intro one. We need things:\n1. Deposit.\n2. Headcount.\n3. Bus.\nNext topic here.";
        assert_eq!(break_at_pauses(single_newline_list, &[3]), single_newline_list);
        // A list's "1." is no sentence end, so a pause after the intro breaks after it.
        let list = "Intro here. Then:\n1. One thing.\n2. Two things.";
        assert_eq!(sentence_ends(list).len(), 3);
        let quoted = "She said \"stop here.\" Then we left. \"Why?\" he asked.";
        assert_eq!(sentence_ends(quoted).len(), 4);
        assert_eq!(break_at_pauses(quoted, &[1]), "She said \"stop here.\"\n\nThen we left. \"Why?\" he asked.");
        assert!(same_words(text, &break_at_pauses(text, &[1, 3])));
    }

    #[test]
    fn long_pauses_at_sentence_ends_hint_paragraphs() {
        let words = [
            word("Hello", 0, 300),
            word("there.", 300, 600),
            word("Next", 2_200, 2_500),
            word("topic", 2_500, 2_800),
            word("here.", 2_800, 3_000),
            word("More", 3_100, 3_300),
            word("now.", 3_300, 3_500),
        ];
        assert_eq!(pause_hints(&words), [1]);
    }
}

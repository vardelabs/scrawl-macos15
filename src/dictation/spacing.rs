//! How a dictation fits the text around where it is pasted. A second dictation into one field gets
//! a space after the first ("dictation?" then " Also"), as Wispr Flow adds it, and one pasted
//! right after an opening bracket closes it ("Notes (" then "see below.)"). Pure, so it is tested
//! without a Mac; `session` reads the field over Accessibility, and pastes the text unchanged when
//! it can't.

/// Brackets and quotes that open, with their closers: text after one belongs to it ("(Also").
const OPENERS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}'), ('“', '”'), ('<', '>'), ('‘', '’'), ('«', '»')];
/// Openers a dictation closes; `<`, `‘` and `«` are as often a comparison, an apostrophe or a
/// language's own quoting.
const CLOSES: &[char] = &['(', '[', '{', '“'];
/// Quotes that open after a space and close after a word (`say "hi"` then `Also`).
const EITHER_WAY: &[char] = &['"', '\'', '`'];

/// The field around the insertion point: up to two characters `before` it (empty at the field's
/// start), the length of the `selected` text the paste replaces, and the characters `after` the
/// selection (empty at the field's end, `None` when they couldn't be read).
pub struct Around<'a> {
    pub before: &'a str,
    pub selected: usize,
    pub after: Option<&'a str>,
}

/// `text` as it should be pasted at `at`: with a space in front when it lands right after a word
/// or punctuation, and with the closer of an opening bracket it lands right after, unless the
/// field or the text already closes it.
pub fn fit(text: &str, at: &Around) -> String {
    let space = if leading_space(at.before, at.selected, text) { " " } else { "" };
    match closer(at, text) {
        Some(close) => format!("{space}{}{close}", text.trim_end()),
        None => format!("{space}{text}"),
    }
}

fn starts_word(text: &str) -> bool {
    text.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '`')
}

fn leading_space(before: &str, selected: usize, text: &str) -> bool {
    if selected > 0 || !starts_word(text) {
        return false;
    }
    let mut back = before.chars().rev();
    let Some(last) = back.next() else { return false };
    if last.is_whitespace() || OPENERS.iter().any(|&(open, _)| open == last) {
        return false;
    }
    if EITHER_WAY.contains(&last) {
        // A quote at the field's start or after a space or an opener opens.
        return back.next().is_some_and(|c| !c.is_whitespace() && !OPENERS.iter().any(|&(open, _)| open == c));
    }
    true
}

fn closer(at: &Around, text: &str) -> Option<char> {
    let last = at.before.chars().next_back()?;
    let &(open, close) = OPENERS.iter().find(|&&(open, _)| open == last && CLOSES.contains(&open))?;
    // Text right after the caret ("(see below)" with the caret after "(") already belongs to the
    // bracket, so only a caret with nothing after it, or a space, gets a closer.
    let after = at.after?;
    let free = after.chars().next().is_none_or(char::is_whitespace);
    let closed_after = !free || after.trim_start().starts_with(close);
    // The text's own pairs ("see (a) below") leave the field's bracket open.
    let closes_it = text.matches(close).count() > text.matches(open).count();
    (starts_word(text) && !closed_after && !closes_it).then_some(close)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn after(before: &str, text: &str) -> String {
        fit(text, &Around { before, selected: 0, after: Some("") })
    }

    #[test]
    fn a_second_dictation_after_a_word_or_punctuation_gets_a_space() {
        assert_eq!(after("dictation?", "Also the rest"), " Also the rest");
        assert_eq!(after("ok.", "Next"), " Next");
        assert_eq!(after("word", "more"), " more");
        assert_eq!(after("is", "42 items"), " 42 items");
        assert_eq!(after("see", "`README.md` first"), " `README.md` first");
        assert_eq!(after("\"hi\"", "Then"), " Then");
        assert_eq!(after("it's", "fine"), " fine");
        assert_eq!(after("e)", "Then"), " Then");
        assert_eq!(after("👍", "Then"), " Then");
    }

    #[test]
    fn whitespace_openers_selections_and_the_field_start_get_no_space() {
        for before in ["", ". ", "e\n", "\t", "a “", " \"", "\"", "(\"", "a <", "a ‘"] {
            assert!(!after(before, "Hello").starts_with(' '), "{before:?}");
        }
        assert_eq!(fit("replacement", &Around { before: "word", selected: 4, after: Some("") }), "replacement");
    }

    #[test]
    fn an_opening_bracket_is_closed_after_the_text() {
        assert_eq!(after("Notes (", "Also the second part is here."), "Also the second part is here.)");
        assert_eq!(after("see [", "the table "), "the table]");
        assert_eq!(after("a {", "b"), "b}");
        assert_eq!(after("a “", "quoted"), "quoted”");
    }

    #[test]
    fn a_bracket_already_closed_or_unread_is_left_alone() {
        let closed = Around { before: "Notes (", selected: 0, after: Some(") and more") };
        assert_eq!(fit("see below", &closed), "see below");
        let spaced_close = Around { before: "Notes (", selected: 0, after: Some("  )") };
        assert_eq!(fit("see below", &spaced_close), "see below");
        let inside = Around { before: "(", selected: 0, after: Some("see ") };
        assert_eq!(fit("Note this.", &inside), "Note this.", "the caret is inside an existing parenthetical");
        let then_text = Around { before: "Notes (", selected: 0, after: Some(" more text") };
        assert_eq!(fit("see below", &then_text), "see below)");
        let unread = Around { before: "Notes (", selected: 0, after: None };
        assert_eq!(fit("see below", &unread), "see below");
        assert_eq!(after("Notes (", "see (a) below"), "see (a) below)", "only the text's own closer counts");
        assert_eq!(after("Notes (", "see below)"), "see below)");
        assert_eq!(after("a <", "b"), "b");
        assert_eq!(after("it ‘", "s"), "s");
    }

    #[test]
    fn text_that_starts_with_punctuation_or_space_is_left_alone() {
        for text in [", and more", ". Next", " already spaced", ""] {
            assert_eq!(after("word", text), text);
            assert_eq!(after("(", text), text);
        }
    }
}

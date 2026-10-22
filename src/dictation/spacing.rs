//! The space between two dictations pasted into one field, as Wispr Flow adds it: "dictation?"
//! followed by "Also" pastes " Also". Pure, so it is tested without a Mac; `session` reads what
//! sits before the insertion point over Accessibility, and pastes the text unchanged when it can't.

/// Brackets and quotes that open: text after one belongs to it ("(Also").
const OPENERS: &[char] = &['(', '[', '{', '<', '“', '‘', '«'];
/// Quotes that open after a space and close after a word (`say "hi"` then `Also`).
const EITHER_WAY: &[char] = &['"', '\'', '`'];

/// Whether `text` needs a space in front to sit after `before`, the field's text just before the
/// insertion point (empty at the field's start; two characters are enough). `selected` is the
/// length of the selection the paste replaces: replaced text has its own spacing.
pub fn leading_space(before: &str, selected: usize, text: &str) -> bool {
    let starts_word = text.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '`');
    if selected > 0 || !starts_word {
        return false;
    }
    let mut back = before.chars().rev();
    let Some(last) = back.next() else { return false };
    if last.is_whitespace() || OPENERS.contains(&last) {
        return false;
    }
    if EITHER_WAY.contains(&last) {
        // A quote at the field's start or after a space or an opener opens.
        return back.next().is_some_and(|c| !c.is_whitespace() && !OPENERS.contains(&c));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_dictation_after_a_word_or_punctuation_gets_a_space() {
        assert!(leading_space("dictation?", 0, "Also the rest"));
        assert!(leading_space("ok.", 0, "Next"));
        assert!(leading_space("word", 0, "more"));
        assert!(leading_space("is", 0, "42 items"));
        assert!(leading_space("see", 0, "`README.md` first"));
        assert!(leading_space("said \"hi\"", 0, "Then"));
        assert!(leading_space("it's", 0, "fine"));
        assert!(leading_space("(see)", 0, "Then"));
    }

    #[test]
    fn whitespace_openers_selections_and_the_field_start_get_none() {
        assert!(!leading_space("", 0, "Hello"));
        assert!(!leading_space("done. ", 0, "Next"));
        assert!(!leading_space("line\n", 0, "Next"));
        assert!(!leading_space("\t", 0, "Next"));
        assert!(!leading_space("note (", 0, "see below"));
        assert!(!leading_space("a “", 0, "quote"));
        assert!(!leading_space("say \"", 0, "hi"));
        assert!(!leading_space("\"", 0, "hi"));
        assert!(!leading_space("x (\"", 0, "hi"));
        assert!(!leading_space("word", 4, "replacement"));
    }

    #[test]
    fn text_that_starts_with_punctuation_or_space_is_left_alone() {
        assert!(!leading_space("word", 0, ", and more"));
        assert!(!leading_space("word", 0, ". Next"));
        assert!(!leading_space("word", 0, " already spaced"));
        assert!(!leading_space("word", 0, ""));
    }
}

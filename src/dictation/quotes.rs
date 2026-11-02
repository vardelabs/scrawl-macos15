//! Spoken quotation marks: "quote unquote token bait" writes `"token bait"`, and "quote ship it
//! unquote" (or "end quote") writes `"ship it"`. Behind the experiment `spokenQuotes`. Pure, so it
//! is tested without a Mac.

const EDGE_PUNCTUATION: &[char] = &['.', ',', '!', '?', ';', ':', '…', '"', '“', '”'];
const PHRASE_MARKS: &[char] = &['.', ',', '!', '?', ';', ':', '…'];
/// Scare quotes after "quote unquote" run to the phrase's end when it is this close; further off,
/// where they close is a guess, so they hold the next word only.
const SCARE_WORDS: usize = 3;
/// "quote" with its "unquote" further off than this is the everyday word.
const QUOTED_WORDS: usize = 12;
/// "a quote", "the quote", "your quote": the word is a noun.
const NOUN_MARKERS: &[&str] = &[
    "a", "an", "the", "this", "that", "no", "any", "every", "each", "another", "my", "our", "your",
    "his", "her", "its", "their",
];

/// Text without a spoken quotation comes back byte for byte.
pub fn apply(text: &str) -> String {
    let tokens: Vec<&str> = text.split_whitespace().collect();
    let mut out: Vec<String> = Vec::with_capacity(tokens.len());
    let mut changed = false;
    let mut i = 0;
    while i < tokens.len() {
        let quoted = scare_quotes(&tokens, i).or_else(|| quotation(&tokens, i));
        let Some(Quoted { words, resume, after }) = quoted else {
            out.push(tokens[i].to_owned());
            i += 1;
            continue;
        };
        let capital = tokens[i].starts_with(char::is_uppercase);
        out.extend(wrap(&tokens[words], capital, after));
        changed = true;
        i = resume;
    }
    if changed { out.join(" ") } else { text.to_owned() }
}

struct Quoted<'a> {
    /// The tokens inside the marks.
    words: std::ops::Range<usize>,
    /// The first token after the spoken quotation.
    resume: usize,
    /// For a spoken "unquote", the punctuation the transcriber put on it; `None` for scare quotes.
    after: Option<&'a str>,
}

fn key(token: &str) -> String {
    token.trim_matches(EDGE_PUNCTUATION).to_lowercase()
}

fn ends_phrase(token: &str) -> bool {
    token.ends_with(PHRASE_MARKS)
}

fn after_noun_marker(tokens: &[&str], i: usize) -> bool {
    i > 0 && !ends_phrase(tokens[i - 1]) && NOUN_MARKERS.contains(&key(tokens[i - 1]).as_str())
}

/// "quote unquote" (or "quote, unquote") and the words it marks.
fn scare_quotes<'a>(tokens: &[&'a str], i: usize) -> Option<Quoted<'a>> {
    if key(tokens[i]) != "quote" || key(tokens.get(i + 1)?) != "unquote" {
        return None;
    }
    let start = i + 2;
    tokens.get(start)?;
    let to_phrase_end = (start..tokens.len())
        .take(SCARE_WORDS)
        .find(|&j| ends_phrase(tokens[j]) || j + 1 == tokens.len());
    let end = to_phrase_end.map_or(start + 1, |j| j + 1);
    Some(Quoted { words: start..end, resume: end, after: None })
}

/// "quote … unquote" or "quote … end quote" around a few words.
fn quotation<'a>(tokens: &[&'a str], i: usize) -> Option<Quoted<'a>> {
    if key(tokens[i]) != "quote" || after_noun_marker(tokens, i) {
        return None;
    }
    let start = i + 1;
    (start + 1..tokens.len().min(start + QUOTED_WORDS + 1)).find_map(|j| {
        let close = match key(tokens[j]).as_str() {
            "unquote" => Some(j),
            "end" if tokens.get(j + 1).is_some_and(|next| key(next) == "quote") => Some(j + 1),
            _ => None,
        }?;
        let after = tokens[close].trim_start_matches(|c: char| !PHRASE_MARKS.contains(&c));
        Some(Quoted { words: start..j, resume: close + 1, after: Some(after) })
    })
}

/// The words between marks. A full stop or comma closes inside them (`"Hello."`), other marks
/// outside, and the transcriber's pause comma before "unquote" goes.
fn wrap(words: &[&str], capital: bool, after: Option<&str>) -> Vec<String> {
    let mut out: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
    let first = &mut out[0];
    if capital {
        let mut chars = first.chars();
        if let Some(c) = chars.next() {
            *first = c.to_uppercase().chain(chars).collect();
        }
    }
    first.insert(0, '"');
    let last = out.last_mut().expect("a quotation holds a word");
    // Before a spoken "unquote" a comma is the speaker's pause, and a mark on "unquote" itself
    // replaces the one on the word before it ("hello. unquote." is one full stop).
    match after {
        Some("") => last.truncate(last.trim_end_matches(',').len()),
        Some(mark) => {
            last.truncate(last.trim_end_matches(PHRASE_MARKS).len());
            last.push_str(mark);
        }
        None => {}
    }
    let word = last.trim_end_matches(PHRASE_MARKS).len();
    let (inside, outside) = match &last[word..] {
        mark @ ("." | ",") => (mark.to_owned(), String::new()),
        mark => (String::new(), mark.to_owned()),
    };
    *last = format!("{}{inside}\"{outside}", &last[..word]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_without_a_spoken_quotation_comes_back_unchanged() {
        for prose in [
            "Can you send me a quote by Friday?",
            "Quote me on that.",
            "The quote was too high, so we passed.",
            "I'll quote the client and wait for an answer before we schedule anything else.",
            "Get a quote from them unquote",
            "He said \"ship it\" twice.",
        ] {
            assert_eq!(apply(prose), prose);
        }
    }

    #[test]
    fn quote_unquote_quotes_to_the_phrase_end() {
        assert_eq!(
            apply("Does this cost more tokens or sort of quote unquote token bait."),
            "Does this cost more tokens or sort of \"token bait.\""
        );
        assert_eq!(apply("It's quote, unquote, fine."), "It's \"fine.\"");
        assert_eq!(apply("The quote unquote experts disagree"), "The \"experts disagree\"");
        assert_eq!(apply("Is it quote unquote done?"), "Is it \"done\"?");
        assert_eq!(apply("Quote unquote experts, again."), "\"Experts,\" again.");
    }

    #[test]
    fn quote_unquote_far_from_a_phrase_end_quotes_one_word() {
        assert_eq!(
            apply("It's quote unquote fine for most people who try it"),
            "It's \"fine\" for most people who try it"
        );
    }

    #[test]
    fn quote_and_unquote_or_end_quote_wrap_what_is_between() {
        assert_eq!(apply("He said quote ship it unquote and left."), "He said \"ship it\" and left.");
        assert_eq!(apply("He said, quote, ship it, end quote."), "He said, \"ship it.\"");
        assert_eq!(apply("The sign read quote closed unquote."), "The sign read \"closed.\"");
        assert_eq!(apply("quote hello, unquote and goodbye"), "\"hello\" and goodbye");
        assert_eq!(apply("quote hello. unquote."), "\"hello.\"");
        assert_eq!(apply("Is it quote really? unquote?"), "Is it \"really\"?");
    }

    #[test]
    fn a_dangling_quote_unquote_is_left_alone() {
        assert_eq!(apply("That's what I'd call quote unquote"), "That's what I'd call quote unquote");
    }
}

//! Filenames and identifiers in dictated text, as Wispr Flow writes them: "dot md" becomes
//! ".md", a spoken phrase that matches a term on screen ("product's design dot MD" with
//! `PRODUCT-DESIGN.md` in the window) is written as that term, and code goes in backticks.
//! Pure, so it is tested without a Mac; `ax::window_text` gathers the screen text.
//!
//! Prose is the common case, so every rule errs toward leaving words alone: brand names
//! (`iPhone`, `YouTube`) and hyphenated words are never code by themselves.

use std::collections::HashMap;

/// File extensions that make a word a filename.
const EXTENSIONS: &[&str] = &[
    "md", "rs", "swift", "json", "toml", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "sh",
    "txt", "html", "css", "go", "rb", "java", "kt", "c", "h", "cpp", "sql", "csv", "png", "jpg",
    "svg", "pdf", "lock", "env", "plist", "xml", "mov", "mp4", "wav",
];
/// Extensions that are also everyday words or letters ("on the dot go"), so a spoken "dot" never
/// joins them; a filename with one still matches when it is on screen.
const SPOKEN_AMBIGUOUS: &[&str] = &["go", "c", "h", "sh", "env", "lock"];
/// Words a spoken "dot" never joins onto ("the dot", "a dot").
const NOT_A_STEM: &[&str] = &["the", "a", "an"];
/// A spoken phrase must carry this many letters and digits to stand for a screen term, so short
/// phrases ("is a", "set up") stay prose.
const MIN_SPOKEN_KEY: usize = 6;
/// Screen text read for terms; a huge document stops here.
const MAX_SCREEN_CHARS: usize = 200_000;

/// Most screen terms kept; a long document stops adding after this.
const MAX_TERMS: usize = 400;
/// The most spoken words one term can stand for ("scrawl macos fifteen dot md").
const MAX_SPAN_WORDS: usize = 6;

/// Joins a spoken "dot" and a known extension onto the word before: "design dot MD" becomes
/// "design.md".
pub fn spoken_extensions(text: &str) -> String {
    let words: Vec<&str> = text.split(' ').collect();
    let mut out: Vec<String> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let ext = words.get(i + 1).map(|next| split_trailing(next));
        let joins = words[i].eq_ignore_ascii_case("dot")
            && out.last().is_some_and(|prev| {
                prev.ends_with(|c: char| c.is_alphanumeric())
                    && !NOT_A_STEM.contains(&prev.to_lowercase().as_str())
            })
            && ext.is_some_and(|(core, _)| {
                let core = core.to_lowercase();
                EXTENSIONS.contains(&core.as_str()) && !SPOKEN_AMBIGUOUS.contains(&core.as_str())
            });
        match (joins, ext) {
            (true, Some((core, rest))) => {
                let prev = out.last_mut().expect("checked above");
                prev.push('.');
                prev.push_str(&core.to_lowercase());
                prev.push_str(rest);
                i += 2;
            }
            _ => {
                out.push(words[i].to_owned());
                i += 1;
            }
        }
    }
    out.join(" ")
}

/// "md." → ("md", ".").
fn split_trailing(word: &str) -> (&str, &str) {
    let end = word
        .char_indices()
        .find(|(_, c)| !c.is_alphanumeric())
        .map_or(word.len(), |(i, _)| i);
    word.split_at(end)
}

/// The filename-like and identifier-like words in screen text: `PRODUCT-DESIGN.md`,
/// `session.rs`, `snake_case`, `camelCase`, `PascalCase`, and a path's last part.
pub fn terms(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut read = 0;
    for text in texts {
        read += text.len();
        if read > MAX_SCREEN_CHARS {
            break;
        }
        for raw in text.split(|c: char| !(c.is_alphanumeric() || "._-/".contains(c))) {
            let word = raw.trim_matches(|c: char| !c.is_alphanumeric());
            for candidate in [word, word.rsplit('/').next().unwrap_or(word)] {
                if term_like(candidate) && !out.iter().any(|t| t == candidate) {
                    out.push(candidate.to_owned());
                }
            }
            if out.len() >= MAX_TERMS {
                return out;
            }
        }
    }
    out
}

/// Code worth backticks: a filename (`main.rs`), `snake_case`, or `lowerCamel` that starts with
/// a lower-case run of two or more (so `iPhone`, `eBay` and `macOS` are names, not code).
fn code_like(word: &str) -> bool {
    let len = word.chars().count();
    if !(3..=80).contains(&len) || word.contains('/') || !word.chars().all(|c| c.is_alphanumeric() || "._-".contains(c)) {
        return false;
    }
    let has_ext = word
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| stem.chars().any(char::is_alphanumeric) && EXTENSIONS.contains(&ext.to_lowercase().as_str()));
    let snake = word.contains('_') && {
        let parts: Vec<&str> = word.split('_').collect();
        parts.iter().all(|p| p.chars().count() >= 2 && p.chars().all(char::is_alphanumeric))
            && parts.iter().any(|p| p.chars().count() >= 3)
    };
    has_ext || snake || lower_camel(word)
}

fn lower_camel(word: &str) -> bool {
    let chars: Vec<char> = word.chars().collect();
    let lead = chars.iter().take_while(|c| c.is_lowercase()).count();
    lead >= 2
        && chars.iter().all(|c| c.is_alphanumeric())
        && chars.windows(2).skip(lead - 1).any(|w| w[0].is_uppercase() && w[1].is_lowercase())
}

/// A screen word a spoken phrase may stand for: code, or a name whose casing speech loses
/// (`PillModel`, `LinkedIn`, `PRODUCT-DESIGN`). Title Case labels ("Sign-In") and numbered words
/// ("COVID-19") are left out.
fn term_like(word: &str) -> bool {
    if code_like(word) {
        return true;
    }
    let chars: Vec<char> = word.chars().collect();
    let pascal = chars.len() >= 6
        && chars.iter().all(|c| c.is_alphabetic())
        && chars[0].is_uppercase()
        && chars.windows(2).any(|w| w[0].is_lowercase() && w[1].is_uppercase());
    let caps_hyphen = word.contains('-')
        && word.split('-').all(|p| p.chars().count() >= 2 && p.chars().all(|c| c.is_uppercase()));
    pascal || caps_hyphen
}

/// Writes spoken phrases that match a screen term as that term, and puts code-like words in
/// backticks when `code` is on. Text already in backticks is left alone.
pub fn apply(text: &str, terms: &[String], code: bool) -> String {
    let mut tokens = tokenize(text);
    let keys: HashMap<String, &str> = terms.iter().map(|t| (key(t, false), t.as_str())).collect();
    let mut i = 0;
    while i < tokens.len() {
        if let Some((end, term)) = longest_match(&tokens, i, &keys) {
            tokens.splice(i..=end, [term.to_owned()]);
        }
        i += 1;
    }
    let text = tokens.concat();
    if code { backtick_code(&text) } else { text }
}

/// Words, separators that may sit inside a filename (`.`, `-`, `_`, `/`), and everything else
/// one character at a time.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for ch in text.chars() {
        let wordy = ch.is_alphanumeric() || ch == '\'';
        match tokens.last_mut() {
            Some(last) if wordy && last.chars().next().is_some_and(|c| c.is_alphanumeric() || c == '\'') => {
                last.push(ch)
            }
            _ => tokens.push(ch.to_string()),
        }
    }
    tokens
}

fn is_word(token: &str) -> bool {
    token.chars().next().is_some_and(char::is_alphanumeric)
}

fn is_joiner(token: &str) -> bool {
    matches!(token, " " | "." | "-" | "_" | "/")
}

/// The longest run of words from `start` (joined by spaces or filename separators) that a term
/// stands for, as its last token's index and the term.
fn longest_match<'a>(
    tokens: &[String],
    start: usize,
    keys: &HashMap<String, &'a str>,
) -> Option<(usize, &'a str)> {
    if !is_word(&tokens[start]) {
        return None;
    }
    let mut ends = vec![start];
    let mut j = start;
    while ends.len() < MAX_SPAN_WORDS && j + 2 < tokens.len() {
        if !is_joiner(&tokens[j + 1]) || !is_word(&tokens[j + 2]) {
            break;
        }
        j += 2;
        ends.push(j);
    }
    ends.into_iter().rev().find_map(|end| {
        let span: String = tokens[start..=end].concat();
        let spoken: Vec<String> = [false, true]
            .iter()
            .map(|&strip| span.split(|c: char| is_joiner(&c.to_string())).map(|w| key(w, strip)).collect())
            .collect();
        // One spoken word is prose, whatever is on screen ("setup" stays, with `set_up` shown).
        if end == start || spoken[0].chars().count() < MIN_SPOKEN_KEY {
            return None;
        }
        let exact = spoken.iter().find_map(|k| keys.get(k).copied());
        exact.or_else(|| spoken.iter().find_map(|k| near_term(k, keys))).map(|term| (end, term))
    })
}

/// A term one letter off a spoken key of 8 letters or more, with the same first letter and the
/// same digits (a spoken "issue 1235" is not `issue-1234`). An exact key wins elsewhere, so ties
/// here are only between near terms: the first in sort order wins, not whichever the map yields
/// first.
fn near_term<'a>(k: &str, keys: &HashMap<String, &'a str>) -> Option<&'a str> {
    if k.chars().count() < 8 {
        return None;
    }
    let digits = |s: &str| s.chars().filter(char::is_ascii_digit).collect::<String>();
    let spoken_digits = digits(k);
    keys.iter()
        .filter(|(t, _)| {
            t.chars().next() == k.chars().next()
                && digits(t) == spoken_digits
                && crate::dictionary::edit_distance(t, k) <= 1
        })
        .map(|(_, term)| *term)
        .min()
}

/// Lower-case letters and digits only; with `strip`, a possessive "'s" comes off first, since
/// "product's design" is how `PRODUCT-DESIGN` sounds.
fn key(word: &str, strip: bool) -> String {
    let word = if strip {
        word.strip_suffix("'s").or_else(|| word.strip_suffix("s'")).unwrap_or(word)
    } else {
        word
    };
    word.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Wraps each code-like word in backticks, outside text already in backticks.
fn backtick_code(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 8);
    for (i, part) in text.split('`').enumerate() {
        if i > 0 {
            out.push('`');
        }
        if i % 2 == 1 {
            out.push_str(part);
            continue;
        }
        let words: Vec<String> = part
            .split(' ')
            .map(|word| {
                let (lead, core, rest) = split_code_punctuation(word);
                if code_like(core) { format!("{lead}`{core}`{rest}") } else { word.to_owned() }
            })
            .collect();
        out.push_str(&words.join(" "));
    }
    out
}

/// "(design.md)," → ("(", "design.md", "),"): a sentence's punctuation and quotes stay outside
/// the backticks.
fn split_code_punctuation(word: &str) -> (&str, &str, &str) {
    let core = word.trim_start_matches(|c: char| "(\"'“‘[".contains(c));
    let lead = &word[..word.len() - core.len()];
    let end = core.trim_end_matches(|c: char| ",.;:!?)\"'”’]".contains(c)).len();
    let (core, rest) = core.split_at(end);
    (lead, core, rest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn terms_of(text: &str) -> Vec<String> {
        terms(&[text.to_owned()])
    }

    #[test]
    fn a_spoken_dot_joins_a_known_extension() {
        assert_eq!(spoken_extensions("open design dot MD."), "open design.md.");
        assert_eq!(spoken_extensions("see main dot rs and lib dot rs"), "see main.rs and lib.rs");
        // Not an extension, no word before, an article before, or an extension that is a word.
        assert_eq!(spoken_extensions("connect the dot later"), "connect the dot later");
        assert_eq!(spoken_extensions("dot md"), "dot md");
        assert_eq!(spoken_extensions("open the dot md file"), "open the dot md file");
        assert_eq!(spoken_extensions("be there on the dot go home"), "be there on the dot go home");
        assert_eq!(spoken_extensions("polka dot sh and Jay dot go"), "polka dot sh and Jay dot go");
        assert_eq!(spoken_extensions("three dot five"), "three dot five");
    }

    #[test]
    fn screen_text_gives_filenames_and_identifiers_not_prose() {
        let found = terms_of(
            "Read PRODUCT-DESIGN.md and src/dictation/session.rs, then call press_shortcut or \
             tickDictation on PillModel. A well-known, Real-Time, Sign-In plan for COVID-19, \
             is_a, set_up.",
        );
        assert_eq!(
            found,
            ["PRODUCT-DESIGN.md", "session.rs", "press_shortcut", "tickDictation", "PillModel", "set_up"]
        );
    }

    #[test]
    fn kais_spoken_filename_is_written_as_it_is_on_screen() {
        let terms = terms_of("Follow PRODUCT-DESIGN.md for the pill.");
        let spoken = spoken_extensions("Update the product's design dot MD.");
        assert_eq!(apply(&spoken, &terms, false), "Update the PRODUCT-DESIGN.md.");
        assert_eq!(apply(&spoken, &terms, true), "Update the `PRODUCT-DESIGN.md`.");
    }

    #[test]
    fn identifiers_match_their_spoken_words() {
        let terms = terms_of("fn press_shortcut() and tickDictation()");
        assert_eq!(
            apply("call press shortcut then tick dictation", &terms, true),
            "call `press_shortcut` then `tickDictation`"
        );
    }

    #[test]
    fn short_phrases_single_words_and_other_numbers_stay_as_spoken() {
        let terms = terms_of("set_up is_a issue-1234.md frame-07.png PillModel");
        assert_eq!(apply("sign in and set up, this is a test", &terms, true), "sign in and set up, this is a test");
        assert_eq!(apply("run setup", &terms, true), "run setup");
        assert_eq!(apply("fix issue 1235 dot md", &terms, true), "fix issue 1235 dot md");
        assert_eq!(apply("open frame 08.png", &terms, false), "open frame 08.png");
        assert_eq!(apply("the pill model", &terms, true), "the PillModel");
    }

    #[test]
    fn brand_names_and_hyphenated_prose_get_no_backticks() {
        let text = "I got a new iPhone and iPad, watched YouTube on macOS, posted on LinkedIn and \
                    GitHub. Self-driving cars, a 24-hour turnaround, a T-shirt and COVID-19.";
        assert_eq!(apply(text, &[], true), text);
        assert_eq!(apply("call useState", &[], true), "call `useState`");
    }

    #[test]
    fn prose_is_left_alone() {
        let terms = terms_of("session.rs PRODUCT-DESIGN.md");
        let text = "The session went well, and the product design looks right.";
        assert_eq!(apply(text, &terms, true), text);
    }

    #[test]
    fn code_formatting_skips_text_already_in_backticks_and_keeps_punctuation_outside() {
        assert_eq!(apply("edit main.rs, then `lib.rs`.", &[], true), "edit `main.rs`, then `lib.rs`.");
        assert_eq!(apply("edit (main.rs) and \"lib.rs\", now", &[], true), "edit (`main.rs`) and \"`lib.rs`\", now");
        assert_eq!(apply("edit main.rs", &[], false), "edit main.rs");
    }
}

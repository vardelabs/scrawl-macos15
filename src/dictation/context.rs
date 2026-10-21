//! Filenames and identifiers in dictated text, as Wispr Flow writes them: "dot md" becomes
//! ".md", a spoken phrase that matches a term on screen ("product's design dot MD" with
//! `PRODUCT-DESIGN.md` in the window) is written as that term, and code-like words go in
//! backticks. Pure, so it is tested without a Mac; `ax::window_text` gathers the screen text.

use std::collections::HashMap;

/// File extensions a spoken "dot" joins on.
const EXTENSIONS: &[&str] = &[
    "md", "rs", "swift", "json", "toml", "yaml", "yml", "ts", "tsx", "js", "jsx", "py", "sh",
    "txt", "html", "css", "go", "rb", "java", "kt", "c", "h", "cpp", "sql", "csv", "png", "jpg",
    "svg", "pdf", "lock", "env", "plist", "xml", "mov", "mp4", "wav",
];

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
            && out.last().is_some_and(|prev| prev.ends_with(|c: char| c.is_alphanumeric()))
            && ext.is_some_and(|(core, _)| EXTENSIONS.contains(&core.to_lowercase().as_str()));
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
/// `session.rs`, `snake_case`, `camelCase`, and a path's last part.
pub fn terms(texts: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for text in texts {
        for raw in text.split(|c: char| !(c.is_alphanumeric() || "._-/".contains(c))) {
            let word = raw.trim_matches(|c: char| !c.is_alphanumeric());
            for candidate in [word, word.rsplit('/').next().unwrap_or(word)] {
                if code_like(candidate) && !out.iter().any(|t| t == candidate) {
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

/// Code rather than prose: a known extension, an underscore or a hyphen between letters, or a
/// capital after a lower-case letter. Ordinary hyphenated words ("well-known") are kept only
/// with a capital or digit in them, so prose stays prose.
fn code_like(word: &str) -> bool {
    let len = word.chars().count();
    if !(3..=80).contains(&len) || word.contains('/') {
        return false;
    }
    let has_ext = word
        .rsplit_once('.')
        .is_some_and(|(stem, ext)| !stem.is_empty() && EXTENSIONS.contains(&ext.to_lowercase().as_str()));
    let camel = word
        .chars()
        .zip(word.chars().skip(1))
        .any(|(a, b)| a.is_lowercase() && b.is_uppercase());
    let marked_hyphen = word.contains('-')
        && word.chars().any(|c| c.is_uppercase() || c.is_ascii_digit())
        && word.chars().any(char::is_alphabetic);
    has_ext || word.contains('_') || camel || marked_hyphen
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
        // A span of one plain word is prose unless it is a term exactly.
        let exact = spoken.iter().find_map(|k| keys.get(k).copied());
        let near = || (end > start).then(|| spoken.iter().find_map(|k| near_term(k, keys))).flatten();
        exact.or_else(near).map(|term| (end, term))
    })
}

/// A term one letter off a spoken key of 8 letters or more, with the same first letter. An exact
/// key wins elsewhere, so ties here are only between near terms: the first in sort order wins, not
/// whichever the map yields first.
fn near_term<'a>(k: &str, keys: &HashMap<String, &'a str>) -> Option<&'a str> {
    if k.chars().count() < 8 {
        return None;
    }
    keys.iter()
        .filter(|(t, _)| {
            t.chars().next() == k.chars().next() && crate::dictionary::edit_distance(t, k) <= 1
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
                let (core, rest) = split_code_trailing(word);
                if code_like(core) { format!("`{core}`{rest}") } else { word.to_owned() }
            })
            .collect();
        out.push_str(&words.join(" "));
    }
    out
}

/// "design.md," → ("design.md", ","): a sentence's punctuation stays outside the backticks.
fn split_code_trailing(word: &str) -> (&str, &str) {
    let end = word.trim_end_matches(|c: char| ",.;:!?)".contains(c)).len();
    word.split_at(end)
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
        // Not an extension, or no word before: left as spoken.
        assert_eq!(spoken_extensions("connect the dot later"), "connect the dot later");
        assert_eq!(spoken_extensions("dot md"), "dot md");
    }

    #[test]
    fn screen_text_gives_filenames_and_identifiers_not_prose() {
        let found = terms_of(
            "Read PRODUCT-DESIGN.md and src/dictation/session.rs, then call press_shortcut or \
             tickDictation. A well-known, high-level plan.",
        );
        assert_eq!(found, ["PRODUCT-DESIGN.md", "session.rs", "press_shortcut", "tickDictation"]);
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
    fn prose_is_left_alone() {
        let terms = terms_of("session.rs PRODUCT-DESIGN.md");
        let text = "The session went well, and the product design looks right.";
        assert_eq!(apply(text, &terms, true), text);
    }

    #[test]
    fn code_formatting_skips_text_already_in_backticks_and_keeps_punctuation_outside() {
        assert_eq!(apply("edit main.rs, then `lib.rs`.", &[], true), "edit `main.rs`, then `lib.rs`.");
        assert_eq!(apply("edit main.rs", &[], false), "edit main.rs");
    }
}

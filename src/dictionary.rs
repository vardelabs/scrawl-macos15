//! Saved words and phrases that fix dictated text after transcription, since Parakeet takes no
//! vocabulary prompt. `~/.config/scrawl/dictionary.json`; see `docs/dictation.md`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use serde::Deserialize;

#[derive(Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Dictionary {
    pub words: Vec<String>,
    pub replacements: BTreeMap<String, String>,
}

pub fn default_path() -> PathBuf {
    crate::handoff::default_config_path().with_file_name("dictionary.json")
}

impl Dictionary {
    /// A missing file is an empty dictionary; a broken one is logged and empty.
    pub fn load() -> Self {
        let path = default_path();
        let Ok(bytes) = std::fs::read(&path) else {
            return Self::default();
        };
        serde_json::from_slice(&bytes).unwrap_or_else(|error| {
            crate::capture::scrawl_log(format!("{}: {error}", path.display()));
            Self::default()
        })
    }

    pub fn apply(&self, text: &str) -> String {
        let mut tokens = tokenize(text);
        let mut rules: Vec<(Vec<String>, &str)> = self
            .replacements
            .iter()
            .map(|(from, to)| (words_of(from), to.as_str()))
            .chain(self.words.iter().map(|w| (words_of(w), w.as_str())))
            .filter(|(from, _)| !from.is_empty())
            .collect();
        rules.sort_by_key(|(from, _)| std::cmp::Reverse(from.len()));
        for (from, to) in &rules {
            replace_phrase(&mut tokens, from, to, |a, b| a == b);
        }
        for word in &self.words {
            let joined = squash(word);
            replace_phrase(&mut tokens, &[joined.clone()], word, |a, _| a == joined);
            if !word.contains(' ') {
                replace_phrase(&mut tokens, &[joined.clone()], word, |a, b| near(a, b));
            }
        }
        tokens.concat()
    }
}

/// Words and the separators between them, so the text rejoins unchanged where no rule hits.
fn tokenize(text: &str) -> Vec<String> {
    let mut tokens: Vec<String> = Vec::new();
    for ch in text.chars() {
        let wordy = is_word_char(ch);
        match tokens.last_mut() {
            Some(last) if last.chars().next().is_some_and(is_word_char) == wordy => last.push(ch),
            _ => tokens.push(ch.to_string()),
        }
    }
    tokens
}

fn is_word_char(ch: char) -> bool {
    ch.is_alphanumeric() || ch == '\''
}

fn words_of(phrase: &str) -> Vec<String> {
    tokenize(phrase)
        .into_iter()
        .filter(|t| t.chars().next().is_some_and(is_word_char))
        .map(|t| t.to_lowercase())
        .collect()
}

fn squash(phrase: &str) -> String {
    words_of(phrase).concat()
}

/// Replaces each run of word tokens matching `from` (separated only by spaces) with `to`.
fn replace_phrase(
    tokens: &mut Vec<String>,
    from: &[String],
    to: &str,
    matches: impl Fn(&str, &str) -> bool,
) {
    let span = from.len() * 2 - 1;
    let mut i = 0;
    while i + span <= tokens.len() {
        let hit = tokens[i].chars().next().is_some_and(is_word_char)
            && (0..span).all(|k| {
                let token = &tokens[i + k];
                if k % 2 == 1 {
                    token.chars().all(|c| c == ' ')
                } else {
                    matches(&token.to_lowercase(), &from[k / 2])
                }
            });
        if hit {
            tokens.splice(i..i + span, [to.to_owned()]);
        }
        i += 1;
    }
}

/// A near miss of a dictionary word: same first letter, one edit away (two from 9 letters),
/// and only for words of 5 letters or more, where a single edit rarely makes another word.
fn near(heard: &str, word: &str) -> bool {
    let len = word.chars().count();
    if len < 5 || heard == word || heard.chars().next() != word.chars().next() {
        return false;
    }
    let allowed = if len >= 9 { 2 } else { 1 };
    heard.chars().count().abs_diff(len) <= allowed && edit_distance(heard, word) <= allowed
}

fn edit_distance(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut diagonal = row[0];
        row[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let above = row[j + 1];
            row[j + 1] = (above + 1).min(row[j] + 1).min(diagonal + usize::from(ca != *cb));
            diagonal = above;
        }
    }
    row[b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dict(words: &[&str], replacements: &[(&str, &str)]) -> Dictionary {
        Dictionary {
            words: words.iter().map(|w| (*w).to_owned()).collect(),
            replacements: replacements
                .iter()
                .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                .collect(),
        }
    }

    #[test]
    fn replacements_match_whole_words_ignoring_case() {
        let d = dict(&[], &[("corvis", "Corvus"), ("var de labs", "Varde Labs")]);
        assert_eq!(
            d.apply("Ask Corvis about var de labs, not corvisx."),
            "Ask Corvus about Varde Labs, not corvisx."
        );
    }

    #[test]
    fn longer_replacements_win_over_shorter_ones() {
        let d = dict(&[], &[("labs", "Labs!"), ("varde labs", "Varde Labs")]);
        assert_eq!(d.apply("varde labs and labs"), "Varde Labs and Labs!");
    }

    #[test]
    fn words_fix_casing_and_spacing() {
        let d = dict(&["Varde Labs", "macOS"], &[]);
        assert_eq!(
            d.apply("varde labs runs MACOS and vardelabs"),
            "Varde Labs runs macOS and Varde Labs"
        );
    }

    #[test]
    fn near_misses_of_long_words_are_fixed_but_short_words_are_left() {
        let d = dict(&["Parakeet", "Scrawl", "Vega", "Kubernetes"], &[]);
        assert_eq!(d.apply("parakeat and scrawll"), "Parakeet and Scrawl");
        assert_eq!(d.apply("kubernetis"), "Kubernetes");
        // Short words and a different first letter are never guessed.
        assert_eq!(d.apply("vegan barakeet"), "vegan barakeet");
    }

    #[test]
    fn untouched_text_keeps_its_punctuation_and_spacing() {
        let d = dict(&["Scrawl"], &[("foo", "bar")]);
        let text = "Hello,  world! It's fine — really.\nNew line";
        assert_eq!(d.apply(text), text);
    }

    #[test]
    fn a_missing_or_partial_file_reads_as_defaults() {
        let d: Dictionary = serde_json::from_str(r#"{"words": ["Corvus"]}"#).unwrap();
        assert_eq!(d, dict(&["Corvus"], &[]));
    }
}

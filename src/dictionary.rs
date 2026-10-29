//! Saved words and phrases that fix dictated text after transcription, since Parakeet takes no
//! vocabulary prompt. `~/.config/scrawl/dictionary.json`; see `docs/dictation.md`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(default)]
pub struct Dictionary {
    pub words: Vec<String>,
    pub replacements: BTreeMap<String, String>,
    /// Words learned from the user's edits to pasted dictation (`dictation::learn`), also in
    /// `words`; Settings marks them.
    #[serde(rename = "learnedWords", skip_serializing_if = "Vec::is_empty")]
    pub learned: Vec<String>,
    /// Learned words the user removed, never learned again.
    #[serde(rename = "learnedRejected", skip_serializing_if = "Vec::is_empty")]
    pub rejected: Vec<String>,
    /// Keys Scrawl doesn't know (a later version's), kept when Settings saves the file.
    #[serde(flatten)]
    pub other: serde_json::Map<String, serde_json::Value>,
}

/// Why Settings refused an entry edited in place, said under the row.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Refusal {
    Empty,
    /// Another entry already has these words (ignoring case and spacing).
    Taken,
}

impl Refusal {
    pub fn message(self) -> &'static str {
        match self {
            Self::Empty => "An entry needs a word.",
            Self::Taken => "That's already in your dictionary.",
        }
    }
}

/// Settings and the edit watcher each load, change and save the file; holding this keeps one from
/// saving over the other's change.
pub fn edit_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn default_path() -> PathBuf {
    path_beside(&crate::handoff::default_config_path())
}

/// The dictionary next to `config.json`.
pub fn path_beside(config: &Path) -> PathBuf {
    config.with_file_name("dictionary.json")
}

impl Dictionary {
    /// A missing file is an empty dictionary; a broken one is logged and empty.
    pub fn load() -> Self {
        let path = default_path();
        Self::load_from(&path).unwrap_or_else(|error| {
            crate::capture::scrawl_log(format!("{error:#}"));
            Self::default()
        })
    }

    /// A missing file is an empty dictionary; a broken one is an error, so nothing overwrites
    /// it with an empty one.
    pub fn load_from(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => serde_json::from_slice(&bytes)
                .with_context(|| format!("{} isn't a valid dictionary", path.display())),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(error) => Err(error).with_context(|| format!("Could not read {}", path.display())),
        }
    }

    /// Writes a temp file and renames it over `path`, so a crash never leaves half a file. A
    /// symlinked dictionary (dotfiles) is written through to its target, with its permissions.
    pub fn save_to(&self, path: &Path) -> Result<()> {
        let path = link_target(path);
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        static SAVES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let save = SAVES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let temp = path.with_extension(format!("json.{}-{save}.tmp", std::process::id()));
        let mut json = serde_json::to_vec_pretty(self)?;
        json.push(b'\n');
        // Owner-only like config.json, since entries can be private names, from the moment the
        // temp file exists. An existing dictionary keeps the mode its owner gave it.
        let _ = std::fs::remove_file(&temp);
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        std::io::Write::write_all(&mut options.open(&temp)?, &json)?;
        if let Ok(meta) = std::fs::metadata(&path) {
            std::fs::set_permissions(&temp, meta.permissions())?;
        }
        std::fs::rename(&temp, &path).with_context(|| format!("Could not save {}", path.display())).inspect_err(|_| {
            let _ = std::fs::remove_file(&temp);
        })
    }

    /// Adds a word or phrase unless it's empty or already there (ignoring case and spacing).
    pub fn add_word(&mut self, word: &str) -> bool {
        let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if words_of(&word).is_empty() || self.words.iter().any(|w| same(w, &word)) {
            return false;
        }
        self.words.push(word);
        true
    }

    pub fn remove_word(&mut self, word: &str) {
        self.words.retain(|w| w != word);
        if self.learned.iter().any(|w| w == word) {
            self.learned.retain(|w| w != word);
            self.rejected.push(word.to_owned());
        }
    }

    /// Rewrites the word `was` where it stands. A learned word stays marked only while its text is
    /// unchanged; it isn't recorded as rejected, since the user corrected it rather than removed it.
    /// A `was` that went meanwhile (Undo in the pill) is added as new.
    pub fn edit_word(&mut self, was: &str, word: &str) -> Result<(), Refusal> {
        let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
        if words_of(&word).is_empty() {
            return Err(Refusal::Empty);
        }
        if self.words.iter().any(|w| w != was && same(w, &word)) {
            return Err(Refusal::Taken);
        }
        match self.words.iter_mut().find(|w| *w == was) {
            Some(slot) if *slot == word => {}
            Some(slot) => {
                *slot = word;
                self.learned.retain(|w| w != was);
            }
            None => self.words.push(word),
        }
        Ok(())
    }

    /// Adds a word learned from an edit, unless the dictionary already has it or the user
    /// removed it after it was learned.
    pub fn learn_word(&mut self, word: &str) -> bool {
        if self.rejected.iter().any(|w| same(w, word)) {
            return false;
        }
        let added = self.add_word(word);
        if added && let Some(word) = self.words.last().cloned()
            && !self.learned.contains(&word)
        {
            self.learned.push(word);
        }
        added
    }

    /// Writes `write` wherever `heard` is heard, replacing an entry for the same heard words.
    pub fn add_replacement(&mut self, heard: &str, write: &str) -> bool {
        let (heard, write) = (heard.trim(), write.trim());
        if words_of(heard).is_empty() || write.is_empty() {
            return false;
        }
        self.replacements.retain(|h, _| !same(h, heard));
        self.replacements.insert(heard.to_owned(), write.to_owned());
        true
    }

    /// Rewrites the replacement for `was` where it stands. Unlike `add_replacement`, words heard
    /// by another entry are refused rather than taking it over.
    pub fn edit_replacement(&mut self, was: &str, heard: &str, write: &str) -> Result<(), Refusal> {
        let (heard, write) = (heard.trim(), write.trim());
        if words_of(heard).is_empty() || write.is_empty() {
            return Err(Refusal::Empty);
        }
        if self.replacements.keys().any(|h| h != was && same(h, heard)) {
            return Err(Refusal::Taken);
        }
        self.replacements.remove(was);
        self.replacements.insert(heard.to_owned(), write.to_owned());
        Ok(())
    }

    pub fn remove_replacement(&mut self, heard: &str) {
        self.replacements.remove(heard);
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

/// Where a dictionary that may be a symlink really lives, even when its target doesn't exist
/// yet (a dotfiles link made before the file).
fn link_target(path: &Path) -> PathBuf {
    if let Ok(real) = std::fs::canonicalize(path) {
        return real;
    }
    match std::fs::read_link(path) {
        Ok(target) if target.is_absolute() => target,
        Ok(target) => path.parent().map_or(target.clone(), |dir| dir.join(&target)),
        Err(_) => path.to_owned(),
    }
}

fn same(a: &str, b: &str) -> bool {
    words_of(a) == words_of(b)
}

/// Replaces each run of word tokens matching `from` with `to`. The words may be separated by
/// spaces or by one hyphen, since a transcriber writes a word it doesn't know as pieces it does
/// ("Kubernetes" as "Cuber-netes").
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
                    word_break(token)
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

/// Spaces with at most one hyphen among them: "cuber netes", "cuber-netes", "cuber- netes".
fn word_break(separator: &str) -> bool {
    separator.chars().all(|c| c == ' ' || c == '-') && separator.matches('-').count() <= 1
}

/// A near miss of a dictionary word: same first letter, one edit away (two from 10 letters),
/// and only for words of 7 letters or more. Shorter words have real neighbours one edit away
/// ("ferries" and "Ferris", "sprawl" and "Scrawl"); those need a replacement. The word with an
/// ending added ("parakeets") is that word, not a miss of it.
fn near(heard: &str, word: &str) -> bool {
    let len = word.chars().count();
    let inflected = heard
        .strip_prefix(word)
        .is_some_and(|ending| ["s", "es", "ed", "er", "ing"].contains(&ending));
    if len < 7 || inflected || heard.chars().next() != word.chars().next() {
        return false;
    }
    let allowed = if len >= 10 { 2 } else { 1 };
    heard.chars().count().abs_diff(len) <= allowed && edit_distance(heard, word) <= allowed
}

pub(crate) fn edit_distance(a: &str, b: &str) -> usize {
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
            ..Dictionary::default()
        }
    }

    #[test]
    fn learned_words_are_marked_saved_and_removed_with_their_word() {
        let mut d = Dictionary::default();
        assert!(d.learn_word("Klose"));
        assert!(!d.learn_word("klose"));
        assert_eq!((d.words.clone(), d.learned.clone()), (vec!["Klose".to_owned()], vec!["Klose".to_owned()]));
        let json = serde_json::to_string(&d).unwrap();
        assert!(json.contains("\"learnedWords\":[\"Klose\"]"));
        assert!(!serde_json::to_string(&Dictionary::default()).unwrap().contains("learnedWords"));
        d.remove_word("Klose");
        assert!(d.words.is_empty() && d.learned.is_empty());
        assert!(!d.learn_word("Klose"), "a removed learned word is not learned again");
        assert!(d.add_word("Klose"), "but can still be added by hand");
    }

    #[test]
    fn replacements_match_whole_words_ignoring_case() {
        let d = dict(&[], &[("feris", "Ferris"), ("var de labs", "Varde Labs")]);
        assert_eq!(
            d.apply("Ask Feris about var de labs, not ferisx."),
            "Ask Ferris about Varde Labs, not ferisx."
        );
    }

    #[test]
    fn a_replacement_matches_words_the_transcriber_hyphenated() {
        let d = dict(&[], &[("cuber netes", "Kubernetes")]);
        assert_eq!(d.apply("Cuber-netes, cuber-netes, cuber- netes."), "Kubernetes, Kubernetes, Kubernetes.");
        // Two hyphens, or other punctuation between the words, is not one spoken word split.
        assert_eq!(d.apply("cuber--netes, cuber, netes"), "cuber--netes, cuber, netes");
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
        let d = dict(&["Parakeet", "Scrawl", "Ferris", "Nova", "Kubernetes"], &[]);
        assert_eq!(d.apply("parakeat"), "Parakeet");
        assert_eq!(d.apply("kubernetis"), "Kubernetes");
        // Real words next to short dictionary words, and a different first letter, stay.
        assert_eq!(d.apply("ferries sprawl novas barakeet"), "ferries sprawl novas barakeet");
        assert_eq!(d.apply("two parakeets, scrawled"), "two parakeets, scrawled");
    }

    #[test]
    fn untouched_text_keeps_its_punctuation_and_spacing() {
        let d = dict(&["Scrawl"], &[("foo", "bar")]);
        let text = "Hello,  world! It's fine — really.\nNew line";
        assert_eq!(d.apply(text), text);
    }

    #[test]
    fn words_and_replacements_are_added_once_and_removed() {
        let mut d = Dictionary::default();
        assert!(d.add_word("  Varde   Labs "));
        assert!(!d.add_word("varde labs"));
        assert!(!d.add_word(" , "));
        assert_eq!(d.words, ["Varde Labs"]);
        d.remove_word("Varde Labs");
        assert!(d.words.is_empty());

        assert!(d.add_replacement("feris", "Ferris"));
        assert!(d.add_replacement("Feris", "Ferris!"));
        assert!(!d.add_replacement("heard", " "));
        assert_eq!(d.replacements.len(), 1);
        assert_eq!(d.apply("feris"), "Ferris!");
        d.remove_replacement("Feris");
        assert!(d.replacements.is_empty());
    }

    #[test]
    fn words_are_edited_in_place_and_refused_when_empty_or_taken() {
        let mut d = dict(&["Atlas", "Nova", "Orbit"], &[]);
        d.learned = vec!["Nova".into()];
        assert_eq!(d.edit_word("Atlas", "  Atlas   Prime "), Ok(()));
        assert_eq!(d.words, ["Atlas Prime", "Nova", "Orbit"], "it keeps its place");
        assert_eq!(d.edit_word("Orbit", " , "), Err(Refusal::Empty));
        assert_eq!(d.edit_word("Orbit", "nova"), Err(Refusal::Taken));
        assert_eq!(d.edit_word("Orbit", "ORBIT"), Ok(()), "its own words, recased, aren't taken");
        assert_eq!(d.words, ["Atlas Prime", "Nova", "ORBIT"]);

        assert_eq!(d.edit_word("Nova", "Nova"), Ok(()));
        assert_eq!(d.learned, ["Nova"], "unchanged, a learned word stays marked");
        assert_eq!(d.edit_word("Nova", "Novak"), Ok(()));
        assert!(d.learned.is_empty(), "changed, it is the user's word");
        assert!(d.rejected.is_empty(), "and the old spelling isn't rejected");

        assert_eq!(d.edit_word("Gone", "Comet"), Ok(()));
        assert_eq!(d.words.last().map(String::as_str), Some("Comet"), "a word removed meanwhile is added");
    }

    #[test]
    fn replacements_are_edited_in_place_and_refused_when_empty_or_taken() {
        let mut d = dict(&[], &[("atlis", "Atlas"), ("no va", "Nova")]);
        assert_eq!(d.edit_replacement("atlis", " atless ", " Atlas! "), Ok(()));
        assert_eq!(d.replacements.get("atless").map(String::as_str), Some("Atlas!"));
        assert!(!d.replacements.contains_key("atlis"));
        assert_eq!(d.edit_replacement("atless", "atless", " "), Err(Refusal::Empty));
        assert_eq!(d.edit_replacement("atless", "  ", "Atlas"), Err(Refusal::Empty));
        assert_eq!(d.edit_replacement("atless", "No  Va", "Atlas"), Err(Refusal::Taken));
        assert_eq!(d.edit_replacement("no va", "No Va", "Nova"), Ok(()), "its own words aren't taken");
        assert_eq!(d.replacements.len(), 2);
    }

    #[test]
    fn saving_keeps_unknown_keys_and_a_broken_file_is_an_error() {
        let dir = std::env::temp_dir().join(format!("scrawl-dictionary-test-{}", std::process::id()));
        let path = dir.join("dictionary.json");
        assert_eq!(Dictionary::load_from(&path).unwrap(), Dictionary::default());
        Dictionary::default().save_to(&path).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }

        std::fs::write(&path, r#"{"words": ["Ferris"], "later": {"feris": 1}}"#).unwrap();
        let mut d = Dictionary::load_from(&path).unwrap();
        d.add_word("Nova");
        d.save_to(&path).unwrap();
        let saved: serde_json::Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(saved["words"], serde_json::json!(["Ferris", "Nova"]));
        assert_eq!(saved["later"], serde_json::json!({"feris": 1}));

        #[cfg(unix)]
        {
            let real = dir.join("dotfiles.json");
            std::fs::rename(&path, &real).unwrap();
            std::os::unix::fs::symlink(&real, &path).unwrap();
            let mut d = Dictionary::load_from(&path).unwrap();
            d.add_word("Atlas");
            d.save_to(&path).unwrap();
            assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert_eq!(Dictionary::load_from(&real).unwrap().words, ["Ferris", "Nova", "Atlas"]);
            std::fs::remove_file(&path).unwrap();

            // A link made before its target: saving creates the target and keeps the link.
            let later = dir.join("later.json");
            std::os::unix::fs::symlink(&later, &path).unwrap();
            Dictionary::default().save_to(&path).unwrap();
            assert!(std::fs::symlink_metadata(&path).unwrap().file_type().is_symlink());
            assert!(later.exists());
            std::fs::remove_file(&path).unwrap();
        }

        std::fs::write(&path, "{ not json").unwrap();
        assert!(Dictionary::load_from(&path).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_missing_or_partial_file_reads_as_defaults() {
        let d: Dictionary = serde_json::from_str(r#"{"words": ["Ferris"]}"#).unwrap();
        assert_eq!(d, dict(&["Ferris"], &[]));
    }
}

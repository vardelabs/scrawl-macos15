//! Layouts learned from the user's edits to text the formatter laid out: a paste whose words the
//! user kept but whose line breaks, list markers or punctuation they changed becomes an example
//! (what was dictated, the user's layout) that `scripts/scrawl-format.py` shows the model, so its
//! layout drifts toward the user's. Pure but for the file, so it is tested without a Mac or model.
//!
//! `~/.config/scrawl/format-examples.json`, beside the dictionary:
//! `{"examples": [{"dictated": "…", "laidOut": "…"}]}`, oldest first.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

use super::learn::Respelling;

/// Examples the formatter is shown: enough to carry a habit, few enough to prefill while the user
/// talks.
pub const MAX_EXAMPLES: usize = 6;
/// A longer dictation is not kept, so one long message can't crowd out the rest.
const MAX_EXAMPLE_WORDS: usize = 250;
/// All examples together, both sides, stay under this (about 1,500 tokens of prompt).
const MAX_TOTAL_CHARS: usize = 6_000;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Example {
    /// The text as it went to the formatter, with the user's respellings applied.
    pub dictated: String,
    /// The user's layout of it.
    #[serde(rename = "laidOut")]
    pub laid_out: String,
}

#[derive(Default, Deserialize, Serialize)]
struct File {
    #[serde(default)]
    examples: Vec<Example>,
}

/// The example to keep from an edit to a paste the formatter was asked to lay out: `dictated` went
/// to the formatter, `pasted` is what was pasted, `edited` the settled span after the user's
/// edits, and `respellings` the words `learn::learned` found respelled. Only a change of layout
/// counts: once those respellings are applied, `edited` must differ from `pasted` and keep the
/// words of `dictated` exactly as `format::same_words` holds the formatter to them.
pub fn example(dictated: &str, pasted: &str, edited: &str, respellings: &[Respelling]) -> Option<Example> {
    let fixes = crate::dictionary::Dictionary {
        replacements: respellings.iter().cloned().collect(),
        ..Default::default()
    };
    let (dictated, pasted) = (fixes.apply(dictated), fixes.apply(pasted));
    let edited = edited.trim();
    let small = dictated.split_whitespace().count() <= MAX_EXAMPLE_WORDS;
    let relaid = layout(&pasted) != layout(edited);
    (small && relaid && super::format::same_words(&dictated, edited))
        .then(|| Example { dictated, laid_out: edited.to_owned() })
}

/// The text without trailing spaces on its lines or around it, which no one sees.
fn layout(text: &str) -> String {
    text.trim().lines().map(str::trim_end).collect::<Vec<_>>().join("\n")
}

/// `examples` with `new` added last, replacing an example of the same dictation, then the oldest
/// dropped until at most `MAX_EXAMPLES` remain within `MAX_TOTAL_CHARS`.
pub fn remember(mut examples: Vec<Example>, new: Example) -> Vec<Example> {
    examples.retain(|e| e.dictated != new.dictated);
    examples.push(new);
    let size = |examples: &[Example]| examples.iter().map(|e| e.dictated.len() + e.laid_out.len()).sum::<usize>();
    while examples.len() > MAX_EXAMPLES || (examples.len() > 1 && size(&examples) > MAX_TOTAL_CHARS) {
        examples.remove(0);
    }
    examples
}

/// `examples` without `learned`, and with `replaced`, the example of the same dictation it took
/// the place of, back. Once `learned` is gone (a later layout of the same dictation, or Forget),
/// nothing changes: bringing `replaced` back then would undo something else.
pub fn unlearn(mut examples: Vec<Example>, learned: &Example, replaced: Option<Example>) -> Vec<Example> {
    let before = examples.len();
    examples.retain(|e| e != learned);
    match replaced {
        Some(replaced) if examples.len() < before => remember(examples, replaced),
        _ => examples,
    }
}

pub fn default_path() -> PathBuf {
    path_beside(&crate::handoff::default_config_path())
}

/// The examples next to `config.json`.
pub fn path_beside(config: &Path) -> PathBuf {
    config.with_file_name("format-examples.json")
}

static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Bumped by each Forget. A layout learned from a paste made before a Forget carries an older
/// generation and is never saved, since a watch outlives the click by up to a minute.
pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

/// Saving and forgetting each hold this, so the two file changes never interleave.
pub fn edit_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// The saved examples; a missing or broken file is none, and the next save replaces it.
pub fn load_from(path: &Path) -> Vec<Example> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
        .map(|file| file.examples)
        .unwrap_or_default()
}

/// Writes a temp file and renames it over `path`, owner-only, since examples are the user's text.
pub fn save_to(path: &Path, examples: Vec<Example>) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut json = serde_json::to_vec_pretty(&File { examples })?;
    json.push(b'\n');
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    std::io::Write::write_all(&mut options.open(&temp)?, &json)?;
    std::fs::rename(&temp, path).with_context(|| format!("Could not save {}", path.display())).inspect_err(|_| {
        let _ = std::fs::remove_file(&temp);
    })
}

/// Deletes the examples and bumps `generation`; none saved is no error.
pub fn forget(path: &Path) -> Result<()> {
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
            Err(error).with_context(|| format!("Could not remove {}", path.display()))
        }
        _ => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DICTATED: &str = "We need three things. First the tent. Second the stove. Third the maps.";
    const PASTED: &str = "We need three things:\n\n1. First the tent.\n2. Second the stove.\n3. Third the maps.";

    fn example_of(edited: &str) -> Option<String> {
        example(DICTATED, PASTED, edited, &[]).map(|e| e.laid_out)
    }

    #[test]
    fn a_new_layout_of_the_same_words_is_kept() {
        let bullets = "We need three things:\n- First the tent.\n- Second the stove.\n- Third the maps.";
        assert_eq!(example_of(bullets).as_deref(), Some(bullets));
        let paragraphs = "We need three things.\n\nFirst the tent. Second the stove. Third the maps.";
        assert_eq!(example_of(&format!("{paragraphs}\n")).as_deref(), Some(paragraphs));
        let kept = example(DICTATED, PASTED, bullets, &[]).unwrap();
        assert_eq!(kept.dictated, DICTATED, "the formatter's input, not its output");
    }

    #[test]
    fn an_unchanged_paste_or_changed_words_are_not_kept() {
        assert_eq!(example_of(PASTED), None);
        assert_eq!(example_of(&format!("  {}  \n", PASTED.replace('\n', "  \n"))), None, "only invisible space changed");
        assert_eq!(example_of("We need four things:\n- First the tent.\n- Second the stove.\n- Third the maps."), None);
        assert_eq!(example_of("We need three things:\n- First the tent.\n- Third the maps."), None);
        assert_eq!(example_of("We need three things:\n- First the tent.\n- Second the stove.\n- Third the maps.\n- Fourth"), None);
        assert_eq!(example_of(""), None, "a cleared span");
    }

    #[test]
    fn a_respelled_name_rides_along_with_a_new_layout() {
        let dictated = "Ask close about it. Then book the room. Then send the notes.";
        let pasted = "Ask close about it. Then book the room. Then send the notes.";
        let edited = "Ask Klose about it.\n\nThen book the room. Then send the notes.";
        let respelled = [("close".to_owned(), "Klose".to_owned())];
        assert_eq!(example(dictated, pasted, edited, &[]), None, "a changed word the word learning didn't find");
        let kept = example(dictated, pasted, edited, &respelled).unwrap();
        assert_eq!(kept.dictated, "Ask Klose about it. Then book the room. Then send the notes.");
        assert_eq!(kept.laid_out, edited);
        assert_eq!(example(dictated, pasted, "Ask Klose about it. Then book the room. Then send the notes.", &respelled), None);
    }

    #[test]
    fn a_long_dictation_is_not_kept() {
        let long = "One more sentence here. ".repeat(70);
        assert_eq!(example(&long, &long, &long.replace(". ", ".\n"), &[]), None);
    }

    fn numbered(n: usize, size: usize) -> Example {
        Example { dictated: format!("{n} {}", "x".repeat(size)), laid_out: format!("{n}\n{}", "x".repeat(size)) }
    }

    #[test]
    fn the_newest_examples_are_kept_within_the_budget() {
        let mut examples = Vec::new();
        for n in 0..10 {
            examples = remember(examples, numbered(n, 10));
        }
        let kept: Vec<String> = examples.iter().map(|e| e.dictated[..1].to_owned()).collect();
        assert_eq!(kept, ["4", "5", "6", "7", "8", "9"]);
        let again = remember(examples.clone(), Example { laid_out: "new".into(), ..examples[0].clone() });
        assert_eq!(again.len(), MAX_EXAMPLES);
        assert_eq!(again.last().unwrap().laid_out, "new", "a dictation laid out again replaces its example");
        let big = remember(examples, numbered(0, 2_950));
        assert!(big.len() < MAX_EXAMPLES && big.iter().map(|e| e.dictated.len() + e.laid_out.len()).sum::<usize>() <= MAX_TOTAL_CHARS);
        assert_eq!(remember(Vec::new(), numbered(1, 10_000)).len(), 1, "the newest stays, even alone over budget");
    }

    #[test]
    fn undoing_a_layout_puts_back_the_one_it_replaced() {
        let old = Example { dictated: "One. Two.".into(), laid_out: "One.\n\nTwo.".into() };
        let new = Example { laid_out: "- One.\n- Two.".into(), ..old.clone() };
        let other = numbered(1, 3);
        let learned = remember(vec![other.clone(), old.clone()], new.clone());
        assert_eq!(learned, [other.clone(), new.clone()]);
        assert_eq!(unlearn(learned.clone(), &new, Some(old.clone())), [other.clone(), old.clone()]);
        assert_eq!(unlearn(learned.clone(), &new, None), [other.clone()]);
        let newer = Example { laid_out: "1. One.\n2. Two.".into(), ..new.clone() };
        let relearned = remember(learned, newer.clone());
        assert_eq!(unlearn(relearned.clone(), &new, Some(old.clone())), relearned, "a later layout took its place");
        assert!(unlearn(Vec::new(), &new, Some(old)).is_empty(), "forgotten");
    }

    #[test]
    fn examples_save_load_and_forget_beside_the_config() {
        let dir = std::env::temp_dir().join(format!("scrawl-layout-test-{}", std::process::id()));
        let path = path_beside(&dir.join("config.json"));
        assert_eq!(path.file_name().unwrap(), "format-examples.json");
        assert!(load_from(&path).is_empty());
        save_to(&path, vec![numbered(1, 3)]).unwrap();
        assert_eq!(load_from(&path), [numbered(1, 3)]);
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());
        let before = generation();
        forget(&path).unwrap();
        forget(&path).unwrap();
        assert!(generation() >= before + 2, "each Forget starts a new generation");
        assert!(!path.exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}

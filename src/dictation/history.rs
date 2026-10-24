//! Recent dictations, as pasted, so one is never lost to a clipboard that something else
//! overwrote: the pill's Copy button copies the last, and the menu bar's Recent Dictations lists
//! them. The last stays in memory for the button; with **Keep recent dictations**
//! (`dictationHistory`, default on) the newest `MAX_ENTRIES` are also kept on disk, owner-only, in
//! `~/Library/Application Support/scrawl/dictation-history.json`, so a quit or crash keeps them,
//! and the last dictation's audio is kept beside it in `last-dictation.wav`, so a transcript that
//! lost words can be checked against what was said. Clear deletes both and forgets the last.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub const MAX_ENTRIES: usize = 20;
/// How many the menu lists.
pub const MENU_ENTRIES: usize = 10;
/// The longest first line a menu item shows.
const LABEL_CHARS: usize = 48;

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
pub struct Entry {
    /// When it was pasted, in epoch milliseconds; also the entry's id in the menu.
    #[serde(rename = "atMs")]
    pub at_ms: u64,
    pub text: String,
}

#[derive(Default, Deserialize, Serialize)]
struct File {
    #[serde(default)]
    entries: Vec<Entry>,
}

/// The last dictation, kept in memory so the pill can copy it with history off too.
static LAST: Mutex<Option<String>> = Mutex::new(None);
/// Saving and clearing each hold this, so the two never interleave.
static EDIT: Mutex<()> = Mutex::new(());
/// Bumped by each Clear. A dictation finished before a Clear and recorded after it carries an
/// older generation and is never kept, since Clear means forget what was said.
static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub fn generation() -> u64 {
    GENERATION.load(std::sync::atomic::Ordering::Acquire)
}

fn edit_lock() -> std::sync::MutexGuard<'static, ()> {
    EDIT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// `entries` with `entry` added newest last, keeping the newest `MAX_ENTRIES`.
pub fn remember(mut entries: Vec<Entry>, entry: Entry) -> Vec<Entry> {
    entries.push(entry);
    let excess = entries.len().saturating_sub(MAX_ENTRIES);
    entries.drain(..excess);
    entries
}

/// `First line of the text…  ·  2 min ago`, for the menu.
pub fn label(entry: &Entry, now_ms: u64) -> String {
    let first = entry.text.lines().map(str::trim).find(|line| !line.is_empty()).unwrap_or("");
    let short: String = first.chars().take(LABEL_CHARS).collect();
    let more = first.chars().count() > LABEL_CHARS || entry.text.trim().lines().count() > 1;
    let age = crate::inbox::relative_age(now_ms.saturating_sub(entry.at_ms));
    format!("{short}{}  ·  {age}", if more { "…" } else { "" })
}

pub fn default_path() -> PathBuf {
    path_in(&crate::handoff::default_captures_dir())
}

/// The history beside the captures folder, in Scrawl's Application Support folder.
pub fn path_in(captures: &Path) -> PathBuf {
    captures.with_file_name("dictation-history.json")
}

/// The saved entries, oldest first; a missing or broken file is none.
pub fn load_from(path: &Path) -> Vec<Entry> {
    std::fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<File>(&bytes).ok())
        .map(|file| file.entries)
        .unwrap_or_default()
}

/// The last dictation's audio, beside the history at `history`.
pub fn audio_path(history: &Path) -> PathBuf {
    history.with_file_name("last-dictation.wav")
}

fn save_to(path: &Path, entries: Vec<Entry>) -> Result<()> {
    let mut json = serde_json::to_vec_pretty(&File { entries })?;
    json.push(b'\n');
    write_private(path, &json)
}

/// Writes a temp file and renames it over `path`, owner-only, since dictations can be private.
fn write_private(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let temp = path.with_extension(format!("{}.tmp", std::process::id()));
    let _ = std::fs::remove_file(&temp);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let written = options.open(&temp).and_then(|mut file| std::io::Write::write_all(&mut file, bytes));
    let saved = written
        .map_err(anyhow::Error::from)
        .and_then(|()| std::fs::rename(&temp, path).with_context(|| format!("Could not save {}", path.display())));
    // A half-written temp file holds dictated text too.
    if saved.is_err() {
        let _ = std::fs::remove_file(&temp);
    }
    saved
}

/// Keeps a pasted dictation: as the last, and on disk at `path` when `keep` (`dictationHistory`);
/// not at all once a Clear came since `generation`, taken when the dictation ended.
pub fn record(path: &Path, text: &str, at_ms: u64, keep: bool, generation: u64) -> Result<()> {
    let _editing = edit_lock();
    if self::generation() != generation {
        return Ok(());
    }
    *LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Some(text.to_owned());
    if !keep {
        return Ok(());
    }
    save_to(path, remember(load_from(path), Entry { at_ms, text: text.to_owned() }))
}

/// Keeps a dictation's audio as the last, beside the history at `history`, when `keep`; with
/// history off, removes any kept before. Like `record`, keeps nothing once a Clear came since
/// `generation`.
pub fn keep_audio(history: &Path, wav: &[u8], keep: bool, generation: u64) -> Result<()> {
    let _editing = edit_lock();
    if self::generation() != generation {
        return Ok(());
    }
    let path = audio_path(history);
    if keep { write_private(&path, wav) } else { remove(&path) }
}

/// The last dictation: this run's, else the newest on disk at `path`.
pub fn last(path: &Path) -> Option<String> {
    let kept = LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner).clone();
    kept.or_else(|| load_from(path).pop().map(|entry| entry.text))
}

/// Forgets the last and deletes the file at `path`; none saved is no error.
pub fn clear(path: &Path) -> Result<()> {
    let _editing = edit_lock();
    GENERATION.fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    *LAST.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    let audio = remove(&audio_path(path));
    remove(path).and(audio)
}

/// Deletes `path`; a missing file is no error.
fn remove(path: &Path) -> Result<()> {
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

    fn entry(n: u64) -> Entry {
        Entry { at_ms: n, text: format!("Dictation {n}") }
    }

    #[test]
    fn the_newest_entries_are_kept() {
        let mut entries = Vec::new();
        for n in 0..(MAX_ENTRIES as u64 + 5) {
            entries = remember(entries, entry(n));
        }
        assert_eq!(entries.len(), MAX_ENTRIES);
        assert_eq!(entries.first(), Some(&entry(5)));
        assert_eq!(entries.last(), Some(&entry(MAX_ENTRIES as u64 + 4)));
    }

    #[test]
    fn a_label_is_the_first_line_and_its_age() {
        let now = 10 * 60_000;
        assert_eq!(label(&Entry { at_ms: now, text: "Book the cabin.".into() }, now), "Book the cabin.  ·  just now");
        let list = Entry { at_ms: 0, text: "\nWe need:\n- tents\n- maps".into() };
        assert_eq!(label(&list, now), "We need:…  ·  10 min ago");
        let long = Entry { at_ms: now, text: "word ".repeat(20) };
        assert!(label(&long, now).starts_with(&"word ".repeat(10)[..LABEL_CHARS]), "cut at LABEL_CHARS");
        assert!(label(&long, now).contains('…'));
    }

    // One test, since the last dictation is process-wide.
    #[test]
    fn dictations_persist_and_clear_deletes_them() {
        let dir = std::env::temp_dir().join(format!("scrawl-history-test-{}", std::process::id()));
        let path = path_in(&dir.join("captures"));
        assert_eq!(path.file_name().unwrap(), "dictation-history.json");
        clear(&path).unwrap();
        assert_eq!(last(&path), None);
        record(&path, "Only in memory.", 1, false, generation()).unwrap();
        assert!(!path.exists(), "history off writes nothing");
        assert_eq!(last(&path).as_deref(), Some("Only in memory."));
        record(&path, "First.", 2, true, generation()).unwrap();
        let before_clear = generation();
        record(&path, "Second.", 3, true, before_clear).unwrap();
        assert_eq!(load_from(&path).iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["First.", "Second."]);
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path).unwrap().permissions()) & 0o777, 0o600);
        *LAST.lock().unwrap() = None;
        assert_eq!(last(&path).as_deref(), Some("Second."), "after a restart, the newest on disk");
        clear(&path).unwrap();
        assert!(!path.exists());
        assert_eq!(last(&path), None);
        record(&path, "Finished before the Clear.", 4, true, before_clear).unwrap();
        assert!(!path.exists(), "a dictation in flight at Clear is not kept");
        assert_eq!(last(&path), None);
        std::fs::write(&path, "not json").unwrap();
        assert!(load_from(&path).is_empty());

        let audio = audio_path(&path);
        assert_eq!(audio.file_name().unwrap(), "last-dictation.wav");
        keep_audio(&path, b"RIFF one", true, generation()).unwrap();
        keep_audio(&path, b"RIFF two", true, generation()).unwrap();
        assert_eq!(std::fs::read(&audio).unwrap(), b"RIFF two", "only the last is kept");
        #[cfg(unix)]
        assert_eq!(std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&audio).unwrap().permissions()) & 0o777, 0o600);
        keep_audio(&path, b"RIFF off", false, generation()).unwrap();
        assert!(!audio.exists(), "history off removes the kept audio");
        keep_audio(&path, b"RIFF three", true, generation()).unwrap();
        let before_clear = generation();
        clear(&path).unwrap();
        assert!(!audio.exists(), "Clear deletes the audio");
        keep_audio(&path, b"RIFF in flight", true, before_clear).unwrap();
        assert!(!audio.exists(), "audio in flight at Clear is not kept");
        let _ = std::fs::remove_dir_all(dir);
    }
}

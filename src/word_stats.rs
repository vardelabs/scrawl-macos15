//! How many words the user has spoken into Scrawl, and how fast: dictations as pasted or copied,
//! and the narration of screen and iPhone captures recorded in the app. Settings › Dictation and
//! the menu bar show the totals and the average speed, and the pill says each dictation's speed.
//!
//! Speed counts only time spent talking. With measured word timings each word counts in full and
//! each gap between two words counts up to `PAUSE_ALLOWANCE_MS`, so a breath counts and stopping
//! to think doesn't: the user need not let go of the chord to keep the number honest. The cap is
//! continuous, so a pause just over the allowance counts the same as one just under. Estimated
//! timings (spread over the speech, as Hex's are) say nothing about pauses, so those dictations
//! count the recording's length.
//!
//! The counts go to `word-stats.json` beside the captures folder, owner-only. It holds numbers
//! only, never text, so Clear (recent dictations) leaves it; Reset in Settings zeroes it.

use std::path::{Path, PathBuf};
use std::sync::Mutex;

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::model::{TranscriptData, WordTiming};

/// The longest gap between two words that counts in full as talking. Settings › Dictation says
/// "one second" in its caption.
pub const PAUSE_ALLOWANCE_MS: u64 = 1_000;
/// A dictation shorter than this says no speed in the pill: a few words make a noisy number.
pub const MIN_NOTICE_WORDS: u64 = 8;

/// Words and the time spent saying them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Tally {
    #[serde(default)]
    pub words: u64,
    #[serde(default)]
    pub speaking_ms: u64,
}

impl Tally {
    /// Words per minute, rounded; `None` with no time spoken.
    pub fn wpm(&self) -> Option<u64> {
        (self.speaking_ms > 0).then(|| (self.words * 60_000 + self.speaking_ms / 2) / self.speaking_ms)
    }

    fn add(&mut self, other: Tally) {
        self.words = self.words.saturating_add(other.words);
        self.speaking_ms = self.speaking_ms.saturating_add(other.speaking_ms);
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Stats {
    #[serde(default)]
    pub dictation: Tally,
    #[serde(default)]
    pub captures: Tally,
    /// When counting began, in epoch milliseconds: the first word after a Reset.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub since_ms: Option<u64>,
}

impl Stats {
    pub fn total(&self) -> Tally {
        let mut total = self.dictation;
        total.add(self.captures);
        total
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    Dictation,
    Capture,
}

/// The words in `text`: whitespace-separated runs with a letter or digit, so a list's `-` or a
/// lone `&` isn't one.
pub fn count_words(text: &str) -> u64 {
    text.split_whitespace().filter(|word| word.chars().any(char::is_alphanumeric)).count() as u64
}

/// Time spent talking: each word in full and each gap between words up to `PAUSE_ALLOWANCE_MS`.
/// Without measured timings, the recording's length.
pub fn speaking_ms(words: &[WordTiming], estimated: bool, audio_ms: u64) -> u64 {
    if estimated || words.is_empty() {
        return audio_ms;
    }
    let said: u64 = words.iter().map(|word| word.t1.saturating_sub(word.t0)).sum();
    let paused: u64 = words
        .windows(2)
        .map(|pair| pair[1].t0.saturating_sub(pair[0].t1).min(PAUSE_ALLOWANCE_MS))
        .sum();
    match said + paused {
        0 => audio_ms,
        spoken => spoken,
    }
}

/// `text` as said in `transcript`, from a recording `audio_ms` long.
pub fn tally(text: &str, transcript: &TranscriptData, audio_ms: u64) -> Tally {
    Tally { words: count_words(text), speaking_ms: speaking_ms(&transcript.words, transcript.estimated, audio_ms) }
}

/// Reading, adding to and resetting the file each hold this, so a dictation and a capture
/// finishing together both count.
static EDIT: Mutex<()> = Mutex::new(());

fn edit_lock() -> std::sync::MutexGuard<'static, ()> {
    EDIT.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

pub fn default_path() -> PathBuf {
    path_in(&crate::handoff::default_captures_dir())
}

/// The counts beside the captures folder, in Scrawl's Application Support folder.
pub fn path_in(captures: &Path) -> PathBuf {
    captures.with_file_name("word-stats.json")
}

/// The counts so far; a missing or broken file is none.
pub fn load_from(path: &Path) -> Stats {
    std::fs::read(path).ok().and_then(|bytes| serde_json::from_slice(&bytes).ok()).unwrap_or_default()
}

/// Adds `tally` from `source` to the counts at `path`, at `now_ms`. A tally with no words adds
/// nothing.
pub fn record(path: &Path, source: Source, tally: Tally, now_ms: u64) -> Result<()> {
    if tally.words == 0 {
        return Ok(());
    }
    let _editing = edit_lock();
    let mut stats = load_from(path);
    stats.since_ms.get_or_insert(now_ms);
    match source {
        Source::Dictation => stats.dictation.add(tally),
        Source::Capture => stats.captures.add(tally),
    }
    let mut json = serde_json::to_vec_pretty(&stats)?;
    json.push(b'\n');
    crate::dictation::history::write_private(path, &json)
}

/// Counts a capture the app just built, logging a failure: the counts never hold up a capture.
pub fn record_capture(bundle: &crate::model::ScrawlBundle) {
    let transcript = &bundle.transcript;
    let tally = tally(&transcript.text, transcript, bundle.duration_ms);
    if let Err(error) = record(&default_path(), Source::Capture, tally, crate::now_epoch_ms()) {
        crate::capture::scrawl_log(format!("Word stats: {error:#}"));
    }
}

/// Zeroes the counts: deletes the file at `path`; none saved is no error.
pub fn reset(path: &Path) -> Result<()> {
    let _editing = edit_lock();
    match std::fs::remove_file(path) {
        Err(error) if error.kind() != std::io::ErrorKind::NotFound => Err(error.into()),
        _ => Ok(()),
    }
}

/// `12,480`: a count with its thousands grouped, for the menu.
pub fn grouped(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

/// `12,480 words  ·  148 WPM`, for the menu bar menu; `None` before the first word.
pub fn menu_label(stats: &Stats) -> Option<String> {
    let total = stats.total();
    if total.words == 0 {
        return None;
    }
    let words = format!("{} {}", grouped(total.words), if total.words == 1 { "word" } else { "words" });
    Some(match total.wpm() {
        Some(wpm) => format!("{words}  ·  {wpm} WPM"),
        None => words,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn word(t0: u64, t1: u64) -> WordTiming {
        WordTiming { t0, t1, text: "word".into() }
    }

    #[test]
    fn a_thinking_pause_counts_only_up_to_the_allowance() {
        // Two words of 400 ms, then a 6 s pause to think, then two more.
        let words = [word(0, 400), word(500, 900), word(6_900, 7_300), word(7_400, 7_800)];
        // 1,600 ms of words, 100 + 1,000 + 100 ms of gaps.
        assert_eq!(speaking_ms(&words, false, 9_000), 2_800);
    }

    #[test]
    fn the_cap_is_continuous_around_the_allowance() {
        let gap = |ms: u64| speaking_ms(&[word(0, 100), word(100 + ms, 200 + ms)], false, 0) - 200;
        assert_eq!(gap(PAUSE_ALLOWANCE_MS - 1), PAUSE_ALLOWANCE_MS - 1);
        assert_eq!(gap(PAUSE_ALLOWANCE_MS + 1), PAUSE_ALLOWANCE_MS);
        assert_eq!(gap(10 * PAUSE_ALLOWANCE_MS), PAUSE_ALLOWANCE_MS);
    }

    #[test]
    fn estimated_or_missing_timings_count_the_recording() {
        assert_eq!(speaking_ms(&[word(0, 400), word(5_000, 5_400)], true, 7_000), 7_000);
        assert_eq!(speaking_ms(&[], false, 7_000), 7_000);
        assert_eq!(speaking_ms(&[word(300, 300)], false, 7_000), 7_000, "a zero-length timing says nothing");
    }

    #[test]
    fn overlapping_timings_add_no_negative_gap() {
        assert_eq!(speaking_ms(&[word(0, 500), word(400, 900)], false, 0), 1_000);
    }

    #[test]
    fn words_need_a_letter_or_digit() {
        assert_eq!(count_words("We need:\n- tents\n- maps & 2 stoves."), 6);
        assert_eq!(count_words("   "), 0);
    }

    #[test]
    fn wpm_rounds_to_the_nearest() {
        assert_eq!(Tally { words: 150, speaking_ms: 60_000 }.wpm(), Some(150));
        assert_eq!(Tally { words: 5, speaking_ms: 2_000 }.wpm(), Some(150));
        assert_eq!(Tally { words: 1, speaking_ms: 40_000 }.wpm(), Some(2), "1.5 rounds up");
        assert_eq!(Tally { words: 3, speaking_ms: 0 }.wpm(), None);
    }

    #[test]
    fn dictations_and_captures_add_up_and_reset() {
        let dir = std::env::temp_dir().join(format!("scrawl-word-stats-{}", std::process::id()));
        let path = dir.join("word-stats.json");
        let _ = std::fs::remove_dir_all(&dir);
        record(&path, Source::Dictation, Tally { words: 100, speaking_ms: 40_000 }, 7).unwrap();
        record(&path, Source::Capture, Tally { words: 50, speaking_ms: 20_000 }, 9).unwrap();
        record(&path, Source::Dictation, Tally { words: 0, speaking_ms: 5_000 }, 11).unwrap();
        let stats = load_from(&path);
        assert_eq!(stats.dictation, Tally { words: 100, speaking_ms: 40_000 });
        assert_eq!(stats.captures, Tally { words: 50, speaking_ms: 20_000 });
        assert_eq!(stats.since_ms, Some(7), "counting began with the first words");
        assert_eq!(stats.total().wpm(), Some(150));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        }
        reset(&path).unwrap();
        assert_eq!(load_from(&path), Stats::default());
        reset(&path).unwrap();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_broken_file_counts_as_none() {
        let dir = std::env::temp_dir().join(format!("scrawl-word-stats-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("word-stats.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(load_from(&path), Stats::default());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_menu_groups_thousands() {
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(12_480), "12,480");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(menu_label(&Stats::default()), None);
        let stats = Stats { dictation: Tally { words: 12_480, speaking_ms: 5_060_000 }, ..Stats::default() };
        assert_eq!(menu_label(&stats).as_deref(), Some("12,480 words  ·  148 WPM"));
    }
}

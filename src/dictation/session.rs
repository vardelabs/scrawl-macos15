//! The running side of dictation on macOS: the microphone, the pill, and the transcribe-and-paste
//! that follows. Driven from the UI loop's 8 ms tick, so it is on the main thread.

use std::time::{Duration, Instant};

use super::chord::{Action, Chord, Mods};
use crate::audio::{AudioRecorder, RecordedAudio};
use crate::capture::{
    FLAG_ALTERNATE, FLAG_COMMAND, FLAG_CONTROL, FLAG_SHIFT, SharedCaptureTelemetry, scrawl_log,
};
use crate::native::pill::{PillAction, model::Pres};

/// Less audio than this is a slip of the chord, not speech.
const MIN_AUDIO_MS: u64 = 300;
/// Settings are read from `config.json` this often, not on every tick.
const SETTINGS_EVERY: Duration = Duration::from_millis(500);
/// The focused window's text is read up to this many elements or this long, for filenames.
const SCREEN_ELEMENTS: usize = 1_500;
const SCREEN_BUDGET: Duration = Duration::from_millis(150);
/// The paste waits at most this long after the chord for the screen read, which can overrun its
/// budget in an app slow to answer Accessibility; past it, the text goes in without filenames.
const SCREEN_WAIT: Duration = Duration::from_millis(400);
/// Apps where text is prose, so code-like words don't go in backticks.
const PROSE_APPS: &[&str] = &[
    "com.apple.mail",
    "com.apple.MobileSMS",
    "com.apple.Notes",
    "com.apple.iWork.Pages",
    "com.microsoft.Word",
    "com.microsoft.Outlook",
];
/// A dictation this long starts the formatter; `format::MIN_WORDS` words take longer to say.
const FORMAT_PRESTART_AFTER: Duration = Duration::from_secs(5);
/// How long the paste waits for the formatter once the text is ready: its model has been loading
/// since the dictation showed, and layout takes longer the more there is.
const FORMAT_WAIT_BASE: Duration = Duration::from_secs(3);
const FORMAT_WAIT_PER_WORD: Duration = Duration::from_millis(40);
const FORMAT_WAIT_MAX: Duration = Duration::from_secs(15);
/// After a paste, the field is watched this long for the user's corrections, or until focus
/// leaves it.
const LEARN_FOR: Duration = Duration::from_secs(60);
const LEARN_EVERY: Duration = Duration::from_secs(1);
/// The paste lands this long after `⌘V`, so the field is read after it.
const PASTE_SETTLE: Duration = Duration::from_millis(300);
/// A click this soon after a notice goes was aimed at the notice, not the hint that replaced it.
const CLICK_GRACE: Duration = Duration::from_millis(700);

struct Settings {
    enabled: bool,
    mask: u64,
    /// The chord as the hint shows it (`⌃⌥`).
    label: String,
    restore_clipboard: bool,
    pill: bool,
    /// Match filenames on screen and put code in backticks (`dictationCodeFormatting`).
    code: bool,
    /// The formatter for paragraphs and lists (`dictationFormatting`, `formatCommand`).
    format: Option<Vec<String>>,
    /// Learn words from the user's edits to pasted text (`dictationLearn`).
    learn: bool,
    /// Say in the pill what dictation learned or laid out (`dictationNotices`).
    notices: bool,
    /// Keep recent dictations on disk (`dictationHistory`).
    history: bool,
    /// A dictation to copy again: this run's, or one on disk.
    has_last: bool,
}

impl Settings {
    fn load() -> Self {
        let config = crate::handoff::dictation();
        let label = config
            .shortcut
            .filter(|chord| super::parse_shortcut(chord).is_some())
            .unwrap_or_else(|| super::DEFAULT_SHORTCUT.to_owned());
        let mods = super::parse_shortcut(&label).expect("the chord was just checked");
        let mask = [
            (mods.control, FLAG_CONTROL),
            (mods.option, FLAG_ALTERNATE),
            (mods.shift, FLAG_SHIFT),
            (mods.command, FLAG_COMMAND),
        ]
        .into_iter()
        .filter(|(on, _)| *on)
        .fold(0, |mask, (_, flag)| mask | flag);
        Self {
            enabled: config.enabled,
            mask,
            label,
            restore_clipboard: config.restore_clipboard,
            pill: config.pill,
            code: config.code,
            format: config.format,
            learn: config.learn,
            notices: config.notices,
            history: config.history,
            has_last: super::history::last(&super::history::default_path()).is_some(),
        }
    }

    fn mods(&self, flags: u64) -> Mods {
        if flags & self.mask != self.mask {
            Mods::Up
        } else if flags == self.mask {
            Mods::Exact
        } else {
            Mods::Extra
        }
    }
}

/// How a finished dictation ended, for the pill's notice.
enum Outcome {
    /// Pasted, with how the formatter laid it out ("Formatted as a list").
    Pasted(Option<&'static str>),
    /// Nowhere to paste: the text is on the clipboard.
    Copied,
    NoSpeech,
    /// Too short to be speech, or it failed (logged): nothing to say.
    Quiet,
}

impl Outcome {
    fn notice(&self) -> Option<&'static str> {
        match self {
            Self::Copied => Some("Copied to the clipboard"),
            Self::NoSpeech => Some("Didn't catch that"),
            Self::Pasted(_) | Self::Quiet => None,
        }
    }
}

/// What the pill's presentation depends on, so the choice can be tested without a pill.
struct PillState<'a> {
    working: bool,
    /// Listening, and whether latched.
    dictating: Option<bool>,
    capture_busy: bool,
    /// The notice's text, and whether it offers Undo.
    notice: Option<(&'a str, bool)>,
    enabled: bool,
    pill: bool,
    hovered: bool,
    chord: &'a str,
    /// A dictation the hint's Copy can copy.
    copy: bool,
}

/// What the pill shows, most pressing first.
fn presentation(state: &PillState) -> Pres {
    if state.working {
        return Pres::Processing;
    }
    if let Some(latched) = state.dictating {
        return Pres::Dictating { latched };
    }
    if state.capture_busy {
        return Pres::Hidden;
    }
    if let Some((text, undo)) = state.notice {
        return Pres::Message {
            label: text.to_owned(),
            undo,
        };
    }
    // With dictation off the bar stays away, so turning dictation on is what adds it to the screen.
    if !state.pill || !state.enabled {
        return Pres::Hidden;
    }
    if state.hovered {
        return Pres::Hint {
            dictate: format!("Dictate {}", state.chord),
            copy: state.copy,
        };
    }
    Pres::DictationIdle
}

/// How dictated text reaches the app, from whether the focused element takes text. Every route
/// pastes: an app with its Accessibility tree off can report nothing focused over a real field.
#[derive(Debug, PartialEq, Eq)]
enum Route {
    /// A text field: paste, then put the clipboard back.
    Paste,
    /// Nothing focused: paste anyway, keep the text on the clipboard, and say so.
    Copy,
    /// The app didn't say: paste, and keep the text on the clipboard so nothing is lost.
    PasteAndKeep,
}

fn route(takes_text: Option<bool>) -> Route {
    match takes_text {
        Some(true) => Route::Paste,
        Some(false) => Route::Copy,
        None => Route::PasteAndKeep,
    }
}

pub struct Dictation {
    chord: Chord,
    clock: Instant,
    settings: Settings,
    settings_read: Instant,
    recorder: Option<AudioRecorder>,
    /// The formatter, loading its model while the user talks; dropped (killed) if unused.
    formatter: Option<super::formatter::Formatter>,
    /// Whether this dictation already started (or failed to start) its formatter.
    formatter_started: bool,
    started: Option<Instant>,
    playback: Option<crate::playback::Guard>,
    /// Transcribing and pasting on a worker; the next dictation waits for it.
    working: Option<flume::Receiver<Outcome>>,
    notices: super::notices::Notices,
    /// What the edit watcher learned, checked and saved on this thread (`take_learned`).
    learned_tx: flume::Sender<Learned>,
    learned_rx: flume::Receiver<Learned>,
}

/// What one paste's edits taught: respelled words, and a new layout of the formatter's text.
struct Learned {
    words: Vec<super::learn::Respelling>,
    layout: Option<super::layout::Example>,
    /// `layout::generation` at the paste, so a Forget since then drops the layout.
    generation: u64,
}

/// What the shared pill reports to dictation while dictation is what it shows.
#[derive(Default)]
pub struct PillInput {
    pub hovered: bool,
    pub clicks: Vec<PillAction>,
    /// The pill shows dictation now; a capture's island (its Completed card too) takes it.
    pub shown: bool,
}

impl Default for Dictation {
    fn default() -> Self {
        let (learned_tx, learned_rx) = flume::unbounded();
        Self {
            chord: Chord::default(),
            clock: Instant::now(),
            settings: Settings::load(),
            settings_read: Instant::now(),
            recorder: None,
            formatter: None,
            formatter_started: false,
            started: None,
            playback: None,
            working: None,
            notices: super::notices::Notices::default(),
            learned_tx,
            learned_rx,
        }
    }
}

impl Dictation {
    /// Latched, or still pasting the last one: the capture shortcut is ignored. A held chord
    /// gives way to it instead (`cede`).
    pub fn is_busy(&self) -> bool {
        self.chord.is_latched() || self.working.is_some()
    }

    /// The capture shortcut went down: a held, unlatched chord was the start of it (a capture
    /// shortcut that contains the chord's modifiers), so that recording is dropped.
    pub fn cede(&mut self) {
        if self.chord.is_active() && !self.chord.is_latched() {
            let action = self.chord.cancel();
            self.apply(action, true, true);
        }
    }

    /// Advances dictation and returns what the shared pill should show while no capture is on
    /// it. `capture_busy`: a capture is recording, selecting or processing, so the chord is
    /// ignored.
    pub fn tick(
        &mut self,
        telemetry: &SharedCaptureTelemetry,
        capture_busy: bool,
        pill: PillInput,
    ) -> Pres {
        if !self.chord.is_active() && self.settings_read.elapsed() >= SETTINGS_EVERY {
            self.settings = Settings::load();
            self.settings_read = Instant::now();
        }
        self.take_outcome();
        self.start_formatter_once_long();
        self.take_learned();
        // Clicks arrive every tick, so one made while dictation was off can't start it later.
        let clicks = pill.clicks;
        // An Undo just after a notice went was aimed at it, not at the notice that took its place.
        let settled = self.notices.gone().is_none_or(|at| at.elapsed() >= CLICK_GRACE);
        if settled && self.notices.showing() && clicks.iter().any(|c| matches!(c, PillAction::Undo)) {
            self.undo();
        }
        if clicks.iter().any(|c| matches!(c, PillAction::CopyLast)) {
            self.copy_last();
        }
        if self.settings.enabled {
            self.listen(telemetry, capture_busy, clicks);
        } else if clicks.iter().any(|c| matches!(c, PillAction::MessageClicked)) {
            // Off, the pill can still show the last dictation's notice; a click dismisses it.
            self.notices.dismiss(Instant::now());
        }
        let free = pill.shown && self.working.is_none() && self.started.is_none() && !capture_busy;
        let notice = self.notices.current(free, Instant::now()).map(|n| (n.text.clone(), n.undo.is_some()));
        presentation(&PillState {
            working: self.working.is_some(),
            dictating: self.started.map(|_| self.chord.is_latched()),
            capture_busy,
            notice: notice.as_ref().map(|(text, undo)| (text.as_str(), *undo)),
            enabled: self.settings.enabled,
            pill: self.settings.pill,
            hovered: pill.hovered,
            chord: &self.settings.label,
            copy: self.settings.has_last,
        })
    }

    /// Listening or transcribing, so the pill should show it now.
    pub fn is_live(&self) -> bool {
        self.started.is_some() || self.working.is_some()
    }

    /// The mic's RMS while it is open (from the chord going down, before the pill grows), so
    /// the pill can show it is hearing; `None` while it is closed.
    pub fn mic_rms(&self) -> Option<f32> {
        self.recorder.as_ref().map(AudioRecorder::rms)
    }

    /// The listening clock (`M:SS`) for the pill.
    pub fn clock(&self) -> String {
        crate::model::format_clock(self.started.map_or(0, |at| at.elapsed().as_millis() as u64))
    }

    /// Copy on the hint: the last dictation onto the clipboard again, for one lost to a clipboard
    /// something else overwrote.
    pub fn copy_last(&mut self) {
        if let Some(text) = super::history::last(&super::history::default_path()) {
            self.copy(&text);
        }
    }

    /// Recent Dictations in the menu bar: the dictation pasted at `at_ms` onto the clipboard.
    pub fn copy_recent(&mut self, at_ms: u64) {
        let entries = super::history::load_from(&super::history::default_path());
        if let Some(entry) = entries.into_iter().find(|entry| entry.at_ms == at_ms) {
            self.copy(&entry.text);
        }
    }

    fn copy(&mut self, text: &str) {
        match crate::handoff::copy_string_to_clipboard(text) {
            Ok(()) => self.notices.push_first("Copied"),
            Err(error) => scrawl_log(format!("Dictation: could not copy a dictation: {error:#}")),
        }
    }

    /// Undo on the notice: takes back what it reports dictation learned.
    fn undo(&mut self) {
        use super::notices::Undo;
        let Some(undo) = self.notices.dismiss(Instant::now()).and_then(|n| n.undo) else { return };
        match undo {
            Undo::Word(word) => {
                let path = crate::dictionary::default_path();
                let _editing = crate::dictionary::edit_lock();
                let Ok(mut dictionary) = crate::dictionary::Dictionary::load_from(&path) else { return };
                // Removing a learned word records it as rejected, so it is never learned again.
                dictionary.remove_word(&word);
                if let Err(error) = dictionary.save_to(&path) {
                    scrawl_log(format!("Dictation: could not undo a learned word: {error:#}"));
                }
            }
            Undo::Layout(learned, replaced) => {
                let path = super::layout::default_path();
                let _editing = super::layout::edit_lock();
                let examples = super::layout::unlearn(super::layout::load_from(&path), &learned, replaced);
                if let Err(error) = super::layout::save_to(&path, examples) {
                    scrawl_log(format!("Dictation: could not undo a learned layout: {error:#}"));
                }
            }
        }
        scrawl_log("Dictation: undid what an edit taught");
    }

    fn listen(
        &mut self,
        telemetry: &SharedCaptureTelemetry,
        capture_busy: bool,
        clicks: Vec<PillAction>,
    ) {
        let key_tap_live = telemetry.key_tap_live() && !crate::capture::secure_input_on();
        let now_ms = self.clock.elapsed().as_millis() as u64;
        let mods = self.settings.mods(crate::capture::modifier_flags());
        let keys = telemetry.key_downs.load(std::sync::atomic::Ordering::Acquire);
        if capture_busy && self.started.is_none() && self.chord.is_active() {
            // A capture started while the chord was undecided: it was the capture's shortcut.
            let action = self.chord.cancel();
            self.apply(action, capture_busy, key_tap_live);
        }
        let action = self.chord.tick(now_ms, mods, keys);
        self.apply(action, capture_busy, key_tap_live);
        if self.chord.is_active() && !self.chord.is_latched() && crate::capture::mouse_button_down()
        {
            // A modifier-click or drag, not dictation.
            let action = self.chord.cancel();
            self.apply(action, capture_busy, key_tap_live);
        }
        if self.chord.is_active() && telemetry.take_escape() {
            let action = self.chord.cancel();
            self.apply(action, capture_busy, key_tap_live);
        }
        for click in clicks {
            let action = match click {
                PillAction::Cancel => self.chord.cancel(),
                PillAction::Finish => self.chord.confirm(),
                PillAction::MessageClicked => {
                    self.message_clicked(capture_busy, key_tap_live);
                    Action::None
                }
                _ => Action::None,
            };
            self.apply(action, capture_busy, key_tap_live);
        }
    }

    /// A click on a notice dismisses it; a click on the hint starts hands-free dictation, unless
    /// it lands just after a notice went (the press was meant for the notice).
    fn message_clicked(&mut self, capture_busy: bool, key_tap_live: bool) {
        if self.notices.showing() {
            self.notices.dismiss(Instant::now());
        } else if self.notices.gone().is_none_or(|at| at.elapsed() >= CLICK_GRACE) {
            self.start_hands_free(capture_busy, key_tap_live);
        }
    }

    /// A click on the idle bar's hint: hands-free dictation, as a double tap starts it.
    fn start_hands_free(&mut self, capture_busy: bool, key_tap_live: bool) {
        let now_ms = self.clock.elapsed().as_millis() as u64;
        if self.working.is_some() || !self.chord.latch(now_ms) {
            return;
        }
        self.apply(Action::Start, capture_busy, key_tap_live);
        if self.chord.is_active() {
            self.apply(Action::Show { latched: true }, capture_busy, key_tap_live);
        }
    }

    /// Starts the formatter once a dictation has run `FORMAT_PRESTART_AFTER`: a shorter one is too
    /// short to lay out, so it never loads the model, and a long one still overlaps the load with
    /// talking.
    fn start_formatter_once_long(&mut self) {
        let long = self.started.is_some_and(|at| at.elapsed() >= FORMAT_PRESTART_AFTER);
        if !long || self.formatter_started {
            return;
        }
        self.formatter_started = true;
        self.formatter = self.settings.format.as_deref().and_then(|command| {
            super::formatter::Formatter::prestart(command)
                .inspect_err(|error| scrawl_log(format!("Dictation: {error:#}")))
                .ok()
        });
    }

    fn take_outcome(&mut self) {
        let Some(rx) = &self.working else {
            return;
        };
        let outcome = match rx.try_recv() {
            Err(flume::TryRecvError::Empty) => return,
            Ok(outcome) => outcome,
            Err(flume::TryRecvError::Disconnected) => Outcome::Quiet,
        };
        self.working = None;
        let text = match outcome {
            Outcome::Pasted(laid_out) => laid_out.filter(|_| self.settings.notices),
            other => other.notice(),
        };
        if let Some(text) = text {
            self.notices.push_first(text);
        }
    }

    /// Keeps the respellings the spell checker doesn't know, adds them to the dictionary, and
    /// says so in the pill; saves learned layouts while the formatter is on.
    fn take_learned(&mut self) {
        let learned: Vec<Learned> = self.learned_rx.try_iter().collect();
        let saving = self.settings.learn && self.settings.format.is_some();
        let mut words = Vec::new();
        for Learned { words: respelled, layout, generation } in learned {
            words.push(respelled);
            if let Some(example) = layout.filter(|_| saving)
                && let Some(replaced) = save_layout(&example, generation)
                && self.settings.notices
            {
                let undo = super::notices::Undo::Layout(example, replaced);
                self.notices.push(super::notices::LEARNED_LAYOUT, Some(undo));
            }
        }
        let words: Vec<String> = words
            .into_iter()
            .flatten()
            .filter(|(was, now)| self.settings.learn && spell_checker_unknown(now) && !spell_checker_typo(was, now))
            .map(|(_, now)| now)
            .take(super::learn::MAX_LEARNED)
            .collect();
        if words.is_empty() {
            return;
        }
        let path = crate::dictionary::default_path();
        let _editing = crate::dictionary::edit_lock();
        // A dictionary that doesn't parse is left alone, as Settings leaves it.
        let Ok(mut dictionary) = crate::dictionary::Dictionary::load_from(&path) else { return };
        let added: Vec<String> = words.into_iter().filter(|w| dictionary.learn_word(w)).collect();
        if added.is_empty() {
            return;
        }
        match dictionary.save_to(&path) {
            Ok(()) => scrawl_log(format!("Dictation: learned {} word(s) from an edit", added.len())),
            Err(error) => return scrawl_log(format!("Dictation: could not save learned words: {error:#}")),
        }
        if self.settings.notices {
            for word in added {
                // As `learn_word` stored it, so Undo's exact match finds it.
                let word = word.split_whitespace().collect::<Vec<_>>().join(" ");
                self.notices.push(super::notices::added_word(&word), Some(super::notices::Undo::Word(word)));
            }
        }
    }

    fn apply(&mut self, action: Action, capture_busy: bool, key_tap_live: bool) {
        if action != Action::None {
            scrawl_log(format!("dictation -> {action:?}"));
        }
        match action {
            Action::None => {}
            Action::Start if capture_busy || self.working.is_some() => {
                self.chord.cancel();
            }
            // Without the key tap a shortcut can't be told apart (nor, without Accessibility, the
            // paste posted). Secure Input, as in a password field, turns the tap off too.
            Action::Start if !key_tap_live => {
                scrawl_log("Dictation: the key tap is off (Accessibility, or Secure Input is on)");
                self.chord.cancel();
            }
            Action::Start => match AudioRecorder::start_mic() {
                Ok(recorder) => self.recorder = Some(recorder),
                Err(error) => {
                    scrawl_log(format!("Dictation: the microphone did not start: {error:#}"));
                    self.chord.cancel();
                }
            },
            Action::Show { .. } => {
                if let Some(recorder) = self.recorder.as_mut() {
                    recorder.prestart_transcriber();
                }
                self.formatter = None;
                self.formatter_started = false;
                self.playback = self.playback.take().or_else(crate::playback::Guard::start_quick);
                self.started = Some(Instant::now());
            }
            Action::Finish => self.finish(),
            // Dropping the recorder stops the microphone without encoding what it heard. The drop
            // waits for the device to finish opening, so it happens off the UI thread.
            Action::Discard => {
                if let Some(recorder) = self.recorder.take() {
                    std::thread::spawn(move || drop(recorder));
                }
                self.formatter = None;
                self.stop_listening();
            }
        }
    }

    fn stop_listening(&mut self) {
        self.started = None;
        self.playback = None;
    }

    /// Stopping the recorder joins its worker (which may still be opening the device) and encodes
    /// the WAV, so it happens on the worker with the transcription, not on the UI thread.
    fn finish(&mut self) {
        self.stop_listening();
        let Some(recorder) = self.recorder.take() else {
            return;
        };
        let restore = self.settings.restore_clipboard;
        let keep = self.settings.history;
        let formatter = self.formatter.take();
        let learn = self.settings.learn.then(|| self.learned_tx.clone());
        let backticks = self.settings.code && !prose_app();
        // The screen is read alongside transcription, while the target window still has focus,
        // and only with code formatting on.
        let screen = self.settings.code.then(|| {
            let (found, screen) = flume::bounded(1);
            let spawned = std::thread::Builder::new().name("scrawl-dictation-screen".into()).spawn(move || {
                let _ = found.send(super::context::terms(&crate::ax::window_text(SCREEN_ELEMENTS, SCREEN_BUDGET)));
            });
            if let Err(error) = spawned {
                scrawl_log(format!("Dictation: screen read: {error}"));
            }
            (screen, Instant::now() + SCREEN_WAIT)
        });
        let (done, working) = flume::bounded(1);
        self.working = Some(working);
        let spawned = std::thread::Builder::new()
            .name("scrawl-dictation".into())
            .spawn(move || {
                let outcome = match recorder.stop() {
                    Ok(audio) if audio.duration_ms >= MIN_AUDIO_MS => {
                        transcribe_and_paste(&audio, restore, screen, backticks, formatter, learn, keep)
                    }
                    Ok(_) => Outcome::Quiet,
                    Err(error) => {
                        scrawl_log(format!("Dictation: {error:#}"));
                        Outcome::Quiet
                    }
                };
                let _ = done.send(outcome);
            });
        if let Err(error) = spawned {
            scrawl_log(format!("Dictation: could not start transcribing: {error}"));
            self.working = None;
        }
    }
}

fn prose_app() -> bool {
    crate::capture::frontmost_app_info()
        .and_then(|(_, _, bundle)| bundle)
        .is_some_and(|bundle| PROSE_APPS.contains(&bundle.as_str()))
}

fn transcribe_and_paste(
    audio: &RecordedAudio,
    restore_clipboard: bool,
    screen: Option<(flume::Receiver<Vec<String>>, Instant)>,
    backticks: bool,
    formatter: Option<super::formatter::Formatter>,
    learn: Option<flume::Sender<Learned>>,
    keep: bool,
) -> Outcome {
    let started = Instant::now();
    let wav = std::env::temp_dir().join(format!("scrawl-dictation-{}.wav", std::process::id()));
    if let Err(error) = std::fs::write(&wav, &audio.wav_bytes) {
        scrawl_log(format!("Dictation: could not write {}: {error}", wav.display()));
        return Outcome::Quiet;
    }
    let transcript = crate::transcribe::transcribe(Some(&wav), Some(audio));
    let _ = std::fs::remove_file(&wav);
    // The dictionary first, so a user's rule for "design dot md" still sees what was said.
    let text = crate::dictionary::Dictionary::load().apply(transcript.text.trim());
    // With code formatting off, `screen` is `None` and the text is left as transcribed.
    let text = match screen {
        Some((read, deadline)) => {
            let text = super::context::spoken_extensions(&text);
            let terms = read.recv_deadline(deadline).unwrap_or_default();
            super::context::apply(&text, &terms, backticks)
        }
        None => text,
    };
    // Estimated timings spread words across the speech, so their gaps are not pauses.
    let timed: &[crate::model::WordTiming] = if transcript.estimated { &[] } else { &transcript.words };
    // What the formatter is asked to lay out, so an edit of its layout can be learned from.
    let dictated = (formatter.is_some() && super::format::worth_formatting(&text)).then(|| text.clone());
    let (text, laid_out) = lay_out(text, timed, formatter);
    scrawl_log(format!(
        "Dictation: {} ms of audio, {} words from {} in {} ms",
        audio.duration_ms,
        text.split_whitespace().count(),
        transcript.source,
        started.elapsed().as_millis()
    ));
    // Silence can come back as a lone full stop.
    if !text.chars().any(char::is_alphanumeric) {
        return Outcome::NoSpeech;
    }
    let outcome = deliver(&text, restore_clipboard, laid_out);
    if matches!(outcome, Outcome::Pasted(_) | Outcome::Copied) {
        let at = crate::now_epoch_ms();
        if let Err(error) = super::history::record(&super::history::default_path(), &text, at, keep) {
            scrawl_log(format!("Dictation: could not keep the dictation: {error:#}"));
        }
    }
    if let (Outcome::Pasted(_), Some(learned)) = (&outcome, learn) {
        watch_edits(text, dictated, learned);
    }
    outcome
}


/// A field longer than this (a whole document) is not watched.
const MAX_FIELD_CHARS: usize = 200_000;

/// Saves a layout learned from a paste in `generation`, unless a Forget came since; checked under
/// the lock Forget holds, so none can come between the check and the save. `Some` once saved,
/// holding the example of the same dictation it replaced, for Undo.
fn save_layout(example: &super::layout::Example, generation: u64) -> Option<Option<super::layout::Example>> {
    use super::layout;
    let path = layout::default_path();
    let _editing = layout::edit_lock();
    if layout::generation() != generation {
        return None;
    }
    let examples = layout::load_from(&path);
    let replaced = examples.iter().find(|e| e.dictated == example.dictated).cloned();
    match layout::save_to(&path, layout::remember(examples, example.clone())) {
        Ok(()) => {
            scrawl_log("Dictation: learned a layout from an edit");
            Some(replaced)
        }
        Err(error) => {
            scrawl_log(format!("Dictation: could not save a learned layout: {error:#}"));
            None
        }
    }
}

/// Watches the field the text was pasted into and sends the words the user respelled (`learn`)
/// once focus leaves the field, after `LEARN_FOR`, or when the field empties (a chat message
/// sent); the UI thread checks them against the spell checker (`take_learned`). With `dictated`,
/// the formatter's input, a change of layout alone is sent too (`layout::example`), a sent
/// message's included, under the same settling as words.
fn watch_edits(pasted: String, dictated: Option<String>, candidates: flume::Sender<Learned>) {
    let generation = super::layout::generation();
    let spawned = std::thread::Builder::new().name("scrawl-dictation-learn".into()).spawn(move || {
        std::thread::sleep(PASTE_SETTLE);
        let read = |field: &crate::ax::FocusedText| field.value(MAX_FIELD_CHARS);
        let Some(field) = crate::ax::FocusedText::current() else { return };
        let Some(paste) = read(&field).and_then(|value| super::learn::locate(&value, &pasted)) else { return };
        let mut settle = super::learn::Settle::default();
        let started = Instant::now();
        while started.elapsed() < LEARN_FOR && field.still_focused() {
            std::thread::sleep(LEARN_EVERY);
            let value = read(&field);
            if !settle.read(value.as_deref().and_then(|value| super::learn::edited_span(&paste, value))) {
                break;
            }
        }
        // Focus moved or time ran out: the last edit gets one more read, a full interval later so
        // it can't agree with a read a moment before. A window closed at the send reads as gone.
        std::thread::sleep(LEARN_EVERY);
        settle.read(read(&field).as_deref().and_then(|value| super::learn::edited_span(&paste, value)));
        let Some(edited) = settle.settled() else { return };
        let words = super::learn::learned(&paste.text, &edited);
        let layout = dictated.and_then(|dictated| super::layout::example(&dictated, &paste.text, &edited, &words));
        if !words.is_empty() || layout.is_some() {
            let _ = candidates.send(Learned { words, layout, generation });
        }
    });
    if let Err(error) = spawned {
        scrawl_log(format!("Dictation: could not watch for edits: {error}"));
    }
}

/// The languages a word is checked in: English, which the transcribers write, and the spell
/// checker's own, so a word only one of them knows isn't taken for a name.
const SPELL_LANGUAGES: [Option<&str>; 2] = [Some("en"), None];

/// What the spell checker says of `word` in `language` (`None`: its own): whether it knows the
/// word, and its correction. The language is always explicit, since automatic identification calls
/// "teh" correct. Main thread only, as AppKit's spell checker asks.
fn spelling(word: &str, language: Option<&str>) -> (bool, Option<String>) {
    use objc2::runtime::{AnyClass, AnyObject};
    use objc2::{Encode, Encoding, msg_send};
    #[repr(C)]
    struct Range {
        location: usize,
        length: usize,
    }
    unsafe impl Encode for Range {
        const ENCODING: Encoding = Encoding::Struct("_NSRange", &[usize::ENCODING, usize::ENCODING]);
    }
    const NOT_FOUND: usize = isize::MAX as usize;
    let Some(class) = AnyClass::get(c"NSSpellChecker") else { return (true, None) };
    unsafe {
        let checker: *mut AnyObject = msg_send![class, sharedSpellChecker];
        if checker.is_null() {
            return (true, None);
        }
        let named = language.map(objc2_foundation::NSString::from_str);
        let language: *mut AnyObject = match &named {
            Some(name) => objc2::rc::Retained::as_ptr(name).cast_mut().cast(),
            None => msg_send![checker, language],
        };
        let text = objc2_foundation::NSString::from_str(word);
        let null = std::ptr::null_mut::<isize>();
        let miss: Range = msg_send![
            checker,
            checkSpellingOfString: &*text,
            startingAt: 0isize,
            language: language,
            wrap: false,
            inSpellDocumentWithTag: 0isize,
            wordCount: null
        ];
        let whole = Range { location: 0, length: text.length() };
        let fixed: *mut objc2_foundation::NSString = msg_send![
            checker,
            correctionForWordRange: whole,
            inString: &*text,
            language: language,
            inSpellDocumentWithTag: 0isize
        ];
        (miss.location == NOT_FOUND, fixed.as_ref().map(|fixed| fixed.to_string()))
    }
}

/// Whether a word of `term` is unknown in every `SPELL_LANGUAGES`: what makes a respelling a
/// name or term (Klose, Pipedrive) and not ordinary editing.
fn spell_checker_unknown(term: &str) -> bool {
    term.split_whitespace().any(|word| SPELL_LANGUAGES.iter().all(|&language| !spelling(word, language).0))
}

/// Whether the spell checker corrects a word of `now` to one a single edit or swapped pair of
/// letters away ("calender", "recieved"): a typo, not a name. A correction back to what was pasted
/// is a typo too, unless the user changed its first letter (Klose for "close", `chose_name_over`).
fn spell_checker_typo(was: &str, now: &str) -> bool {
    let was = was.to_lowercase();
    now.split_whitespace().any(|word| {
        SPELL_LANGUAGES.iter().any(|&language| {
            spelling(word, language).1.is_some_and(|fixed| {
                let fixed = fixed.to_lowercase();
                (fixed != was || !super::learn::chose_name_over(&was, word)) && one_slip(&word.to_lowercase(), &fixed)
            })
        })
    })
}

/// One edit, or two neighbouring letters swapped.
fn one_slip(a: &str, b: &str) -> bool {
    if crate::dictionary::edit_distance(a, b) <= 1 {
        return true;
    }
    let (a, b): (Vec<char>, Vec<char>) = (a.chars().collect(), b.chars().collect());
    let differ: Vec<usize> = (0..a.len().min(b.len())).filter(|&i| a[i] != b[i]).collect();
    a.len() == b.len() && differ.len() == 2 && differ[1] == differ[0] + 1 && a[differ[0]] == b[differ[1]] && a[differ[1]] == b[differ[0]]
}

/// The text in paragraphs and lists from the formatter, when it is long enough to need them and
/// the formatter kept every word (`format::same_words`); otherwise the text as it was. Only the
/// outcome is logged, never the text.
fn lay_out(
    text: String,
    words: &[crate::model::WordTiming],
    formatter: Option<super::formatter::Formatter>,
) -> (String, Option<&'static str>) {
    use super::format;
    let Some(formatter) = formatter.filter(|_| format::worth_formatting(&text)) else { return (text, None) };
    let count = text.split_whitespace().count() as u32;
    let wait = (FORMAT_WAIT_BASE + FORMAT_WAIT_PER_WORD * count).min(FORMAT_WAIT_MAX);
    let started = Instant::now();
    let pauses = format::pause_hints(words);
    match formatter.format(&text, &pauses, wait).map(|laid_out| format::break_at_pauses(&laid_out, &pauses)) {
        Some(laid_out) if format::same_words(&text, &laid_out) => {
            scrawl_log(format!("Dictation: laid out in {} ms", started.elapsed().as_millis()));
            let described = format::describe(&laid_out);
            (laid_out, described)
        }
        Some(_) => {
            scrawl_log("Dictation: the formatter changed words; pasting the text as dictated");
            (text, None)
        }
        None => {
            scrawl_log(format!("Dictation: no layout after {} ms; pasting the text as dictated", started.elapsed().as_millis()));
            (text, None)
        }
    }
}

/// Pastes where a text field has focus and then puts the clipboard back; with nowhere to paste,
/// the text stays on the clipboard to paste by hand. An app that doesn't say gets the paste and
/// keeps the text on the clipboard too, so nothing is lost.
fn deliver(text: &str, restore_clipboard: bool, laid_out: Option<&'static str>) -> Outcome {
    let delivered = match route(crate::ax::focused_takes_text()) {
        Route::Paste => crate::handoff::paste_text(&fitted(text), restore_clipboard).map(|()| Outcome::Pasted(laid_out)),
        Route::Copy => crate::handoff::paste_text(text, false).map(|()| Outcome::Copied),
        Route::PasteAndKeep => crate::handoff::paste_text(&fitted(text), false).map(|()| Outcome::Pasted(laid_out)),
    };
    delivered.unwrap_or_else(|error| {
        scrawl_log(format!("Dictation: paste failed: {error:#}"));
        Outcome::Quiet
    })
}

/// UTF-16 units read on each side of the insertion point: two characters, even past an emoji's
/// pair.
const AROUND_UNITS: usize = 4;

/// `text` fitted to the focused field (`spacing::fit`): a space in front when it lands right after
/// a word, and the closer of an opening bracket it lands right after. A field Accessibility can't
/// read (some web and Electron editors) gets the text unchanged, rather than a guess.
fn fitted(text: &str) -> String {
    let field = crate::ax::FocusedText::current();
    match field.and_then(|field| field.around_insertion(AROUND_UNITS, MAX_FIELD_CHARS)) {
        Some((before, selected, after)) => {
            super::spacing::fit(text, &super::spacing::Around { before: &before, selected, after: after.as_deref() })
        }
        None => text.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle() -> PillState<'static> {
        PillState {
            working: false,
            dictating: None,
            capture_busy: false,
            notice: None,
            enabled: true,
            pill: true,
            hovered: false,
            chord: "⌃⌥",
            copy: false,
        }
    }

    #[test]
    fn the_pill_shows_the_most_pressing_state() {
        assert_eq!(presentation(&idle()), Pres::DictationIdle);
        let hint = Pres::Hint {
            dictate: "Dictate ⌃⌥".into(),
            copy: false,
        };
        assert_eq!(presentation(&PillState { hovered: true, ..idle() }), hint);
        let copy = Pres::Hint { dictate: "Dictate ⌃⌥".into(), copy: true };
        assert_eq!(presentation(&PillState { hovered: true, copy: true, ..idle() }), copy);
        let copied = PillState { notice: Some(("Copied to the clipboard", false)), hovered: true, ..idle() };
        assert!(matches!(presentation(&copied), Pres::Message { undo: false, .. }));
        let learned = PillState { notice: Some(("Learned your formatting", true)), ..idle() };
        assert!(matches!(presentation(&learned), Pres::Message { undo: true, .. }));
        let listening = PillState { dictating: Some(true), notice: Some(("x", true)), ..idle() };
        assert_eq!(presentation(&listening), Pres::Dictating { latched: true });
        let both = PillState { working: true, dictating: Some(false), ..idle() };
        assert_eq!(presentation(&both), Pres::Processing);
    }

    #[test]
    fn the_idle_pill_hides_for_captures_and_when_off() {
        let capture = PillState { capture_busy: true, notice: Some(("x", false)), ..idle() };
        assert_eq!(presentation(&capture), Pres::Hidden);
        assert_eq!(presentation(&PillState { pill: false, ..idle() }), Pres::Hidden);
        assert_eq!(presentation(&PillState { enabled: false, hovered: true, ..idle() }), Pres::Hidden);
        // A notice still shows with the idle pill off: it reports a dictation that just ran.
        let notice = PillState { pill: false, enabled: false, notice: Some(("x", false)), ..idle() };
        assert!(matches!(presentation(&notice), Pres::Message { .. }));
    }

    #[test]
    fn only_a_definite_no_keeps_the_text_off_the_app() {
        assert_eq!(route(Some(true)), Route::Paste);
        assert_eq!(route(Some(false)), Route::Copy);
        assert_eq!(route(None), Route::PasteAndKeep);
        assert_eq!(Outcome::Copied.notice(), Some("Copied to the clipboard"));
        assert_eq!(Outcome::NoSpeech.notice(), Some("Didn't catch that"));
        assert_eq!(Outcome::Pasted(Some("Formatted as a list")).notice(), None, "a layout's notice is a setting's");
        assert_eq!(Outcome::Quiet.notice(), None);
        assert!(one_slip("calender", "calendar") && one_slip("recieved", "received"));
        assert!(!one_slip("klose", "klaus"));
    }
}

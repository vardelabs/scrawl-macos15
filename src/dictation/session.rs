//! The running side of dictation on macOS: the microphone, the pill, and the transcribe-and-paste
//! that follows. Driven from the UI loop's 8 ms tick, so it is on the main thread.

use std::time::{Duration, Instant};

use super::chord::{Action, Chord, Mods};
use crate::audio::{AudioRecorder, RecordedAudio};
use crate::capture::{
    FLAG_ALTERNATE, FLAG_COMMAND, FLAG_CONTROL, FLAG_SHIFT, SharedCaptureTelemetry, scrawl_log,
};
use crate::native::pill::{NativePill, PillAction, PillSnapshot, PillStyle, model::Pres};

/// Less audio than this is a slip of the chord, not speech.
const MIN_AUDIO_MS: u64 = 300;
/// Settings are read from `config.json` this often, not on every tick.
const SETTINGS_EVERY: Duration = Duration::from_millis(500);
/// How long a notice ("Copied to the clipboard") stays in the pill.
const NOTICE_FOR: Duration = Duration::from_millis(2_500);

struct Settings {
    enabled: bool,
    mask: u64,
    /// The chord as the hint shows it (`⌃⌥`).
    label: String,
    restore_clipboard: bool,
    pill: bool,
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
    Pasted,
    /// Nowhere to paste: the text is on the clipboard.
    Copied,
    NoSpeech,
    /// Too short to be speech, or it failed (logged): nothing to say.
    Quiet,
}

struct Notice {
    text: &'static str,
    until: Instant,
}

pub struct Dictation {
    chord: Chord,
    clock: Instant,
    settings: Settings,
    settings_read: Instant,
    recorder: Option<AudioRecorder>,
    started: Option<Instant>,
    playback: Option<crate::playback::Guard>,
    pill: Option<NativePill>,
    shown: Option<PillSnapshot>,
    /// Transcribing and pasting on a worker; the next dictation waits for it.
    working: Option<flume::Receiver<Outcome>>,
    notice: Option<Notice>,
}

impl Default for Dictation {
    fn default() -> Self {
        Self {
            chord: Chord::default(),
            clock: Instant::now(),
            settings: Settings::load(),
            settings_read: Instant::now(),
            recorder: None,
            started: None,
            playback: None,
            pill: None,
            shown: None,
            working: None,
            notice: None,
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

    /// `capture_busy`: a capture is recording, selecting or processing, so the chord is ignored
    /// and the idle bar hides (it would be in the recording).
    pub fn tick(&mut self, telemetry: &SharedCaptureTelemetry, capture_busy: bool) {
        if !self.chord.is_active() && self.settings_read.elapsed() >= SETTINGS_EVERY {
            self.settings = Settings::load();
            self.settings_read = Instant::now();
            self.follow_pointer();
        }
        self.take_outcome();
        if self.settings.enabled {
            self.listen(telemetry, capture_busy);
        }
        let pres = self.presentation(capture_busy);
        if pres != Pres::Hidden {
            self.ensure_pill(telemetry);
        }
        self.show(pres);
    }

    fn listen(&mut self, telemetry: &SharedCaptureTelemetry, capture_busy: bool) {
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
        for pill_action in self.pill.as_ref().map(NativePill::drain_actions).unwrap_or_default() {
            let action = match pill_action {
                PillAction::Cancel => self.chord.cancel(),
                PillAction::Finish => self.chord.confirm(),
                PillAction::MessageClicked if self.notice.take().is_none() => {
                    self.start_hands_free(capture_busy, key_tap_live);
                    Action::None
                }
                _ => Action::None,
            };
            self.apply(action, capture_busy, key_tap_live);
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

    fn take_outcome(&mut self) {
        let Some(rx) = &self.working else {
            return;
        };
        let text = match rx.try_recv() {
            Err(flume::TryRecvError::Empty) => return,
            Ok(Outcome::Copied) => Some("Copied to the clipboard"),
            Ok(Outcome::NoSpeech) => Some("Didn't catch that"),
            Ok(Outcome::Pasted | Outcome::Quiet) | Err(flume::TryRecvError::Disconnected) => None,
        };
        self.working = None;
        self.notice = text.map(|text| Notice {
            text,
            until: Instant::now() + NOTICE_FOR,
        });
    }

    /// What the pill shows, most pressing first.
    fn presentation(&self, capture_busy: bool) -> Pres {
        if self.working.is_some() {
            return Pres::Processing;
        }
        if self.started.is_some() {
            return Pres::Dictating {
                latched: self.chord.is_latched(),
            };
        }
        if capture_busy {
            return Pres::Hidden;
        }
        if let Some(notice) = self.notice.as_ref().filter(|n| n.until > Instant::now()) {
            return Pres::Message {
                label: notice.text.to_owned(),
                notice: true,
            };
        }
        if !self.settings.enabled || !self.settings.pill {
            return Pres::Hidden;
        }
        if self.pill.as_ref().is_some_and(NativePill::pointer_on_pill) {
            return Pres::Message {
                label: format!("Click or hold {} to dictate", self.settings.label),
                notice: false,
            };
        }
        Pres::DictationIdle
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
                self.playback = self.playback.take().or_else(crate::playback::Guard::start);
                self.started = Some(Instant::now());
                self.notice = None;
                self.follow_pointer();
            }
            Action::Finish => self.finish(),
            // Dropping the recorder stops the microphone without encoding what it heard. The drop
            // waits for the device to finish opening, so it happens off the UI thread.
            Action::Discard => {
                if let Some(recorder) = self.recorder.take() {
                    std::thread::spawn(move || drop(recorder));
                }
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
        let (done, working) = flume::bounded(1);
        self.working = Some(working);
        let spawned = std::thread::Builder::new()
            .name("scrawl-dictation".into())
            .spawn(move || {
                let outcome = match recorder.stop() {
                    Ok(audio) if audio.duration_ms >= MIN_AUDIO_MS => {
                        transcribe_and_paste(&audio, restore)
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

    /// Opens the pill the first time it has something to show, and keeps it out of captures.
    fn ensure_pill(&mut self, telemetry: &SharedCaptureTelemetry) {
        if self.pill.is_some() {
            return;
        }
        match NativePill::open() {
            Ok(pill) => {
                telemetry
                    .excluded_window_ids
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .push(pill.window_number());
                self.pill = Some(pill);
                self.follow_pointer();
            }
            Err(error) => scrawl_log(format!("Dictation pill: {error}")),
        }
    }

    /// Keeps the pill on the display under the pointer, as Wispr's follows the active screen.
    fn follow_pointer(&self) {
        if let (Some(pill), Some([x, y])) = (&self.pill, crate::capture::current_mouse_screen()) {
            pill.place_at_bottom([f64::from(x), f64::from(y)]);
        }
    }

    fn show(&mut self, pres: Pres) {
        let Some(pill) = &self.pill else {
            return;
        };
        let elapsed_ms = self.started.map_or(0, |at| at.elapsed().as_millis() as u64);
        let snapshot = PillSnapshot {
            pres,
            clock: crate::model::format_clock(elapsed_ms),
            reduced_motion: false,
            style: PillStyle::current(),
        };
        if self.shown.as_ref() != Some(&snapshot) {
            pill.sync(snapshot.clone());
            self.shown = Some(snapshot);
        }
    }
}

fn transcribe_and_paste(audio: &RecordedAudio, restore_clipboard: bool) -> Outcome {
    let started = Instant::now();
    let wav = std::env::temp_dir().join(format!("scrawl-dictation-{}.wav", std::process::id()));
    if let Err(error) = std::fs::write(&wav, &audio.wav_bytes) {
        scrawl_log(format!("Dictation: could not write {}: {error}", wav.display()));
        return Outcome::Quiet;
    }
    let transcript = crate::transcribe::transcribe(Some(&wav), Some(audio));
    let _ = std::fs::remove_file(&wav);
    let text = crate::dictionary::Dictionary::load().apply(transcript.text.trim());
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
    deliver(&text, restore_clipboard)
}

/// Pastes where a text field has focus and then puts the clipboard back; with nowhere to paste,
/// the text stays on the clipboard to paste by hand. An app that doesn't say gets the paste and
/// keeps the text on the clipboard too, so nothing is lost.
fn deliver(text: &str, restore_clipboard: bool) -> Outcome {
    let delivered = match crate::ax::focused_takes_text() {
        Some(true) => crate::handoff::paste_text(text, restore_clipboard).map(|()| Outcome::Pasted),
        Some(false) => crate::handoff::copy_string_to_clipboard(text).map(|()| Outcome::Copied),
        None => crate::handoff::paste_text(text, false).map(|()| Outcome::Pasted),
    };
    delivered.unwrap_or_else(|error| {
        scrawl_log(format!("Dictation: paste failed: {error:#}"));
        Outcome::Quiet
    })
}

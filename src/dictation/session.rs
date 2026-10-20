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

struct Settings {
    enabled: bool,
    mask: u64,
    restore_clipboard: bool,
}

impl Settings {
    fn load() -> Self {
        let (enabled, shortcut, restore_clipboard) = crate::handoff::dictation();
        let mods = shortcut
            .as_deref()
            .and_then(super::parse_shortcut)
            .or_else(|| super::parse_shortcut(super::DEFAULT_SHORTCUT))
            .expect("the default dictation chord parses");
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
            enabled,
            mask,
            restore_clipboard,
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
    working: Option<flume::Receiver<()>>,
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

    /// `capture_busy`: a capture is recording, selecting or processing, so the chord is ignored.
    pub fn tick(&mut self, telemetry: &SharedCaptureTelemetry, capture_busy: bool) {
        if !self.chord.is_active() && self.settings_read.elapsed() >= SETTINGS_EVERY {
            self.settings = Settings::load();
            self.settings_read = Instant::now();
        }
        if self.working.as_ref().is_some_and(|rx| !rx.is_empty() || rx.is_disconnected()) {
            self.working = None;
            self.show(Pres::Hidden);
        }
        if !self.settings.enabled {
            return;
        }
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
                _ => Action::None,
            };
            self.apply(action, capture_busy, key_tap_live);
        }
        if self.started.is_some() && self.chord.is_latched() {
            self.show(Pres::Dictating { latched: true });
        } else if self.started.is_some() {
            self.show(Pres::Dictating { latched: false });
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
                self.playback = self.playback.take().or_else(crate::playback::Guard::start);
                self.started = Some(Instant::now());
                self.place_pill();
            }
            Action::Finish => self.finish(),
            // Dropping the recorder stops the microphone without encoding what it heard. The drop
            // waits for the device to finish opening, so it happens off the UI thread.
            Action::Discard => {
                if let Some(recorder) = self.recorder.take() {
                    std::thread::spawn(move || drop(recorder));
                }
                self.stop_listening(Pres::Hidden);
            }
        }
    }

    /// Stops ducking playback and shows `pres` in place of the listening pill.
    fn stop_listening(&mut self, pres: Pres) {
        self.started = None;
        self.playback = None;
        self.show(pres);
    }

    /// Stopping the recorder joins its worker (which may still be opening the device) and encodes
    /// the WAV, so it happens on the worker with the transcription, not on the UI thread.
    fn finish(&mut self) {
        let Some(recorder) = self.recorder.take() else {
            self.stop_listening(Pres::Hidden);
            return;
        };
        self.stop_listening(Pres::Processing);
        let restore = self.settings.restore_clipboard;
        let (done, working) = flume::bounded(1);
        self.working = Some(working);
        let spawned = std::thread::Builder::new()
            .name("scrawl-dictation".into())
            .spawn(move || {
                match recorder.stop() {
                    Ok(audio) if audio.duration_ms >= MIN_AUDIO_MS => {
                        transcribe_and_paste(&audio, restore);
                    }
                    Ok(_) => {}
                    Err(error) => scrawl_log(format!("Dictation: {error:#}")),
                }
                let _ = done.send(());
            });
        if let Err(error) = spawned {
            scrawl_log(format!("Dictation: could not start transcribing: {error}"));
            self.working = None;
            self.show(Pres::Hidden);
        }
    }

    fn place_pill(&mut self) {
        if self.pill.is_none() {
            match NativePill::open() {
                Ok(pill) => self.pill = Some(pill),
                Err(error) => scrawl_log(format!("Dictation pill: {error}")),
            }
        }
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

fn transcribe_and_paste(audio: &RecordedAudio, restore_clipboard: bool) {
    let started = Instant::now();
    let wav = std::env::temp_dir().join(format!("scrawl-dictation-{}.wav", std::process::id()));
    if let Err(error) = std::fs::write(&wav, &audio.wav_bytes) {
        scrawl_log(format!("Dictation: could not write {}: {error}", wav.display()));
        return;
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
        return;
    }
    if let Err(error) = crate::handoff::paste_text(&text, restore_clipboard) {
        scrawl_log(format!("Dictation: paste failed: {error:#}"));
    }
}

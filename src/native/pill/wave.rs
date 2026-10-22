//! The dictation pill's mic level line: a thin wave that is flat in silence and moves with the
//! voice, so the user can see the mic hears them. The newest level sits at the centre and older
//! levels ripple outward; the line fades into the capsule at both ends.

use super::model::{CHIP_H, FrameElem, Pres, Silhouette, SpringCfg, SpringVal};

/// Levels kept for the line, newest first, plus the live level ahead of them.
pub const SAMPLES: usize = 48;
const PUSH_HZ: f32 = 40.0;
/// A quiet room sits below the floor, so silence draws flat. A louder room (a fan, music) raises
/// it: the floor follows the quietest recent level, rising slowly through speech and dropping at
/// once to a quieter one, and the line starts `NOISE_MARGIN_DB` above it.
const FLOOR_DB: f32 = -52.0;
const SPAN_DB: f32 = 34.0;
const NOISE_RISE_DB_PER_S: f32 = 6.0;
const NOISE_MARGIN_DB: f32 = 4.0;
/// The noise estimate before the mic has heard the room: low enough that the floor is
/// `FLOOR_DB`. It carries across dictations, since the room does, so the 250 ms the idle bar
/// shows a hold's line is not spent relearning a fan.
const QUIET_NOISE_DB: f32 = FLOOR_DB - NOISE_MARGIN_DB;
/// A syllable lifts the line at once; it settles without flicker.
const ATTACK_S: f32 = 0.04;
const RELEASE_S: f32 = 0.14;
/// How fast the crests travel, in radians per second.
const PHASE_SPEED: f32 = 5.5;
const STILL: f32 = 1e-3;
const FADE: SpringCfg = SpringCfg::new(0.2, 0.0);

/// In the listening pill the line follows the clock inside the lead element.
pub const LISTENING_W: f32 = 56.0;
pub const LISTENING_GAP: f32 = 6.0;
const LISTENING_AMP: f32 = 7.0;
const LISTENING_STROKE: f32 = 1.5;
const LISTENING_WAVELENGTH: f32 = 11.0;
/// In the idle bar (`DICTATION_IDLE_W` × `DICTATION_IDLE_H`) the line is shorter and lower.
const IDLE_W: f32 = 30.0;
const IDLE_AMP: f32 = 2.4;
const IDLE_STROKE: f32 = 1.25;
const IDLE_WAVELENGTH: f32 = 7.0;

/// The lead's clock starts this far in, after the dot (`emit_elem_prims`).
pub const CLOCK_X: f32 = CHIP_H - 2.0;

pub fn db_of_rms(rms: f32) -> f32 {
    20.0 * rms.max(1e-6).log10()
}

/// A level in dBFS as the line's height, 0..1, above a noise floor.
fn height_of(db: f32, noise_db: f32) -> f32 {
    let floor = FLOOR_DB.max(noise_db + NOISE_MARGIN_DB);
    ((db - floor) / SPAN_DB).clamp(0.0, 1.0)
}

/// The inverse of `height_of` in a quiet room, for a pretend voice (the pill lab).
pub fn rms_of_height(height: f32) -> f32 {
    10_f32.powf((FLOOR_DB + height.clamp(0.0, 1.0) * SPAN_DB) / 20.0)
}

/// One line to draw, in panel points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveLine {
    /// `[x, y, w, h]`: the line runs across `w` and swings within `h` about the middle.
    pub rect: [f32; 4],
    pub alpha: f32,
    pub blur: f32,
    pub stroke: f32,
    pub wavelength: f32,
}

#[derive(Clone, Debug)]
pub struct Wave {
    /// The mic's newest level in dBFS, or `None` while it is closed.
    db: Option<f32>,
    noise_db: f32,
    target: f32,
    level: f32,
    history: [f32; SAMPLES - 1],
    since_push: f32,
    phase: f32,
    mic: SpringVal,
    listening: SpringVal,
    idle: SpringVal,
}

impl Default for Wave {
    fn default() -> Self {
        Self {
            db: None,
            noise_db: QUIET_NOISE_DB,
            target: 0.0,
            level: 0.0,
            history: [0.0; SAMPLES - 1],
            since_push: 0.0,
            phase: 0.0,
            mic: SpringVal::new(0.0, FADE),
            listening: SpringVal::new(0.0, FADE),
            idle: SpringVal::new(0.0, FADE),
        }
    }
}

impl Wave {
    /// The mic's RMS (16 kHz mono, -1..1), or `None` while it is closed.
    pub fn set_rms(&mut self, rms: Option<f32>) {
        self.db = rms.map(db_of_rms);
        // The new height is known now, not on the next `advance`: after a silence the pill has
        // stopped drawing, and only `is_moving` seeing the voice starts it again.
        self.target = self
            .db
            .map_or(0.0, |db| height_of(db, self.noise_db.min(db)));
        self.mic.to(f32::from(u8::from(rms.is_some())), FADE);
    }

    /// Only dictation's own states carry the line: a capture's lead never does.
    pub fn show(&mut self, pres: &Pres) {
        let listening = matches!(pres, Pres::Dictating { .. });
        let idle = matches!(pres, Pres::DictationIdle);
        self.listening.to(f32::from(u8::from(listening)), FADE);
        self.idle.to(f32::from(u8::from(idle)), FADE);
    }

    /// Under Reduce Motion the crests hold still; the height still follows the voice.
    pub fn advance(&mut self, dt: f32, reduced: bool) {
        self.target = match self.db {
            Some(db) => {
                self.noise_db = db.min(self.noise_db + NOISE_RISE_DB_PER_S * dt);
                height_of(db, self.noise_db)
            }
            None => 0.0,
        };
        let tau = if self.target > self.level { ATTACK_S } else { RELEASE_S };
        self.level += (self.target - self.level) * (1.0 - (-dt / tau).exp());
        if !reduced {
            self.phase = (self.phase + dt * PHASE_SPEED) % std::f32::consts::TAU;
        }
        self.since_push += dt * PUSH_HZ;
        while self.since_push >= 1.0 {
            self.since_push -= 1.0;
            self.history.copy_within(..SAMPLES - 2, 1);
            self.history[0] = self.level;
        }
        for spring in [&mut self.mic, &mut self.listening, &mut self.idle] {
            spring.step(dt);
        }
    }

    /// Something on the line would change on the next frame, so the pill keeps drawing; a flat
    /// line in silence, or one no state shows, draws nothing new.
    pub fn is_moving(&self) -> bool {
        let fading = !(self.mic.is_resting() && self.listening.is_resting() && self.idle.is_resting());
        let shown = self.mic.x > STILL && (self.listening.x > STILL || self.idle.x > STILL);
        let heard = self.target > STILL
            || self.level > STILL
            || self.history.iter().any(|&h| h > STILL);
        fading || (shown && heard)
    }

    /// The live level, then the history, newest first, for the shader.
    pub fn samples(&self) -> [f32; SAMPLES] {
        let mut out = [0.0; SAMPLES];
        out[0] = self.level;
        out[1..].copy_from_slice(&self.history);
        out
    }

    /// How far the history has moved toward its next slot (0..1), so it slides smoothly.
    pub fn scroll(&self) -> f32 {
        self.since_push
    }

    pub fn phase(&self) -> f32 {
        self.phase
    }

    /// The lines to draw this frame: one inside the listening pill's lead, after the clock, and
    /// one in the idle bar while the mic is open before the pill grows. They cross-fade as the
    /// pill changes.
    pub fn lines(&self, shape: &Silhouette, lead: Option<&FrameElem>, clock_w: f32) -> Vec<WaveLine> {
        let mut lines = Vec::with_capacity(2);
        let mic = self.mic.x.clamp(0.0, 1.0);
        if let Some(lead) = lead {
            let alpha = lead.alpha * self.listening.x.clamp(0.0, 1.0) * mic;
            if alpha > STILL {
                let k = lead.scale;
                let x = lead.rect[0] + (CLOCK_X + clock_w + LISTENING_GAP) * k;
                let cy = lead.rect[1] + lead.rect[3] * 0.5;
                lines.push(WaveLine {
                    rect: [x, cy - LISTENING_AMP * k, LISTENING_W * k, 2.0 * LISTENING_AMP * k],
                    alpha,
                    blur: lead.blur,
                    stroke: LISTENING_STROKE * k,
                    wavelength: LISTENING_WAVELENGTH * k,
                });
            }
        }
        let alpha = shape.alpha * self.idle.x.clamp(0.0, 1.0) * mic;
        if alpha > STILL {
            let cx = (shape.l + shape.r) * 0.5;
            let cy = (shape.t + shape.b) * 0.5;
            let w = IDLE_W.min(shape.width() - shape.height());
            lines.push(WaveLine {
                rect: [cx - w * 0.5, cy - IDLE_AMP, w, 2.0 * IDLE_AMP],
                alpha,
                blur: shape.blur,
                stroke: IDLE_STROKE,
                wavelength: IDLE_WAVELENGTH,
            });
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(wave: &mut Wave, seconds: f32) {
        for _ in 0..(seconds * 120.0) as usize {
            wave.advance(1.0 / 120.0, false);
        }
    }

    /// Levels measured on real captures: a quiet room, a fan, and speech.
    const QUIET: f32 = 0.0016; // -56 dBFS
    const FAN: f32 = 0.0071; // -43 dBFS
    const SPEECH: f32 = 0.032; // -30 dBFS

    #[test]
    fn a_quiet_room_is_flat_and_speech_fills_the_line() {
        assert_eq!(height_of(db_of_rms(0.0), QUIET_NOISE_DB), 0.0);
        assert_eq!(height_of(db_of_rms(QUIET), QUIET_NOISE_DB), 0.0);
        let speech = height_of(db_of_rms(SPEECH), QUIET_NOISE_DB);
        assert!(speech > 0.5 && speech < 1.0, "{speech}");
        assert_eq!(height_of(db_of_rms(0.5), QUIET_NOISE_DB), 1.0);
        assert!((height_of(db_of_rms(rms_of_height(0.6)), QUIET_NOISE_DB) - 0.6).abs() < 1e-3);
    }

    #[test]
    fn a_noisy_room_settles_flat_and_speech_still_shows() {
        let mut wave = Wave::default();
        wave.set_rms(Some(FAN));
        run(&mut wave, 3.0);
        assert!(wave.samples()[0] < 0.01, "{}", wave.samples()[0]);
        wave.set_rms(Some(SPEECH));
        run(&mut wave, 0.1);
        assert!(wave.samples()[0] > 0.2, "{}", wave.samples()[0]);
    }

    #[test]
    fn the_next_dictation_remembers_a_noisy_room() {
        let mut wave = Wave::default();
        wave.show(&Pres::DictationIdle);
        wave.set_rms(Some(FAN));
        run(&mut wave, 3.0);
        wave.set_rms(None);
        run(&mut wave, 5.0);
        wave.set_rms(Some(FAN));
        run(&mut wave, 0.25);
        assert!(wave.samples()[0] < 0.01, "{}", wave.samples()[0]);
    }

    #[test]
    fn a_syllable_lifts_the_line_fast_and_it_settles_slower() {
        let mut wave = Wave::default();
        wave.set_rms(Some(rms_of_height(1.0)));
        run(&mut wave, 0.08);
        let risen = wave.samples()[0];
        assert!(risen > 0.8, "{risen}");
        wave.set_rms(Some(QUIET));
        run(&mut wave, 0.08);
        let fallen = wave.samples()[0];
        assert!(fallen > 0.4, "release outlasts attack: {fallen}");
    }

    #[test]
    fn older_levels_move_outward_and_silence_stops_the_redraws() {
        let mut wave = Wave::default();
        wave.show(&Pres::Dictating { latched: false });
        wave.set_rms(Some(rms_of_height(1.0)));
        run(&mut wave, 0.2);
        wave.set_rms(Some(QUIET));
        run(&mut wave, 0.3);
        let samples = wave.samples();
        assert!(samples[0] < 0.2 && samples[12..].iter().any(|&s| s > 0.8));
        assert!(wave.is_moving());
        run(&mut wave, 3.0);
        assert!(!wave.is_moving(), "a flat line in silence draws nothing new");
    }

    #[test]
    fn speech_after_a_silence_wakes_the_line() {
        let mut wave = Wave::default();
        wave.show(&Pres::Dictating { latched: false });
        wave.set_rms(Some(QUIET));
        run(&mut wave, 3.0);
        assert!(!wave.is_moving());
        wave.set_rms(Some(SPEECH));
        assert!(wave.is_moving(), "the voice restarts drawing before the next frame");
    }

    #[test]
    fn a_line_no_state_shows_draws_nothing_new() {
        let mut wave = Wave::default();
        wave.show(&Pres::Hidden);
        wave.set_rms(Some(SPEECH));
        run(&mut wave, 0.5);
        assert!(!wave.is_moving());
    }

    #[test]
    fn the_idle_bar_shows_the_line_only_while_the_mic_is_open() {
        let shape = Silhouette { l: 298.0, r: 342.0, t: 22.0, b: 29.0, radius: 3.5, alpha: 1.0, blur: 0.0 };
        let mut wave = Wave::default();
        wave.show(&Pres::DictationIdle);
        run(&mut wave, 0.5);
        assert!(wave.lines(&shape, None, 29.0).is_empty());
        wave.set_rms(Some(SPEECH));
        run(&mut wave, 0.3);
        let lines = wave.lines(&shape, None, 29.0);
        assert_eq!(lines.len(), 1);
        let [x, _, w, _] = lines[0].rect;
        assert!(x > shape.l && x + w < shape.r, "the line stays inside the bar");
        wave.show(&Pres::Hidden);
        run(&mut wave, 0.5);
        assert!(wave.lines(&shape, None, 29.0).is_empty());
    }
}

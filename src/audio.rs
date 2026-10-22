use std::io::Cursor;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, anyhow, bail, ensure};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};

const SAMPLE_RATE: u32 = 16_000;
/// The pill's mic level covers this much of the newest audio: about one 30 fps frame.
const LEVEL_WINDOW: usize = SAMPLE_RATE as usize * 32 / 1_000;

#[derive(Clone, Debug)]
pub struct RecordedAudio {
    pub sample_count: usize,
    pub duration_ms: u64,
    pub wav_bytes: Vec<u8>,
    /// The transcription that ran while recording, if one did (`live_transcribe`).
    pub live: crate::live_transcribe::Slot,
    /// The command transcriber started with the recording, if `transcribePrestart` is on.
    pub prestarted: crate::transcribe::PrestartSlot,
}

pub struct AudioRecorder {
    worker: Option<std::thread::JoinHandle<Result<()>>>,
    stop_sender: Option<std::sync::mpsc::Sender<()>>,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    live: Option<crate::live_transcribe::Live>,
    prestarted: Option<crate::command_transcriber::Prestarted>,
}

impl AudioRecorder {
    pub fn start() -> Result<Self> {
        Self::start_pausable(None)
    }

    /// Like `start`, but drops what the microphone hears while `pause` is paused, so the WAV
    /// (and the transcribers reading it) holds the active audio back to back.
    pub fn start_pausable(pause: Option<Arc<crate::pause::PauseClock>>) -> Result<Self> {
        let mut recorder = Self::start_mic_with(pause)?;
        recorder.prestart_transcriber();
        Ok(recorder)
    }

    /// The microphone alone. Dictation starts it on every press of its chord, many of which
    /// turn out to be other shortcuts, so the transcribers wait for `prestart_transcriber`.
    pub fn start_mic() -> Result<Self> {
        Self::start_mic_with(None)
    }

    fn start_mic_with(pause: Option<Arc<crate::pause::PauseClock>>) -> Result<Self> {
        let started = std::time::Instant::now();
        let samples = Arc::new(Mutex::new(Vec::new()));
        let error = Arc::new(Mutex::new(None));
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker_samples = Arc::clone(&samples);
        let worker_error = Arc::clone(&error);
        let worker = std::thread::Builder::new()
            .name("scrawl-audio".into())
            .spawn(move || {
                let stream = Self::open_stream(worker_samples, worker_error, pause).inspect_err(
                    |error| {
                        crate::capture::scrawl_log(format!("Microphone startup failed: {error:#}"));
                    },
                )?;
                crate::ui::start_metric("audio_ready_ms", started);
                // Stream never crosses threads. Disconnect also stops abandoned recorders.
                let _ = stop_receiver.recv();
                drop(stream);
                Ok(())
            })
            .context("Could not spawn microphone worker")?;
        Ok(Self {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error,
            live: None,
            prestarted: None,
        })
    }

    /// The live transcriber and the prestarted command transcriber, whichever are configured.
    pub fn prestart_transcriber(&mut self) {
        if self.live.is_none() && crate::live_transcribe::wanted() {
            self.live = crate::live_transcribe::Live::start(Arc::clone(&self.samples))
                .inspect_err(|error| {
                    crate::capture::scrawl_log(format!("Live transcription: {error:#}"));
                })
                .ok();
        }
        if self.prestarted.is_none() {
            self.prestarted = crate::transcribe::prestart();
        }
    }

    /// The RMS of the newest audio, for the pill's level line; 0 before the device delivers any.
    pub fn rms(&self) -> f32 {
        let Ok(samples) = self.samples.lock() else {
            return 0.0;
        };
        rms_of(&samples[samples.len().saturating_sub(LEVEL_WINDOW)..])
    }

    /// The chosen microphone (`mic`), else the macOS default. A chosen device that is missing or
    /// won't open never stops the recording.
    fn open_stream(
        samples: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
        pause: Option<Arc<crate::pause::PauseClock>>,
    ) -> Result<cpal::Stream> {
        let choice = crate::mic::choice();
        if let Some(choice) = &choice {
            if let Some(device) = crate::mic::device(&choice.id) {
                let input = crate::mic::Input {
                    id: choice.id.clone(),
                    name: device_name(&device).unwrap_or_else(|| choice.name.clone()),
                };
                crate::mic::note_pick(&crate::mic::Pick::Chosen(&input));
                match Self::open_device(&device, samples.clone(), error.clone(), pause.clone()) {
                    Ok(stream) => {
                        crate::capture::scrawl_log(format!("Microphone: {}", input.name));
                        return Ok(stream);
                    }
                    Err(e) => crate::capture::scrawl_log(format!(
                        "Microphone: could not open {} ({e:#}); using the default",
                        input.name
                    )),
                }
            } else {
                let default = cpal::default_host()
                    .default_input_device()
                    .and_then(|device| {
                        Some(crate::mic::Input {
                            id: device.id().ok()?.id().to_owned(),
                            name: device_name(&device)?,
                        })
                    });
                let inputs = crate::mic::Inputs {
                    default: default.as_ref().map(|d| d.id.clone()),
                    devices: default.into_iter().collect(),
                };
                crate::mic::note_pick(&crate::mic::Pick::new(Some(choice), &inputs));
            }
        }
        let device = cpal::default_host()
            .default_input_device()
            .context("No microphone is available")?;
        let stream = Self::open_device(&device, samples, error, pause)?;
        crate::capture::scrawl_log(format!(
            "Microphone: {} (default)",
            device_name(&device).unwrap_or_default()
        ));
        Ok(stream)
    }

    fn open_device(
        device: &cpal::Device,
        samples: Arc<Mutex<Vec<f32>>>,
        error: Arc<Mutex<Option<String>>>,
        pause: Option<Arc<crate::pause::PauseClock>>,
    ) -> Result<cpal::Stream> {
        let supported = device.default_input_config()?;
        let config = supported.config();
        crate::capture::scrawl_log(format!(
            "Microphone: {} Hz, {} ch, {:?}",
            config.sample_rate,
            config.channels,
            supported.sample_format()
        ));
        ensure!(
            config.channels > 0 && config.sample_rate > 0,
            "Invalid microphone format"
        );
        macro_rules! stream {
            ($ty:ty) => {
                input_stream::<$ty>(
                    &device,
                    config,
                    samples.clone(),
                    error.clone(),
                    pause.clone(),
                )?
            };
        }
        let stream = match supported.sample_format() {
            cpal::SampleFormat::F32 => stream!(f32),
            cpal::SampleFormat::F64 => stream!(f64),
            cpal::SampleFormat::I8 => stream!(i8),
            cpal::SampleFormat::I16 => stream!(i16),
            cpal::SampleFormat::I24 => stream!(cpal::I24),
            cpal::SampleFormat::I32 => stream!(i32),
            cpal::SampleFormat::I64 => stream!(i64),
            cpal::SampleFormat::U8 => stream!(u8),
            cpal::SampleFormat::U16 => stream!(u16),
            cpal::SampleFormat::U24 => stream!(cpal::U24),
            cpal::SampleFormat::U32 => stream!(u32),
            cpal::SampleFormat::U64 => stream!(u64),
            format => bail!("Unsupported microphone format: {format}"),
        };
        stream.play().context("Could not start microphone")?;
        Ok(stream)
    }

    fn join_worker(&mut self) -> Result<()> {
        self.stop_sender.take();
        if let Some(worker) = self.worker.take() {
            worker
                .join()
                .map_err(|_| anyhow!("Microphone worker panicked"))??;
        }
        Ok(())
    }

    pub fn stop(mut self) -> Result<RecordedAudio> {
        // Joining drops the stream and stops its callback before taking the buffer,
        // including a stop requested while the device was still being initialized.
        self.join_worker()?;
        let error = self
            .error
            .lock()
            .map_err(|_| anyhow!("Microphone error lock poisoned"))?
            .take();
        let samples_16k = std::mem::take(
            &mut *self
                .samples
                .lock()
                .map_err(|_| anyhow!("Microphone buffer lock poisoned"))?,
        );
        // An overload, or a device unplugged partway, keeps what was heard before it.
        if let Some(error) = error {
            if samples_16k.is_empty() {
                bail!("Microphone capture failed: {error}");
            }
            crate::capture::scrawl_log(format!("Microphone: {error}; keeping what it heard"));
        }
        let sample_count = samples_16k.len();
        let duration_ms = sample_count as u64 * 1_000 / SAMPLE_RATE as u64;
        let wav_bytes = encode_wav_16k(&samples_16k)?;
        Ok(RecordedAudio {
            sample_count,
            duration_ms,
            wav_bytes,
            live: crate::live_transcribe::Slot::new(self.live.take()),
            prestarted: crate::transcribe::PrestartSlot::new(self.prestarted.take()),
        })
    }
}

impl Drop for AudioRecorder {
    fn drop(&mut self) {
        let _ = self.join_worker();
    }
}

fn device_name(device: &cpal::Device) -> Option<String> {
    Some(device.description().ok()?.name().to_owned())
}

fn input_stream<T>(
    device: &cpal::Device,
    config: cpal::StreamConfig,
    samples: Arc<Mutex<Vec<f32>>>,
    error: Arc<Mutex<Option<String>>>,
    pause: Option<Arc<crate::pause::PauseClock>>,
) -> Result<cpal::Stream>
where
    T: cpal::SizedSample,
    f32: cpal::FromSample<T>,
{
    let channels = usize::from(config.channels);
    let mut downmix = Downmix::new(config.sample_rate);
    Ok(device.build_input_stream::<T, _, _>(
        config,
        move |data, _| {
            let paused = pause.as_ref().is_some_and(|p| p.is_paused());
            let Ok(mut output) = samples.lock() else {
                return;
            };
            downmix.push_frames(
                data.chunks_exact(channels).map(|frame| {
                    frame
                        .iter()
                        .map(|sample| sample.to_sample::<f32>() as f64)
                        .sum::<f64>()
                        / channels as f64
                }),
                paused,
                &mut output,
            );
        },
        move |err| {
            // A buffer under- or overrun drops a few samples; the recording goes on.
            if err.kind() == cpal::ErrorKind::Xrun {
                return;
            }
            if let Ok(mut error) = error.lock() {
                *error = Some(err.to_string());
            }
        },
        None,
    )?)
}

/// Mono samples at the device's rate in, 16 kHz out.
struct Downmix {
    rate: u32,
    phase: u32,
    sum: f64,
}

impl Downmix {
    fn new(rate: u32) -> Self {
        Self {
            rate,
            phase: 0,
            sum: 0.0,
        }
    }

    /// A paused buffer is dropped whole, with the part-built output sample, so nothing heard
    /// while paused leaks into the first sample after it.
    fn push_frames(&mut self, mono: impl Iterator<Item = f64>, paused: bool, out: &mut Vec<f32>) {
        if paused {
            self.phase = 0;
            self.sum = 0.0;
            return;
        }
        for mono in mono {
            let mono = if mono.is_finite() { mono } else { 0.0 };
            // Area averaging gives a continuous-phase downmix/decimator,
            // including non-integer ratios such as 44.1 kHz -> 16 kHz.
            let mut remaining = SAMPLE_RATE;
            while remaining > 0 {
                let weight = remaining.min(self.rate - self.phase);
                self.sum += mono * weight as f64;
                self.phase += weight;
                remaining -= weight;
                if self.phase == self.rate {
                    out.push((self.sum / self.rate as f64) as f32);
                    self.phase = 0;
                    self.sum = 0.0;
                }
            }
        }
    }
}

fn rms_of(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

/// Reads a 16 kHz mono WAV (as written by `scrawl-helper audio`) back into a recording.
pub fn recorded_audio_from_wav(wav_bytes: Vec<u8>) -> Result<RecordedAudio> {
    let reader = hound::WavReader::new(Cursor::new(&wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE && spec.channels == 1 && spec.bits_per_sample == 16,
        "Expected 16 kHz mono 16-bit audio"
    );
    let sample_count = reader.duration() as usize;
    Ok(RecordedAudio {
        sample_count,
        duration_ms: sample_count as u64 * 1_000 / u64::from(SAMPLE_RATE),
        wav_bytes,
        live: Default::default(),
        prestarted: Default::default(),
    })
}

/// Decodes a 16 kHz mono 16-bit WAV (this module's own format) into samples.
pub fn decode_wav_16k(wav_bytes: &[u8]) -> Result<Vec<f32>> {
    let reader = hound::WavReader::new(Cursor::new(wav_bytes))?;
    let spec = reader.spec();
    ensure!(
        spec.sample_rate == SAMPLE_RATE
            && spec.channels == 1
            && spec.bits_per_sample == 16
            && spec.sample_format == hound::SampleFormat::Int,
        "Expected 16 kHz mono 16-bit audio"
    );
    reader
        .into_samples::<i16>()
        .map(|sample| Ok(f32::from(sample?) / 32_768.0))
        .collect()
}

const ISLAND_WINDOW_MS: u64 = 50;
const ISLAND_MERGE_SILENCE_MS: u64 = 350;

/// Energy-based speech spans in milliseconds, quantized to 50 ms. These are not recognizer
/// timestamps. The speech threshold follows the recording's own noise floor (10th percentile of
/// window energy) so a quiet microphone and a noisy room both find their speech.
pub fn speech_islands(samples_16k: &[f32]) -> Vec<(u64, u64)> {
    let window = (u64::from(SAMPLE_RATE) * ISLAND_WINDOW_MS / 1_000) as usize;
    let rms: Vec<f64> = samples_16k
        .chunks(window)
        .map(|chunk| {
            (chunk
                .iter()
                .map(|&x| {
                    if x.is_finite() {
                        f64::from(x).powi(2)
                    } else {
                        0.0
                    }
                })
                .sum::<f64>()
                / chunk.len() as f64)
                .sqrt()
        })
        .collect();
    if rms.is_empty() {
        return Vec::new();
    }
    let mut sorted = rms.clone();
    sorted.sort_by(f64::total_cmp);
    let floor = sorted[sorted.len() / 10];
    let threshold = (floor * 3.0).clamp(0.005, 0.02);
    let mut islands: Vec<(u64, u64)> = Vec::new();
    for (index, &level) in rms.iter().enumerate() {
        if level <= threshold {
            continue;
        }
        let start = index as u64 * ISLAND_WINDOW_MS;
        let end = start + ISLAND_WINDOW_MS;
        match islands.last_mut() {
            Some(last) if start.saturating_sub(last.1) < ISLAND_MERGE_SILENCE_MS => last.1 = end,
            _ => islands.push((start, end)),
        }
    }
    let duration = samples_16k.len() as u64 * 1_000 / u64::from(SAMPLE_RATE);
    for island in &mut islands {
        island.1 = island.1.min(duration);
    }
    islands.retain(|&(a, b)| b > a);
    islands
}

/// One sample as `audio.wav` stores it.
pub fn pcm16(sample: f32) -> i16 {
    let sample = if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    };
    (sample * 32_768.0).round().clamp(-32_768.0, 32_767.0) as i16
}

pub fn encode_wav_16k(samples_16k: &[f32]) -> Result<Vec<u8>> {
    let mut bytes = Cursor::new(Vec::with_capacity(44 + samples_16k.len() * 2));
    {
        let mut writer = hound::WavWriter::new(
            &mut bytes,
            hound::WavSpec {
                channels: 1,
                sample_rate: SAMPLE_RATE,
                bits_per_sample: 16,
                sample_format: hound::SampleFormat::Int,
            },
        )?;
        let mut i16_writer = writer.get_i16_writer(samples_16k.len() as u32);
        for &sample in samples_16k {
            i16_writer.write_sample(pcm16(sample));
        }
        i16_writer.flush()?;
        writer.finalize()?;
    }
    Ok(bytes.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Feeds 10 ms buffers at 48 kHz of `level`, paused or not, as the microphone callback does.
    fn feed(downmix: &mut Downmix, out: &mut Vec<f32>, ms: u64, level: f64, paused: bool) {
        for _ in 0..ms / 10 {
            downmix.push_frames(std::iter::repeat_n(level, 480), paused, out);
        }
    }

    #[test]
    fn paused_audio_is_cut_and_the_rest_is_back_to_back() {
        let mut downmix = Downmix::new(48_000);
        let mut out = Vec::new();
        feed(&mut downmix, &mut out, 1_000, 0.25, false);
        feed(&mut downmix, &mut out, 5_000, 0.9, true);
        feed(&mut downmix, &mut out, 500, 0.25, false);
        assert_eq!(out.len(), 24_000, "1.5 s of active audio at 16 kHz");
        assert!(
            out.iter().all(|s| (s - 0.25).abs() < 1e-6),
            "nothing heard while paused leaks in"
        );
    }

    #[test]
    fn pauses_at_the_start_the_end_and_several_between_keep_only_active_audio() {
        let mut downmix = Downmix::new(44_100);
        let mut out = Vec::new();
        let mut active_ms = 0;
        for (ms, paused) in [
            (300, true),
            (700, false),
            (200, true),
            (1_100, false),
            (2_000, true),
            (400, false),
            (900, true),
        ] {
            for _ in 0..ms / 10 {
                downmix.push_frames(std::iter::repeat_n(0.5, 441), paused, &mut out);
            }
            if !paused {
                active_ms += ms;
            }
        }
        assert_eq!(active_ms, 2_200);
        // A pause drops the part-built sample, so each active run may come up one short.
        let expected = active_ms as usize * 16;
        assert!(
            (expected - 3..=expected).contains(&out.len()),
            "{} samples for {expected}",
            out.len()
        );
    }

    #[test]
    fn stop_joins_worker_before_reading_its_last_samples() {
        let samples = Arc::new(Mutex::new(Vec::new()));
        let output = Arc::clone(&samples);
        let (stop_sender, stop_receiver) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            // Model a final callback flushed while the worker drops its stream.
            assert!(matches!(
                stop_receiver.recv_timeout(Duration::from_secs(2)),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
            ));
            output.lock().unwrap().extend([0.1, 0.2, 0.3]);
            Ok(())
        });
        let audio = AudioRecorder {
            worker: Some(worker),
            stop_sender: Some(stop_sender),
            samples,
            error: Arc::new(Mutex::new(None)),
            live: None,
            prestarted: None,
        }
        .stop()
        .unwrap();
        assert_eq!(audio.sample_count, 3);
        assert_eq!(
            hound::WavReader::new(Cursor::new(audio.wav_bytes))
                .unwrap()
                .duration(),
            3
        );
    }

    #[test]
    fn a_device_lost_partway_keeps_what_it_heard() {
        let recorder = |samples: Vec<f32>| AudioRecorder {
            worker: None,
            stop_sender: None,
            samples: Arc::new(Mutex::new(samples)),
            error: Arc::new(Mutex::new(Some("Device disconnected".into()))),
            live: None,
            prestarted: None,
        };
        assert_eq!(recorder(vec![0.1; 160]).stop().unwrap().sample_count, 160);
        assert!(recorder(Vec::new()).stop().is_err());
    }

    #[test]
    fn stop_propagates_background_initialization_failure() {
        let recorder = AudioRecorder {
            worker: Some(std::thread::spawn(|| bail!("device unavailable"))),
            stop_sender: None,
            samples: Arc::new(Mutex::new(Vec::new())),
            error: Arc::new(Mutex::new(None)),
            live: None,
            prestarted: None,
        };
        assert!(
            recorder
                .stop()
                .unwrap_err()
                .to_string()
                .contains("device unavailable")
        );
    }

    #[test]
    fn speech_islands_merge_only_short_silences() {
        let mut samples = vec![0.0; 32_000];
        samples[1_600..4_800].fill(0.2); // 100..300
        samples[8_800..12_800].fill(0.2); // 550..800: merge 250 ms gap
        samples[18_400..20_800].fill(0.2); // 1150..1300: keep 350 ms gap
        assert_eq!(speech_islands(&samples), vec![(100, 800), (1_150, 1_300)]);
        assert!(speech_islands(&[0.001; 800]).is_empty());
        assert!(speech_islands(&[]).is_empty());
    }

    #[test]
    fn speech_islands_adapt_to_a_noisy_floor() {
        let mut samples: Vec<f32> = (0..32_000)
            .map(|i| if i % 2 == 0 { 0.006 } else { -0.006 })
            .collect();
        samples[8_000..16_000]
            .iter_mut()
            .for_each(|s| *s = if *s > 0.0 { 0.15 } else { -0.15 });
        assert_eq!(speech_islands(&samples), vec![(500, 1_000)]);
    }

    #[test]
    fn decoded_wav_round_trips() {
        let wav = encode_wav_16k(&[0.0, 0.5, -0.5]).unwrap();
        assert_eq!(decode_wav_16k(&wav).unwrap(), vec![0.0, 0.5, -0.5]);
        assert!(decode_wav_16k(b"nope").is_err());
    }

    #[test]
    fn wav_is_mono_16k_pcm_and_clips_safely() {
        let bytes = encode_wav_16k(&[-2.0, -0.5, 0.0, 0.5, 2.0, f32::NAN]).unwrap();
        let mut reader = hound::WavReader::new(Cursor::new(bytes)).unwrap();
        assert_eq!(reader.spec().channels, 1);
        assert_eq!(reader.spec().sample_rate, 16_000);
        assert_eq!(reader.spec().bits_per_sample, 16);
        assert_eq!(reader.spec().sample_format, hound::SampleFormat::Int);
        assert_eq!(
            reader
                .samples::<i16>()
                .collect::<std::result::Result<Vec<_>, _>>()
                .unwrap(),
            vec![-32_768, -16_384, 0, 16_384, 32_767, 0]
        );
    }
}

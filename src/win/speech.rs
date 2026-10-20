//! On-device transcription on Windows, with measured word timings: Parakeet through
//! `transcribe.cpp`, the engine Hex uses, loading the model Hex has already installed
//! (`%APPDATA%\voice-control\models`). Scrawl never downloads one. Like the Hex client, it
//! transcribes each speech island on its own; unlike it, every word keeps the time the model
//! heard it (`estimated: false`), so spoken cues ("look here") can cut keyframes.

use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};
use transcribe_cpp::{Model, RunOptions, TimestampKind};

use crate::model::{TranscriptData, TranscriptSegment, WordTiming};

const KHZ: u64 = 16;
/// Silence kept around each clip, so first and last words aren't clipped.
const CLIP_PAD_MS: u64 = 200;

fn models_dir() -> PathBuf {
    crate::hex::support_dir().join("models")
}

/// Hex's installed models (verified downloads), Hex's chosen one first, then Parakeet ones.
pub fn installed_models() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(models_dir()) else {
        return Vec::new();
    };
    let chosen = std::fs::read(crate::hex::support_dir().join("settings.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .and_then(|settings| {
            settings["transcription"]["model"]
                .as_str()
                .map(str::to_owned)
        })
        .map(|id| id.replace('_', "-"));
    let mut models: Vec<PathBuf> = entries
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| {
            path.extension().is_some_and(|ext| ext == "gguf")
                && path
                    .with_file_name(format!(
                        "{}.verified.json",
                        path.file_name().unwrap_or_default().to_string_lossy()
                    ))
                    .is_file()
        })
        .collect();
    let rank = |path: &PathBuf| {
        let name = path.file_name().unwrap_or_default().to_string_lossy();
        let picked = chosen.as_deref().is_some_and(|id| name.starts_with(id));
        (!picked, !name.starts_with("parakeet"), name.into_owned())
    };
    models.sort_by_key(rank);
    models
}

/// The model's name for `TranscriptData.model`: its file name without the extension.
fn model_name(path: &Path) -> String {
    path.file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_default()
}

pub fn transcribe(audio: &Path) -> Result<TranscriptData> {
    let Some(path) = installed_models().into_iter().next() else {
        bail!(
            "No transcription model: install Hex (https://github.com/anomalyco/hex) and download a model in it"
        );
    };
    let wav =
        std::fs::read(audio).with_context(|| format!("Could not read {}", audio.display()))?;
    let samples = crate::audio::decode_wav_16k(&wav)?;
    let duration_ms = samples.len() as u64 / KHZ;
    let mut clips = crate::hex::plan_clips(&crate::audio::speech_islands(&samples), duration_ms);
    if clips.is_empty() && duration_ms >= 480 {
        clips.push((0, duration_ms));
    }
    let name = model_name(&path);
    let mut transcript = TranscriptData {
        source: format!("parakeet:{name}"),
        model: Some(name),
        estimated: false,
        ..Default::default()
    };
    if clips.is_empty() {
        return Ok(transcript);
    }

    transcribe_cpp::disable_logging();
    transcribe_cpp::init_backends_default().context("transcribe.cpp backends")?;
    let model = Model::load(&path).with_context(|| format!("Could not load {}", path.display()))?;
    let mut session = model.session()?;
    let options = RunOptions {
        timestamps: TimestampKind::Word,
        language: Some("en".into()),
        ..Default::default()
    };
    let mut texts = Vec::new();
    for (start, end) in clips {
        let from_ms = start.saturating_sub(CLIP_PAD_MS);
        let from = (from_ms * KHZ) as usize;
        let to = (((end + CLIP_PAD_MS) * KHZ) as usize).min(samples.len());
        if from >= to {
            continue;
        }
        let result = session.run(&samples[from..to], &options)?;
        let text = result.text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            continue;
        }
        let at = |ms: i64| from_ms + ms.max(0) as u64;
        let words: Vec<WordTiming> = result
            .words
            .iter()
            .filter(|word| !word.text.trim().is_empty())
            .map(|word| WordTiming {
                t0: at(word.t0_ms),
                t1: at(word.t1_ms.max(word.t0_ms)),
                text: word.text.trim().to_owned(),
            })
            .collect();
        if words.is_empty() {
            // No word times from this model: spread the text over the clip, and say so.
            let (segments, words) = crate::hex::spread_words(&text, start, end);
            transcript.estimated = true;
            transcript.segments.extend(segments);
            transcript.words.extend(words);
        } else {
            let segments: Vec<TranscriptSegment> = result
                .segments
                .iter()
                .filter(|s| !s.text.trim().is_empty())
                .map(|s| TranscriptSegment {
                    t0: at(s.t0_ms),
                    t1: at(s.t1_ms.max(s.t0_ms)),
                    text: s.text.trim().to_owned(),
                })
                .collect();
            transcript.segments.extend(if segments.is_empty() {
                vec![TranscriptSegment {
                    t0: words[0].t0,
                    t1: words[words.len() - 1].t1,
                    text: text.clone(),
                }]
            } else {
                segments
            });
            transcript.words.extend(words);
        }
        texts.push(text);
    }
    transcript.text = texts.join(" ");
    Ok(transcript)
}

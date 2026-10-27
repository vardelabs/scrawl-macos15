#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["mlx-whisper>=0.4"]
# ///
"""Scrawl command transcriber: OpenAI Whisper large-v3 turbo on Apple silicon through mlx-whisper.

Reads the 16 kHz mono WAV Scrawl hands it and prints the transcript JSON Scrawl expects
(version 1; text, Whisper's segments, words with measured times in ms). Configure it in
~/.config/scrawl/config.json:

    "transcriber": "whisper",
    "whisperCommand": ["/Users/you/.local/share/uv/tools/mlx-whisper/bin/python",
                       "/path/to/scrawl-whisper.py", "{audio}"]

Add "--language", "nl" before "{audio}" to fix the language; without it Whisper detects it.

Needs mlx-whisper as a uv tool (brew install uv; uv tool install mlx-whisper). Run it once by
hand with that Python on any WAV before the first recording: that run downloads the model (about
1.6 GB) from Hugging Face, which can outlast Scrawl's timeout. Once the model is in the hub cache
the script runs offline. SCRAWL_WHISPER_MODEL picks another mlx-whisper model repo.

With "transcribePrestart": true, Scrawl starts this when a recording begins, with
SCRAWL_PRESTART=1 and "-" for the audio. It loads the model and runs it once on a second of
silence while the user talks, then transcribes the WAV whose path Scrawl sends on stdin. Empty
stdin means the recording was cancelled.
"""

import argparse
import glob
import json
import os
import sys
import wave

DEFAULT_MODEL = "mlx-community/whisper-large-v3-turbo"
MODEL_NAME = os.environ.get("SCRAWL_WHISPER_MODEL", DEFAULT_MODEL)
SAMPLE_RATE = 16000


def model_is_cached(name: str) -> bool:
    """config.json and the weights in one snapshot. A download cut off after config.json must
    not count, or every later run would go offline and fail on the missing weights."""
    hub = os.path.join(os.environ.get("HF_HOME", os.path.expanduser("~/.cache/huggingface")), "hub")
    snapshots = os.path.join(hub, "models--" + name.replace("/", "--"), "snapshots")
    return any(
        any(os.path.exists(os.path.join(snapshot, weights)) for weights in
            ("model.safetensors", "weights.safetensors", "weights.npz"))
        for snapshot in glob.glob(os.path.join(snapshots, "*", "config.json"))
        for snapshot in [os.path.dirname(snapshot)]
    )


# The hub client checks every cached file against huggingface.co on each load unless it is
# offline, which would put a network round trip into every recording.
if model_is_cached(MODEL_NAME):
    os.environ.setdefault("HF_HUB_OFFLINE", "1")

import mlx_whisper  # noqa: E402
import numpy as np  # noqa: E402


def ms(seconds: float) -> int:
    return max(0, round(seconds * 1000))


def read_wav(path: str) -> np.ndarray:
    """Scrawl's WAV is already what Whisper wants, so it is read directly: mlx-whisper would
    otherwise decode it with ffmpeg, which Scrawl.app's empty PATH does not find."""
    with wave.open(path, "rb") as audio:
        if (audio.getnchannels(), audio.getsampwidth(), audio.getframerate()) != (1, 2, SAMPLE_RATE):
            raise SystemExit("expected 16 kHz mono 16-bit audio")
        frames = audio.readframes(audio.getnframes())
    return np.frombuffer(frames, dtype="<i2").astype(np.float32) / 32768.0


def transcribe(audio: np.ndarray, language: str | None) -> dict:
    return mlx_whisper.transcribe(
        audio, path_or_hf_repo=MODEL_NAME, language=language, word_timestamps=True
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Scrawl command transcriber for Whisper.")
    parser.add_argument("--language", help="a Whisper language code; detected when absent")
    parser.add_argument("audio", help="the WAV, or - with SCRAWL_PRESTART=1")
    args = parser.parse_args()
    audio = args.audio
    if os.environ.get("SCRAWL_PRESTART") == "1":
        # The first pass in a process loads the weights onto the GPU, and mlx-whisper keeps
        # the model for the next call.
        transcribe(np.zeros(SAMPLE_RATE, dtype=np.float32), args.language)
        audio = sys.stdin.readline().strip()
        if not audio:
            return 0
    result = transcribe(read_wav(audio), args.language)
    segments = [
        {"t0": ms(segment["start"]), "t1": ms(segment["end"]), "text": segment["text"].strip()}
        for segment in result["segments"]
        if segment["text"].strip()
    ]
    words = [
        {"t0": ms(word["start"]), "t1": ms(word["end"]), "text": word["word"].strip()}
        for segment in result["segments"]
        for word in segment.get("words", [])
        if word["word"].strip()
    ]
    json.dump(
        {
            "version": 1,
            "model": MODEL_NAME.rsplit("/", 1)[-1],
            "text": result["text"].strip(),
            "segments": segments,
            "words": words,
            "estimated": False,
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

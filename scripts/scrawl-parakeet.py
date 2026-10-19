#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["parakeet-mlx>=0.5"]
# ///
"""Scrawl command transcriber: NVIDIA Parakeet TDT 0.6B v3 on Apple silicon through parakeet-mlx.

Reads the 16 kHz mono WAV Scrawl hands it and prints the transcript JSON Scrawl expects
(version 1; text, sentences as segments, words with measured times in ms). Configure it in
~/.config/scrawl/config.json:

    "transcriber": "command",
    "transcribeCommand": ["/opt/homebrew/bin/uv", "run", "--quiet", "--script",
                          "/path/to/scrawl-parakeet.py", "{audio}"]

Needs uv and ffmpeg (brew install uv ffmpeg). Run it once by hand on any WAV before the first
recording: that run resolves the dependencies and downloads the model (about 1.2 GB) from
Hugging Face, which can outlast Scrawl's timeout. Once the model is in the hub cache the script
runs offline. SCRAWL_PARAKEET_MODEL picks another parakeet-mlx model repo.
"""

import glob
import json
import os
import sys

DEFAULT_MODEL = "mlx-community/parakeet-tdt-0.6b-v3"
MODEL_NAME = os.environ.get("SCRAWL_PARAKEET_MODEL", DEFAULT_MODEL)
# Long recordings are decoded in windows so memory stays flat; the overlap keeps words whole.
CHUNK_SECONDS = 120.0
OVERLAP_SECONDS = 15.0

# Scrawl.app runs this without a shell PATH, and parakeet-mlx decodes audio by finding ffmpeg
# on it; Homebrew's prefixes are where a Mac has ffmpeg.
os.environ["PATH"] = os.pathsep.join(
    ["/opt/homebrew/bin", "/usr/local/bin", os.environ.get("PATH", "/usr/bin:/bin")]
)


def model_is_cached(name: str) -> bool:
    """Both files parakeet-mlx fetches, in one snapshot. A download cut off after config.json
    must not count, or every later run would go offline and fail on the missing weights."""
    hub = os.path.join(os.environ.get("HF_HOME", os.path.expanduser("~/.cache/huggingface")), "hub")
    snapshots = os.path.join(hub, "models--" + name.replace("/", "--"), "snapshots")
    return any(
        os.path.exists(os.path.join(snapshot, "model.safetensors"))
        for snapshot in glob.glob(os.path.join(snapshots, "*", "config.json"))
        for snapshot in [os.path.dirname(snapshot)]
    )


# The hub client checks every cached file against huggingface.co on each download call unless
# it is offline, which would put a network round trip into every recording.
if model_is_cached(MODEL_NAME):
    os.environ.setdefault("HF_HUB_OFFLINE", "1")

from parakeet_mlx import from_pretrained  # noqa: E402


def ms(seconds: float) -> int:
    return max(0, round(seconds * 1000))


def words_from_tokens(tokens):
    """Sentencepiece tokens carry a leading space on the first piece of a word, or a bare space
    token before pieces that never carry one (digits); punctuation attaches to the word before."""
    words = []
    boundary = True
    for token in tokens:
        piece = token.text.strip()
        if not piece:
            boundary = True
            continue
        if boundary or token.text.startswith(" "):
            words.append({"t0": ms(token.start), "t1": ms(token.end), "text": piece})
        else:
            words[-1]["text"] += piece
            words[-1]["t1"] = ms(token.end)
        boundary = False
    return words


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: scrawl-parakeet.py AUDIO.wav", file=sys.stderr)
        return 2
    model_name = MODEL_NAME
    model = from_pretrained(model_name)
    result = model.transcribe(
        sys.argv[1], chunk_duration=CHUNK_SECONDS, overlap_duration=OVERLAP_SECONDS
    )
    segments = [
        {"t0": ms(sentence.start), "t1": ms(sentence.end), "text": sentence.text.strip()}
        for sentence in result.sentences
        if sentence.text.strip()
    ]
    words = []
    for sentence in result.sentences:
        words.extend(words_from_tokens(sentence.tokens))
    json.dump(
        {
            "version": 1,
            "model": model_name.rsplit("/", 1)[-1],
            "text": result.text.strip(),
            "segments": segments,
            "words": words,
            "estimated": False,
        },
        sys.stdout,
    )
    return 0


if __name__ == "__main__":
    sys.exit(main())

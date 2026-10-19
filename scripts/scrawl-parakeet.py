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

The first run resolves the dependencies and downloads the model (about 1.2 GB) from Hugging
Face; later runs are offline. SCRAWL_PARAKEET_MODEL picks another parakeet-mlx model repo.
"""

import json
import os
import sys

from parakeet_mlx import from_pretrained

DEFAULT_MODEL = "mlx-community/parakeet-tdt-0.6b-v3"
# Long recordings are decoded in windows so memory stays flat; the overlap keeps words whole.
CHUNK_SECONDS = 120.0
OVERLAP_SECONDS = 15.0


def ms(seconds: float) -> int:
    return max(0, round(seconds * 1000))


def words_from_tokens(tokens):
    """Sentencepiece tokens carry a leading space on the first piece of a word; punctuation
    pieces attach to the word before them."""
    words = []
    for token in tokens:
        text = token.text
        starts_word = text.startswith(" ") or not words
        piece = text.strip()
        if not piece:
            continue
        if starts_word:
            words.append({"t0": ms(token.start), "t1": ms(token.end), "text": piece})
        else:
            words[-1]["text"] += piece
            words[-1]["t1"] = ms(token.end)
    return words


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: scrawl-parakeet.py AUDIO.wav", file=sys.stderr)
        return 2
    model_name = os.environ.get("SCRAWL_PARAKEET_MODEL", DEFAULT_MODEL)
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

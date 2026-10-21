#!/usr/bin/env -S uv run --quiet --script
# /// script
# requires-python = ">=3.11"
# dependencies = ["mlx-lm>=0.20"]
# ///
"""Scrawl dictation formatter: a small local instruct model through mlx-lm that adds paragraph
breaks and lists to a long dictation, the way Wispr Flow lays one out. Configure it in
~/.config/scrawl/config.json:

    "formatCommand": ["/Users/you/.local/share/uv/tools/mlx-lm/bin/python",
                      "/path/to/scrawl-format.py"]

Needs mlx-lm as a uv tool (brew install uv; uv tool install mlx-lm). Run it once by hand before
the first dictation, as `SCRAWL_PRESTART=1 python scrawl-format.py` and then an empty line: that
downloads the model (about 1.8 GB) from Hugging Face. Once the model is in the hub cache it runs
offline. SCRAWL_FORMAT_MODEL picks another mlx-lm model repo.

Scrawl starts this once a dictation has run 5 seconds, with SCRAWL_PRESTART=1, so the model
loads while the user talks. It then reads one JSON line on stdin, {"text": ..., "pauses": [sentence numbers]},
and prints {"version": 1, "text": ...}. Stdin closing with nothing on it means Scrawl no longer needs
a layout. Scrawl checks that the words came back unchanged and pastes the plain text otherwise.
"""

import glob
import json
import os
import sys

DEFAULT_MODEL = "mlx-community/Qwen2.5-3B-Instruct-4bit"
MODEL_NAME = os.environ.get("SCRAWL_FORMAT_MODEL", DEFAULT_MODEL)

INSTRUCTIONS = """You lay out dictated text. Copy the text exactly, every sentence from the first to
the last, changing only the layout:
- Put a blank line between paragraphs where the topic changes; phrases such as "on a different
  note", "separately" or "also" often start a new one.
- When the speaker lists steps or ordered items, put each on its own line starting "1. ", "2. ",
  and so on; for unordered items use "- ". Keep any words that introduce the list.
- Add only the punctuation that layout needs, such as a colon before a list. The "and" or "or"
  before a list's last item may go.

Never add, remove, reorder or reword a word. Keep words in backticks exactly as they are. Text that
needs no layout comes back unchanged. Return only the text."""

EXAMPLE_IN = ("We need three things for the trip. First book the cabin. Second rent a car. Third pack "
              "the cooler. Also, the dog sitter needs our keys by Friday.")
EXAMPLE_OUT = ("We need three things for the trip:\n\n1. First book the cabin.\n2. Second rent a car.\n"
               "3. Third pack the cooler.\n\nAlso, the dog sitter needs our keys by Friday.")


def model_is_cached(name: str) -> bool:
    """A snapshot with weights in it; a download cut off after config.json must not count, or
    every later run would go offline and fail on the missing weights."""
    hub = os.path.join(os.environ.get("HF_HOME", os.path.expanduser("~/.cache/huggingface")), "hub")
    snapshots = os.path.join(hub, "models--" + name.replace("/", "--"), "snapshots")
    return any(
        glob.glob(os.path.join(os.path.dirname(config), "*.safetensors"))
        for config in glob.glob(os.path.join(snapshots, "*", "config.json"))
    )


# Otherwise the hub client checks every cached file online on each load.
if model_is_cached(MODEL_NAME):
    os.environ.setdefault("HF_HUB_OFFLINE", "1")

from mlx_lm import generate, load  # noqa: E402


def prompt_for(tokenizer, text: str, pauses: list) -> str:
    system = INSTRUCTIONS
    if pauses:
        # In the instructions, not the text, so the model never copies it into its answer.
        numbers = ", ".join(str(n) for n in pauses)
        system += f"\n\nIn the next text, the speaker paused after sentence {numbers}; a pause often starts a new paragraph."
    messages = [
        {"role": "system", "content": system},
        {"role": "user", "content": EXAMPLE_IN},
        {"role": "assistant", "content": EXAMPLE_OUT},
        {"role": "user", "content": text},
    ]
    return tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True)


def main() -> int:
    model, tokenizer = load(MODEL_NAME)
    # One short pass puts the weights on the GPU and compiles kernels while the user talks.
    generate(model, tokenizer, prompt_for(tokenizer, "Hello there.", []), max_tokens=4)
    line = sys.stdin.readline().strip()
    if not line:
        return 0
    request = json.loads(line)
    text = request["text"]
    # Layout adds a few tokens per line; the cap only stops a runaway.
    limit = len(tokenizer.encode(text)) * 2 + 64
    formatted = generate(
        model, tokenizer, prompt_for(tokenizer, text, request.get("pauses", [])), max_tokens=limit
    )
    json.dump({"version": 1, "text": formatted.strip()}, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())

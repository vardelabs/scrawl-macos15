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
loads while the user talks. SCRAWL_FORMAT_EXAMPLES names a JSON file of layouts learned from the
user's edits, {"examples": [{"dictated": ..., "laidOut": ...}]}, oldest first; they join the prompt
as examples, and the prompt up to the dictation is computed while the user talks. It then reads one
JSON line on stdin, {"text": ..., "pauses": [sentence numbers]}, and prints {"version": 1, "text": ...}.
Stdin closing with nothing on it means Scrawl no longer needs a layout. Scrawl checks that the words
came back unchanged and pastes the plain text otherwise.
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

LEARNED_NOTE = """

The examples after the first are this user's own layouts of their dictations. Lay text out the way
they do."""

# Scrawl keeps this many; more would only slow the prompt.
MAX_LEARNED = 6

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

import mlx.core as mx  # noqa: E402
from mlx_lm import generate, load  # noqa: E402
from mlx_lm.models.cache import make_prompt_cache  # noqa: E402


def learned_examples(path: str) -> list:
    """The newest layouts learned from the user's edits; a missing or broken file is none."""
    try:
        with open(path, encoding="utf-8") as file:
            examples = json.load(file).get("examples", [])
    except (OSError, ValueError, AttributeError):
        return []
    if not isinstance(examples, list):
        return []
    pairs = [
        (e["dictated"], e["laidOut"])
        for e in examples
        if isinstance(e, dict) and isinstance(e.get("dictated"), str) and isinstance(e.get("laidOut"), str)
    ]
    return pairs[-MAX_LEARNED:]


def prefix_messages(learned: list) -> list:
    """Everything before the dictation, the same for any text, so it is computed while the user talks."""
    messages = [
        {"role": "system", "content": INSTRUCTIONS + (LEARNED_NOTE if learned else "")},
        {"role": "user", "content": EXAMPLE_IN},
        {"role": "assistant", "content": EXAMPLE_OUT},
    ]
    for dictated, laid_out in learned:
        messages += [{"role": "user", "content": dictated}, {"role": "assistant", "content": laid_out}]
    return messages


def request_messages(text: str, pauses: list) -> list:
    messages = []
    if pauses:
        # A system note, not the text, so the model never copies it into its answer.
        numbers = ", ".join(str(n) for n in pauses)
        note = f"In the next text, the speaker paused after sentence {numbers}; a pause often starts a new paragraph."
        messages.append({"role": "system", "content": note})
    return messages + [{"role": "user", "content": text}]


def prefill(model, tokens: list):
    """A prompt cache holding `tokens`, computed now."""
    cache = make_prompt_cache(model)
    for start in range(0, len(tokens), 512):
        model(mx.array(tokens[start : start + 512])[None], cache=cache)
        mx.eval([c.state for c in cache])
    return cache


def main() -> int:
    model, tokenizer = load(MODEL_NAME)
    prefix = prefix_messages(learned_examples(os.environ.get("SCRAWL_FORMAT_EXAMPLES", "")))
    prefix_tokens = tokenizer.encode(tokenizer.apply_chat_template(prefix, tokenize=False))
    # Computing the prompt before the dictation also puts the weights on the GPU.
    cache = prefill(model, prefix_tokens)
    line = sys.stdin.readline().strip()
    if not line:
        return 0
    request = json.loads(line)
    text = request["text"]
    messages = prefix + request_messages(text, request.get("pauses", []))
    tokens = tokenizer.encode(tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True))
    if tokens[: len(prefix_tokens)] == prefix_tokens:
        tokens = tokens[len(prefix_tokens) :]
    else:
        cache = make_prompt_cache(model)
    # Layout adds a few tokens per line; the cap only stops a runaway.
    limit = len(tokenizer.encode(text)) * 2 + 64
    formatted = generate(model, tokenizer, tokens, max_tokens=limit, prompt_cache=cache)
    json.dump({"version": 1, "text": formatted.strip()}, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())

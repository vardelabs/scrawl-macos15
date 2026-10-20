# Dictation: voice only, pasted where you type

Read this before changing `src/dictation.rs`, `src/dictionary.rs` or the dictation pill.

Dictation is the screenless sibling of a capture: a modifier-only chord records the microphone, the configured transcriber turns it into text, and Scrawl pastes the text into the app in front. It is off by default (`"dictation": true` turns it on, or Settings › General › Dictation).

## The chord (`dictation::Chord`)

The default chord is `⌃⌥` (`dictationShortcut`, also `⌃⌘`, `⌥⌘` or `⌃⇧`). It is a modifier-only chord, so Carbon cannot register it. The UI loop polls `capture::modifier_flags()` (no permission needed) and feeds `Chord::tick`, and the key tap's key-down counter (`SharedCaptureTelemetry::key_downs`, Accessibility) tells it when the modifiers were the start of an ordinary shortcut.

- **Hold to talk.** The chord goes down: the microphone starts at once, so the first word is kept. Held past `TAP_MS`, the pill shows and the command transcriber prestarts. Release: transcribe and paste.
- **Double tap to latch.** Two taps within `DOUBLE_TAP_MS`: hands-free. The pill shows ✕ and ✓. The chord again (pressed and released with no key in between) or ✓ confirms and pastes; Esc or ✕ discards. A latched dictation stops itself after `MAX_LATCHED_MS`.
- **Not dictation.** A key press, or another modifier joining, while the chord is down discards the recording silently: `⌃⌥←` or `⌃⌥⇧S` stays a shortcut. A single short tap is discarded too. Recordings shorter than `MIN_AUDIO_MS` are not transcribed.
- Dictation never starts while a capture is recording, selecting or processing, and the capture shortcut does nothing while dictation runs.

## The pill

Dictation owns its own `NativePill`, placed at the bottom centre of the display under the pointer, clear of the capture capsule at the top. `Pres::Dictating { latched }` lays out the recording dot and clock, with ✕ and ✓ when latched; transcription shows `Pres::Processing`.

## Transcription and paste

`dictation::transcribe` writes the WAV to a temp file and runs `transcribe::transcribe`, the same engines and filler removal as captures (the command transcriber's prestart applies). `dictionary::Dictionary::apply` then fixes words, and `handoff::paste_text` puts the text on the clipboard, posts `⌘V`, and puts the previous clipboard back after `RESTORE_DELAY_MS` unless something else changed it meanwhile (`dictationRestoreClipboard`, default on). Playback ducks while dictating, as during a capture.

## Dictionary (`src/dictionary.rs`)

`~/.config/scrawl/dictionary.json`, read on each dictation:

```json
{
  "words": ["Corvus", "Varde Labs", "Parakeet"],
  "replacements": { "corvis": "Corvus", "var de labs": "Varde Labs" }
}
```

- `replacements` match whole words, ignoring case, longest first.
- `words` fix casing and spacing (`varde labs`, `vardelabs` → `Varde Labs`) and near misses of words of 5 letters or more (one edit, two from 9 letters, same first letter).

Parakeet takes no vocabulary prompt, so the dictionary corrects after transcription.

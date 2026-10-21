# Dictation: voice only, pasted where you type

Read this before changing `src/dictation/` (`chord.rs`, the state machine; `session.rs`, the runtime), `src/dictionary.rs` or the dictation pill.

Dictation is the screenless sibling of a capture: a modifier-only chord records the microphone, the configured transcriber turns it into text, and Scrawl pastes the text into the app in front. It is off by default (`"dictation": true` turns it on, or Settings › Dictation).

## The chord (`dictation::Chord`)

The default chord is `⌃⌥` (`dictationShortcut`, also `⌃⌘`, `⌥⌘` or `⌃⇧`). It is a modifier-only chord, so Carbon cannot register it. The UI loop polls `capture::modifier_flags()` (no permission needed) and feeds `Chord::tick`, and the key tap's key-down counter (`SharedCaptureTelemetry::key_downs`, Accessibility) tells it when the modifiers were the start of an ordinary shortcut. With the key tap off (no Accessibility, or Secure Input on, as in a password field) dictation doesn't start. The clipboard is not restored when it held a password manager's concealed or transient item.

- **Hold to talk.** The chord goes down: the microphone starts at once, so the first word is kept. Held past `TAP_MS`, the pill shows and the command transcriber prestarts. Release: transcribe and paste.
- **Double tap to latch.** Two taps within `DOUBLE_TAP_MS`: hands-free. The pill shows ✕ and ✓. The chord again (pressed and released with no key in between) or ✓ confirms and pastes; Esc or ✕ discards. Any dictation stops itself after `chord::MAX_MS`.
- **Not dictation.** A key press, a mouse button, or another modifier joining, while the chord is down and not latched, discards the recording silently: `⌃⌥←`, `⌃⌥⇧S` or a `⌃⌥`-drag stays what it was. A single short tap is discarded too. Recordings shorter than `MIN_AUDIO_MS` are not transcribed, and a transcript with no letters or digits is not pasted.
- The microphone opens on every press of the chord, so the menu bar's microphone indicator flickers on `⌃⌥` shortcuts; a discarded recorder is dropped off the UI thread.
- Dictation never starts while a capture is recording, selecting or processing. The capture shortcut cancels a held, unlatched chord (a capture shortcut may contain its modifiers) and is ignored while dictation is latched or pasting.

## The pill

Dictation shares Scrawl's one capsule with captures (`ScrawlCoordinator::tick_dictation`). While no capture is on it, dictation gets its hover and clicks (`PillInput`) and decides what it shows (`session::presentation`); a capture takes it over and gives it back when its island is hidden. The capsule sits at the bottom centre of the active screen (`screens::active_point`), a few points above the bottom edge whatever the Dock does, and grows upward from there, as Wispr Flow's pill does:

- `Pres::DictationIdle`: a small see-through bar with a light rim (`FrameState::is_idle`), on while `dictationPill` is (default on), even with dictation off, since it also starts recordings.
- Hovered, `Pres::Hint`: "Dictate ⌃⌥" (when dictation is on), which starts hands-free dictation (`Chord::latch`), and Record, which does what a tap of the record shortcut does.
- `Pres::Dictating { latched }`: the dot and clock, with ✕ and ✓ when latched; `Pres::Processing` while it transcribes.
- A notice, `Pres::Message { notice: true }` with a check, for 2.5 s: "Copied to the clipboard" when nothing was focused, "Didn't catch that" when nothing was heard. Automatic dictionary updates will use the same notice.

Dictation's states sit one window level under the Dock (`window_level`), so a Dock that pops up covers the pill; capture states stay above Scrawl's overlays. The capsule hides while a capture records, selects or processes, and its window is put back on the capture exclusion list every 500 ms (each recording starts a fresh list). Element tiles are packed by width (`render_frame`), so a message wider than a chip isn't cut off. `scrawl lab pill dictation` plays these states.

## Transcription and paste

`session::transcribe_and_paste` writes the WAV to a temp file and runs `transcribe::transcribe`, the same engines and filler removal as captures (the command transcriber's prestart applies). `dictionary::Dictionary::apply` then fixes words, and `session::deliver` asks Accessibility whether the focused element takes text (`ax::focused_takes_text`: a text role, or a selection on an element whose `AXValue` can be set; read-only selectable text doesn't count). If it does, `handoff::paste_text` puts the text on the clipboard, posts `⌘V`, and puts the previous clipboard back after `RESTORE_DELAY_MS` (an empty clipboard stays empty) unless something else changed it meanwhile (`dictationRestoreClipboard`, default on). Every route pastes, since an app with its Accessibility tree off can report nothing focused over a real field. When the system says nothing is focused (`kAXErrorNoValue`), the text also stays on the clipboard and the pill says "Copied to the clipboard". An app that doesn't answer in time, or a focused element whose role doesn't say, gets the paste and keeps the text on the clipboard, so nothing is lost. A click on the hint just after a notice goes (`CLICK_GRACE`) is taken as meant for the notice. Playback ducks while dictating, as during a capture.

## Dictionary (`src/dictionary.rs`)

`~/.config/scrawl/dictionary.json`, read on each dictation:

```json
{
  "words": ["Corvus", "Varde Labs", "Parakeet"],
  "replacements": { "corvis": "Corvus", "var de labs": "Varde Labs" }
}
```

- `replacements` match whole words, ignoring case, longest first.
- `words` fix casing and spacing (`varde labs`, `vardelabs` → `Varde Labs`) and near misses of words of 7 letters or more (one edit, two from 10 letters, same first letter). Shorter words have real neighbours (`corpus` and `Corvus`), so they need a replacement.

Parakeet takes no vocabulary prompt, so the dictionary corrects after transcription.

## Settings › Dictation

The pane (`settings::DictationView`, `dictation` in `helper/Settings.swift`) holds the switch, the chord, **Always show the pill** (`dictationPill`), **Restore the clipboard after a paste**, **Remove filler words** (`removeFillers`, shared with recordings) and the dictionary editor. `add-word`, `remove-word`, `add-replacement` and `remove-replacement` load, change and save `dictionary.json` (`Dictionary::save_to` renames a temp file into place and keeps keys it doesn't know). A file that doesn't parse is shown as an error and never overwritten; **Edit as Text…** (`edit-dictionary`) opens it in the default text editor, made empty first if there is none. The pane is macOS only: the Windows settings window doesn't list it. Render it with `scrawl-helper settings --render DIR` (`dictation`, `dictation-empty`, `dictation-replacements`, `dictation-broken`).

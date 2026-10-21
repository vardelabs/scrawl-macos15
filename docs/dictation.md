# Dictation: voice only, pasted where you type

Read this before changing `src/dictation/` (`chord.rs`, the state machine; `session.rs`, the runtime; `context.rs`, filenames and code; `format.rs` and `formatter.rs`, paragraphs and lists), `src/dictionary.rs` or the dictation pill.

Dictation is the screenless sibling of a capture: a modifier-only chord records the microphone, the configured transcriber turns it into text, and Scrawl pastes the text into the app in front. It is off by default (`"dictation": true` turns it on, or Settings › Dictation).

## The chord (`dictation::Chord`)

The default chord is `⌃⌥` (`dictationShortcut`, also `⌃⌘`, `⌥⌘` or `⌃⇧`). It is a modifier-only chord, so Carbon cannot register it. The UI loop polls `capture::modifier_flags()` (no permission needed) and feeds `Chord::tick`, and the key tap's key-down counter (`SharedCaptureTelemetry::key_downs`, Accessibility) tells it when the modifiers were the start of an ordinary shortcut. With the key tap off (no Accessibility, or Secure Input on, as in a password field) dictation doesn't start. The clipboard is not restored when it held a password manager's concealed or transient item.

- **Hold to talk.** The chord goes down: the microphone starts at once, so the first word is kept. Held past `TAP_MS`, the pill shows and the command transcriber prestarts. Release: transcribe and paste.
- **Double tap to latch.** Two taps within `DOUBLE_TAP_MS`: hands-free. The pill shows ✕ and ✓. The chord again (pressed and released with no key in between) or ✓ confirms and pastes; Esc or ✕ discards. Any dictation stops itself after `chord::MAX_MS`.
- **Not dictation.** A key press, a mouse button, or another modifier joining, while the chord is down and not latched, discards the recording silently: `⌃⌥←`, `⌃⌥⇧S` or a `⌃⌥`-drag stays what it was. A single short tap is discarded too. Recordings shorter than `MIN_AUDIO_MS` are not transcribed, and a transcript with no letters or digits is not pasted.
- The microphone opens on every press of the chord, so the menu bar's microphone indicator flickers on `⌃⌥` shortcuts; a discarded recorder is dropped off the UI thread.
- Dictation never starts while a capture is recording, selecting or processing. The capture shortcut cancels a held, unlatched chord (a capture shortcut may contain its modifiers) and is ignored while dictation is latched or pasting.

## The pill

Dictation shares Scrawl's one capsule with captures (`ScrawlCoordinator::tick_dictation`). While no capture is on it, dictation gets its hover and clicks (`PillInput`) and decides what it shows (`session::presentation`); a capture takes it over and gives it back when its island is hidden. The capsule sits at the bottom centre of the active screen (`screens::active_point`) and grows upward from there, as Wispr Flow's pill does. With a Dock that stays shown at the bottom it sits just above it; with a hidden Dock (or one at the side) it sits a few points above the screen's own bottom edge, where the Dock slides over it (`bottom_edge`). It needs the native capsule (`nativePill`, on by default on macOS):

- `Pres::DictationIdle`: a small see-through bar with a light rim (`FrameState::is_idle`), on while dictation and `dictationPill` (default on) both are. Hovered, it offers Dictate and Record.
- Hovered, `Pres::Hint`: "Dictate ⌃⌥", which starts hands-free dictation (`Chord::latch`), and Record, which does what a tap of the record shortcut does.
- `Pres::Dictating { latched }`: the dot and clock, with ✕ and ✓ when latched; `Pres::Processing` while it transcribes.
- A notice, `Pres::Message` with a check, for 2.5 s: "Copied to the clipboard" when nothing was focused, "Didn't catch that" when nothing was heard. Automatic dictionary updates will use the same notice.

Dictation's states, its Processing included, sit one window level under the Dock (`PillSnapshot::under_dock`), so a hidden Dock that pops up covers the pill; capture states stay above Scrawl's overlays. While a capture records, selects or processes, the capsule shows the capture and dictation's chord is ignored; recordings leave the capsule out because SCK excludes Scrawl's windows above layer 0, and both of the capsule's levels are. Element tiles are packed by width (`render_frame`), so a message wider than a chip isn't cut off. `scrawl lab pill dictation` plays these states.

## Transcription and paste

`session::transcribe_and_paste` writes the WAV to a temp file and runs `transcribe::transcribe`, the same engines and filler removal as captures (the command transcriber's prestart applies). `dictionary::Dictionary::apply` then fixes words, and `session::deliver` asks Accessibility whether the focused element takes text (`ax::focused_takes_text`: a text role, or a selection on an element whose `AXValue` can be set; read-only selectable text doesn't count). If it does, `handoff::paste_text` puts the text on the clipboard, posts `⌘V`, and puts the previous clipboard back after `RESTORE_DELAY_MS` (an empty clipboard stays empty) unless something else changed it meanwhile (`dictationRestoreClipboard`, default on). Every route pastes, since an app with its Accessibility tree off can report nothing focused over a real field. When the system says nothing is focused (`kAXErrorNoValue`), the text also stays on the clipboard and the pill says "Copied to the clipboard". An app that doesn't answer in time, or a focused element whose role doesn't say, gets the paste and keeps the text on the clipboard, so nothing is lost. A click on the hint just after a notice goes (`CLICK_GRACE`) is taken as meant for the notice. Playback ducks while dictating, as during a capture, but over 0.12 s (`playback::Guard::start_quick`) instead of a capture's one-second fade, so it is quiet before the first word.

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

## Filenames and code (`src/dictation/context.rs`)

With **Format code and filenames** (`dictationCodeFormatting`, default on), the text goes through three pure steps after the dictionary, so a rule for "design dot md" still sees what was said. Prose is the common case, so every rule needs a spoken "dot" or a written `.` or `_`. Names (deSilva), brand names (iPhone, Node), camelCase, and phrasal verbs that match an identifier on screen ("sign in" beside `signIn`) all stay as spoken.

1. `spoken_extensions`: a lower-case spoken "dot" before a known extension joins the word before it, so "design dot MD" becomes "design.md". The name "Dot", "the", "a" or "an" before the "dot", and extensions that are everyday words or letters (`AMBIGUOUS`: go, c, h, sh, env, lock) are left as spoken.
2. `apply` with screen filenames: when the chord is released, `ax::window_text` walks the focused window's Accessibility tree breadth first, on a thread of its own alongside transcription. It stops at `SCREEN_ELEMENTS` elements or `SCREEN_BUDGET`, and never reads a secure text field. The paste waits for it at most `SCREEN_WAIT` after the chord; an app slow to answer Accessibility gets its text without filenames. `terms` keeps the filenames it finds. A spoken run of up to `MAX_SPAN_WORDS` words that ends in a spoken "dot" and an extension, and equals a screen filename on letters and digits (a possessive "'s" or "s'" dropped from the words before the extension), is written as that filename. So "the product's design dot MD" with `PRODUCT-DESIGN.md` on screen becomes `PRODUCT-DESIGN.md`, while "note text" beside `note.txt` stays.
3. Backticks go around filenames (not a product name such as `Node.js`) and `snake_case` (outside existing backticks, with punctuation and quotes outside), except in prose apps (`PROSE_APPS`: Mail, Messages, Notes, Pages, Word, Outlook).

With the setting off, none of this runs and nothing is read from the screen. Screen text is kept for one dictation only, and never stored or sent.

## Paragraphs and lists (`src/dictation/format.rs`, `formatter.rs`)

A long dictation is laid out as Wispr Flow lays one out: paragraphs where the topic changes, and numbered or bulleted lists when the speaker lists things. A local model does the layout (`scripts/scrawl-format.py`, a 4-bit Qwen2.5 3B through mlx-lm, set up like Parakeet). Nothing leaves the Mac. It needs `"formatCommand": ["/Users/you/.local/share/uv/tools/mlx-lm/bin/python", "/path/to/scrawl-format.py"]`, and **Format paragraphs and lists** (`dictationFormatting`, default on).

1. When the dictation shows, `formatter::Formatter::prestart` starts the formatter with `SCRAWL_PRESTART=1`, so its model loads while the user talks. A cancelled dictation drops it, which kills its process group.
2. After filler removal, the dictionary and the filename and code pass, `format::worth_formatting` sends only dictations of `MIN_WORDS` words and `MIN_SENTENCES` sentences or more. Shorter ones drop the formatter unused.
3. The request carries the text and `format::pause_hints`: the sentences after which Parakeet's word timings show a silence of `PARAGRAPH_PAUSE_MS` or more. The paste waits for the reply for `FORMAT_WAIT_BASE` plus `FORMAT_WAIT_PER_WORD` per word, at most `FORMAT_WAIT_MAX`. When the model laid out no paragraphs, `break_at_pauses` breaks at those pauses.
4. `format::same_words` checks that the reply has the dictated words in order, ignoring case, punctuation, Markdown wrapping and line-leading list markers. A list may drop the "and" or "or" before its last item. Any other change, a timeout or a bad reply pastes the text as dictated. The log says which happened, never the text.

## Settings › Dictation

The pane (`settings::DictationView`, `dictation` in `helper/Settings.swift`) holds the switch, the chord, **Always show the pill** (`dictationPill`), **Restore the clipboard after a paste**, **Format code and filenames** (`dictationCodeFormatting`, default on), **Format paragraphs and lists** (`dictationFormatting`, default on; dimmed until `formatCommand` is set up), **Remove filler words** (`removeFillers`, shared with recordings) and the dictionary editor. `add-word`, `remove-word`, `add-replacement` and `remove-replacement` load, change and save `dictionary.json` (`Dictionary::save_to` renames a temp file into place and keeps keys it doesn't know). A file that doesn't parse is shown as an error and never overwritten; **Edit as Text…** (`edit-dictionary`) opens it in the default text editor, made empty first if there is none. The pane is macOS only: the Windows settings window doesn't list it. Render it with `scrawl-helper settings --render DIR` (`dictation`, `dictation-empty`, `dictation-replacements`, `dictation-broken`).

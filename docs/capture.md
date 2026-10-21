# Capture: shortcuts, Mac recording, displays

Read this before changing `src/hold.rs`, `src/capture.rs`, `src/screens.rs` or the picker and capsule placement in `src/ui.rs`.

## Shortcuts (`src/hold.rs`, `is_capture_shortcut` in `src/capture.rs`)

`⌥⇧S` (or the shortcut set in Settings) works like `⌘Tab`. A tap (modifiers up within 250 ms) records the display under the pointer at once, auto-cropped, with a brief red edge glow from the overlay as the start cue, and the capsule springs open onto switch chips (Area · Window · Screen · iPhone if cabled; `hold::Reveal`, `REVEAL_*` constants at the top of `src/ui.rs`) until 3.5 s pass or the user clicks, scrolls or draws (never while hovered). A chip (`switch_recording`) restarts rather than retargets: Window/Area discard the few seconds and morph the capsule into the non-held picker, iPhone discards and starts the phone.

Held, the switcher opens on Screen; each further press of the key (S) steps Screen → Window → iPhone, a mouse drag selects an area, Esc cancels, and releasing the modifiers records the selection. While recording, a tap finishes, ignoring one within 400 ms of the start. The menu's **Choose What to Record…** opens the full picker.

Carbon hotkeys and the key event tap report presses through `emit_capture_shortcut` (deduplicated within 90 ms); the UI loop polls the modifiers and takes Esc from the key tap (`tick_switcher`, `take_escape`), so the release is seen whichever window has focus.

## Mac captures (`src/capture.rs`)

The sampler keeps only small candidates (480 px long edge, in a `CandidateBuffer` capped at 384 MB that thins evenly and halves its sampling rate when full, never dropping the start or click/stroke frames). At finish the chosen keyframes are decoded from `recording.mp4` at native size (2560 px Screen, 1280 px Window/Region, never above the display's own scale, so a 1x 1920 pt display records 1920 px), shifted by `videoOffsetMs` (when the movie started on the capture clock); if decoding fails the candidates are used. macOS prompts over a Screen or Area capture (`windows::system_prompts_over`: UserNotificationCenter, SecurityAgent; ScreenCaptureKit's "bypass the system private window picker" alert comes back every 30 days) are left out of the recording through the content filter when ScreenCaptureKit lists them, frames that may show one are dropped from the candidates (`CandidateBuffer::drop_since`), and clicks on them aren't recorded. `scrawl capture-sim` drives this headless for memory and timing checks.

## Pausing (`src/pause.rs`)

`⌥⇧P` (a Carbon hotkey registered only while a capture runs, `set_pause_hotkey_enabled`) or the capsule's **Pause** chip pauses a Screen, Window or Area capture; the capsule shows **Paused** with the clock stopped and **Resume** in Pause's place. iPhone captures don't pause (the phone records on its own clock), nor do captures on Windows (Media Foundation writes one movie straight through, `PAUSE_SUPPORTED`).

Everything in the bundle runs on the capture's *active* time (`PauseClock`), which stands still while paused: event and stroke times (`SharedCaptureTelemetry::elapsed_ms`), candidate frames, the window log, the microphone (the callback drops what it hears while paused, so `audio.wav` and the live transcription hold the active audio back to back and word times need no shifting) and the movie. While paused the input taps record nothing (`SharedCaptureTelemetry::capturing`), and a press, drag or ⇧-mark in progress is dropped.

On pause the sampler stops the ScreenCaptureKit stream, which finishes that run's movie; on resume it starts a new stream writing `recording-2.mp4`, `recording-3.mp4`, …, and only once it's running does the clock resume (`PauseClock::resume_at`), so the next segment starts exactly where the last stopped on the active timeline and the few hundred ms the stream takes to start are part of the pause. At finish `scrawl-helper join` puts the segments back into `recording.mp4` without re-encoding, each cut to where its pause began (`join_plan`); a capture never paused skips this. Resume keeps the original target: a display or region is the same rect on the same display, and a window is followed wherever it moved or whichever display it's on, as ScreenCaptureKit's window filter does; a window closed while paused records nothing more, as one closed while recording does. If the join fails the candidates stand in for the movie. A capture that crashes after a pause recovers only up to its first pause (`recover` reads `recording.mp4`).

## More than one display (`src/screens.rs`)

Every rect and point is global top-left points (`CGEvent`, `CGDisplayBounds`, `SCWindow` frames, `TargetFrame`); displays left of or above the primary one are negative. `screens::flip` converts AppKit frames, `screens::current()` lists displays with menu bar, notch and `backingScaleFactor` (main thread), `screens::displays()` the CoreGraphics view (any thread).

A capture belongs to one display: a tap takes the one under the pointer, Window the one holding the window's centre, Area the one holding its centre (clamped to it; `sourceRect` is display-local via `screens::source_rect`); `TargetFrame.scale` is that display's.

The picker opens on the pointer's display and follows the pointer between displays (window, Metal overlay and capsule: `follow_pointer_display`) except while an area is dragged or iPhone is shown; the overlay `move_to`s the target's display when recording starts and follows the iPhone mirror if it's dragged. The capsule (`NativePill::place_at_bottom`) sits at the bottom centre of the display in use (above a Dock that stays shown, else just above the screen's bottom edge: `bottom_edge`) and grows upward from there (`PillModel::anchor_bottom`); with nothing being recorded it follows the active screen (`screens::active_point`: the frontmost app's front window, else the pointer). Dictation shares it (`docs/dictation.md`). Test with a `CGVirtualDisplay` (see `scripts/virtual-display.swift`), never on Kit's desk Mac.

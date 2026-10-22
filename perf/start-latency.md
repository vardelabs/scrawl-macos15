# Start latency

## Goal and method

Primary: `recording_start_ms`, synchronous main-thread wall time in the real
`start_recording_with_target` path. Secondary: `picker_prep_ms`, `picker_open_ms`,
`target_discovery_ms`, `metal_open_ms`, `audio_start_ms`, `sck_start_ms`.
Picker-open ends after window creation, not first presented pixels or hotkey delivery.

```
taskpolicy -c utility cargo build -j 3 --offline
bun scripts/bench-start.ts
taskpolicy -c utility cargo test -j 3 --offline -- --test-threads=2
bun scripts/bench-start.ts --quick-stop
```

The benchmark runs one warmup and seven measured captures in one GPUI process.
It uses the real picker/coordinator, Metal overlay, microphone and ScreenCaptureKit.
Each captures for 1.5 seconds after synchronous start returns, then stops and joins.
The `--quick-stop` smoke mode stops both workers immediately, including during
initialization. It checks clean joins and valid WAV structure; zero samples/frames
and absent MP4 are allowed for these intentionally zero-length sessions.
Temporary recordings are checked for frames, nonempty WAV and decodable MP4, then
deleted. No transcription, clipboard, hotkey registration, or persistent bundles.
Requires granted screen/microphone permissions and `ffprobe`. The child runs under
`caffeinate -i` (never `-d`/`-u`: those wake the display). It needs an awake screen someone is using: `perf/screen-guard.sh` refuses on the Studio, so run it on the MacBook Pro under `/tmp/pro-gui.lock`.
Median, MAD, min/max and raw samples are recorded; startup acceptance is distinct
from hardware readiness, so worker initialization and first-frame timing must also
be watched when moving initialization off-thread.

## Hypotheses

1. Move SCK initialization to the frame sampler; keep stream ownership through stop.
2. Move microphone initialization to a worker; join and propagate errors on stop.
3. Populate picker windows asynchronously; resolve regions from that snapshot.
4. Cache Metal device/queue/pipeline between captures.

## Results

All timings below are milliseconds, median ± MAD (one warmup, seven measured).

| Experiment | Recording start | Picker prep | Picker open | Metal | Audio | SCK | Decision |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| Baseline | 222.375 ± 3.260 | 60.224 ± 0.919 | 82.702 ± 13.669 | 2.623 ± 0.188 | 57.320 ± 0.791 | 161.622 ± 1.385 | baseline |
| H1: SCK worker | 59.177 ± 0.658 | 62.712 ± 3.082 | 76.031 ± 6.966 | 2.638 ± 0.129 | 56.194 ± 0.745 | 0.018 ± 0.001 | keep |
| H2: audio worker | 3.205 ± 0.094 | 63.436 ± 3.795 | 96.891 ± 2.115 | 2.878 ± 0.066 | 0.016 ± 0.002 | 0.015 ± 0.002 | keep |
| H3: async picker/snapshot | 3.025 ± 0.116 | 0.018 ± 0.001 | 14.313 ± 1.853 | 2.765 ± 0.116 | 0.014 ± 0.001 | 0.011 ± 0.001 | keep |
| H4: Metal cache | 3.026 ± 0.117 | 0.018 ± 0.001 | 18.591 ± 4.664 | 2.677 ± 0.123 | 0.016 ± 0.001 | 0.014 ± 0.001 | discard |
| Final H1–H3 | 4.224 ± 0.617 | 0.019 ± 0.003 | 18.851 ± 6.726 | 3.490 ± 0.798 | 0.016 ± 0.000 | 0.014 ± 0.003 | keep |

Baseline recording range: 218.804–246.828; region discovery 67.428 ± 1.831.
All eight captures returned real 960×640 frames, nonempty WAV and decodable MP4.
Before this baseline, two runs failed on inactive-display `CVDisplayLink -6661`;
a third lost its display mid-run. These were rejected, not included as fast starts.
`caffeinate -du` fixed that benchmark precondition.

H1: `scrawl-frame-sampler` owns setup, stream, recording output and teardown.
`stop()` signals and joins the worker before collecting frames/video; dropping a
session also signals and joins. Recording-start range 58.083–64.023. SCK hardware
readiness remains 168.389 ± 0.873, rather than being misreported as 0.018 ms.
All eight media checks and all 36 tests pass.

H2: `scrawl-audio` creates/plays/holds/drops the CPAL stream. Channel disconnect
signals stop; joining precedes buffer extraction and propagates startup failures.
Drop also stops/joins. Recording-start range 2.981–8.651. Audio hardware readiness
is 63.433 ± 2.268; SCK readiness 176.457 ± 2.797. UI acceptance is fast, but initial
microphone/screen acquisition still takes time. All eight media checks and 36 tests pass.

H3: picker opens before SCK window enumeration; generation checks reject stale
results after dismiss/reopen. Window selection waits for discovery; hover/click
hit-testing uses the current pointer and cached front-to-back windows. Region
metadata uses that same snapshot (bundle ID now comes from the matched window).
Picker-open range 11.004–18.593; region discovery falls from 63.168 to
0.402 ± 0.032. Window-list readiness is separate: 169.146 ± 20.910.
SCK readiness 196.772 ± 12.332 with one 871.743 outlier. All eight media checks
and 37 tests pass, including overlap/negative-coordinate/empty-snapshot coverage.

H4: tried a `OnceLock` for device, command queue and render pipeline, with failures
left retryable. Recording-start range 2.803–3.503. Metal improved only 0.088 ms,
less than either run's MAD; total start was unchanged. All media checks and 37
tests passed, but the added cache is discarded as noise. Kept code is H1–H3.

## Final verification

- Recording main-thread startup: 222.375 → 4.224 ms (98.1% reduction).
- Picker opening: 82.702 → 18.851 ms (77.2% reduction); preparation 60.224 → 0.019 ms.
- Region discovery: 67.428 → 0.392 ms.
- Hardware readiness still takes time: audio 57.569 ms; SCK 179.785 ms.
  These changes remove main-thread stalls, not device initialization latency.
- Eight full captures passed real-frame, WAV sample-count, and decoded-MP4 checks.
  Final measured runs contained 6–7 real frames and 23,219–24,148 audio samples each.
- Eight immediate-stop sessions cleanly joined both initializing workers and
  returned structurally valid empty WAVs; no hanging or leftover worker.
- All 40 tests pass, including final frame/video collection, audio callback flush,
  background initialization error propagation, and cached window hit-testing.
- Cargo manifest/lock unchanged. Metal caching intentionally not retained.

Final recording samples (excluding warmup):
`[4.545833, 4.841750, 4.224375, 2.907458, 4.672333, 2.912250, 2.812584]`.
Final recording range 2.813–4.842; picker-open range 11.614–35.458.

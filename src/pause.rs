//! Pausing a screen capture. While paused nothing is captured, and every timeline in the bundle
//! (video, audio, clicks, marks, keys, the window log) runs on the capture's *active* time, which
//! stands still while paused, so the bundle reads as if the pause never happened.
//!
//! The screen stream is torn down on pause and started again on resume, each run writing its own
//! movie segment; `join_plan` says how the segments are put back together at the end. Resuming
//! takes effect when the new stream is running (`PauseClock::finish_resume`), so the next segment
//! starts where the last one stopped on the active timeline.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// The paused spans of a capture, in wall-clock ms since it started.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Timeline {
    closed: Vec<(u64, u64)>,
    open: Option<u64>,
}

impl Timeline {
    /// Starts a pause at `wall_ms`. Returns false when already paused.
    pub fn pause(&mut self, wall_ms: u64) -> bool {
        if self.open.is_some() {
            return false;
        }
        self.open = Some(wall_ms);
        true
    }

    /// Ends the pause at `wall_ms`. Returns false when not paused.
    pub fn resume(&mut self, wall_ms: u64) -> bool {
        let Some(start) = self.open.take() else {
            return false;
        };
        self.closed.push((start, wall_ms.max(start)));
        true
    }

    /// The active time at `wall_ms`: the wall time less whatever of it was paused.
    pub fn active_ms(&self, wall_ms: u64) -> u64 {
        let overlap = |start: u64, end: u64| end.min(wall_ms).saturating_sub(start);
        let paused: u64 = self
            .closed
            .iter()
            .map(|&(start, end)| overlap(start, end))
            .sum::<u64>()
            + self.open.map_or(0, |start| overlap(start, u64::MAX));
        wall_ms.saturating_sub(paused)
    }

    /// The active time at `wall_ms`, or `None` when that moment was paused.
    pub fn active_at(&self, wall_ms: u64) -> Option<u64> {
        let paused = self
            .closed
            .iter()
            .any(|&(start, end)| (start..end).contains(&wall_ms))
            || self.open.is_some_and(|start| wall_ms >= start);
        (!paused).then(|| self.active_ms(wall_ms))
    }

    pub fn paused_spans(&self) -> &[(u64, u64)] {
        &self.closed
    }
}

/// One capture's clock, shared by everything that stamps times into its bundle.
pub struct PauseClock {
    started_at: Instant,
    /// Checked lock-free by the input taps and the microphone callback.
    paused: AtomicBool,
    /// Resume was asked for; the sampler takes it once the screen stream is back.
    resume_wanted: AtomicBool,
    timeline: Mutex<Timeline>,
}

impl PauseClock {
    pub fn new() -> Self {
        Self::since(Instant::now())
    }

    pub fn since(started_at: Instant) -> Self {
        Self {
            started_at,
            paused: AtomicBool::new(false),
            resume_wanted: AtomicBool::new(false),
            timeline: Mutex::new(Timeline::default()),
        }
    }

    pub fn started_at(&self) -> Instant {
        self.started_at
    }

    fn wall_ms(&self, at: Instant) -> u64 {
        at.saturating_duration_since(self.started_at).as_millis() as u64
    }

    fn timeline(&self) -> std::sync::MutexGuard<'_, Timeline> {
        self.timeline
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    pub fn active_ms(&self) -> u64 {
        self.active_ms_at(Instant::now())
    }

    pub fn active_ms_at(&self, at: Instant) -> u64 {
        self.timeline().active_ms(self.wall_ms(at))
    }

    /// Nothing may be captured: paused, or resumed but the screen stream isn't back yet.
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Acquire)
    }

    /// What the user last asked for, which the pill shows.
    pub fn wants_paused(&self) -> bool {
        self.is_paused() && !self.resume_wanted.load(Ordering::Acquire)
    }

    /// Returns whether this changed anything. Pausing again before a resume took effect cancels it.
    pub fn pause(&self) -> bool {
        if self.resume_wanted.swap(false, Ordering::AcqRel) {
            return true;
        }
        let paused = self.timeline().pause(self.wall_ms(Instant::now()));
        if paused {
            self.paused.store(true, Ordering::Release);
        }
        paused
    }

    pub fn request_resume(&self) -> bool {
        self.is_paused() && !self.resume_wanted.swap(true, Ordering::AcqRel)
    }

    /// A resume is asked for and not yet cancelled; the sampler starts the stream on it.
    pub fn resume_requested(&self) -> bool {
        self.is_paused() && self.resume_wanted.load(Ordering::Acquire)
    }

    /// Capture goes on from `at`, when the screen stream started again. Returns false when a
    /// pause cancelled the resume while the stream was starting: capture stays paused.
    pub fn finish_resume(&self, at: Instant) -> bool {
        let mut timeline = self.timeline();
        if !self.resume_wanted.swap(false, Ordering::AcqRel) {
            return false;
        }
        timeline.resume(self.wall_ms(at));
        self.paused.store(false, Ordering::Release);
        true
    }

    /// The active time at `at`, or `None` when that moment was paused.
    pub fn active_at(&self, at: Instant) -> Option<u64> {
        self.timeline().active_at(self.wall_ms(at))
    }

    pub fn paused_spans(&self) -> Vec<(u64, u64)> {
        self.timeline().paused_spans().to_vec()
    }
}

impl Default for PauseClock {
    fn default() -> Self {
        Self::new()
    }
}

/// The clock as one part of the capture sees it: active ms since that part started.
#[derive(Clone)]
pub struct SessionClock {
    pub clock: Arc<PauseClock>,
    base_ms: u64,
}

impl SessionClock {
    pub fn starting_now(clock: Arc<PauseClock>) -> Self {
        let base_ms = clock.active_ms();
        Self { clock, base_ms }
    }

    /// Times from when the clock itself started.
    pub fn whole(clock: Arc<PauseClock>) -> Self {
        Self { clock, base_ms: 0 }
    }

    pub fn ms(&self) -> u64 {
        self.clock.active_ms().saturating_sub(self.base_ms)
    }

    pub fn ms_at(&self, at: Instant) -> u64 {
        self.clock.active_ms_at(at).saturating_sub(self.base_ms)
    }
}

/// One run of the screen stream between pauses: its movie, and the active ms (on the session's
/// clock) where it starts and, if it was paused, ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Segment {
    pub path: PathBuf,
    pub start_ms: u64,
    pub end_ms: Option<u64>,
}

/// How to join segments into one movie.
#[derive(Debug, PartialEq, Eq)]
pub struct JoinPlan<'a> {
    /// The joined movie's `videoOffsetMs`: where its first frame sits on the active timeline.
    pub video_offset_ms: u64,
    /// Each segment's movie, where it goes in the joined movie, and how much of it to keep
    /// (`None`: all of it).
    pub parts: Vec<(&'a Path, u64, Option<u64>)>,
}

/// Drops segments with nothing on the active timeline (a pause before the stream was up) and
/// trims each paused one to where the pause began, so the stream's few ms of stopping latency
/// never show paused screen.
pub fn join_plan(segments: &[Segment]) -> Option<JoinPlan<'_>> {
    let kept: Vec<&Segment> = segments
        .iter()
        .filter(|s| s.end_ms.is_none_or(|end| end > s.start_ms))
        .collect();
    let video_offset_ms = kept.first()?.start_ms;
    let parts = kept
        .into_iter()
        .map(|s| {
            (
                s.path.as_path(),
                s.start_ms - video_offset_ms,
                s.end_ms.map(|end| end - s.start_ms),
            )
        })
        .collect();
    Some(JoinPlan {
        video_offset_ms,
        parts,
    })
}

/// The movie for segment `index` (0-based) next to the first one: `recording.mp4`,
/// `recording-2.mp4`, …
pub fn segment_path(first: &Path, index: usize) -> PathBuf {
    if index == 0 {
        return first.to_path_buf();
    }
    let stem = first
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("recording");
    let ext = first.extension().and_then(|e| e.to_str()).unwrap_or("mp4");
    first.with_file_name(format!("{stem}-{}.{ext}", index + 1))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn times_after_a_pause_continue_from_where_it_began() {
        let mut t = Timeline::default();
        assert_eq!(t.active_ms(4_000), 4_000);
        t.pause(5_000);
        assert_eq!(t.active_ms(5_000), 5_000);
        assert_eq!(
            t.active_ms(65_000),
            5_000,
            "the clock stands still while paused"
        );
        t.resume(65_000);
        assert_eq!(t.active_ms(65_000), 5_000);
        assert_eq!(t.active_ms(67_500), 7_500);
        assert_eq!(
            t.active_ms(3_000),
            3_000,
            "times before the pause are untouched"
        );
    }

    #[test]
    fn a_pause_at_the_very_start_loses_no_active_time() {
        let mut t = Timeline::default();
        t.pause(0);
        assert_eq!(t.active_ms(9_000), 0);
        t.resume(9_000);
        assert_eq!(t.active_ms(10_000), 1_000);
    }

    #[test]
    fn a_capture_stopped_while_paused_ends_where_the_pause_began() {
        let mut t = Timeline::default();
        t.pause(12_000);
        assert_eq!(t.active_ms(40_000), 12_000);
        assert!(
            t.paused_spans().is_empty(),
            "an open pause isn't a closed span yet"
        );
    }

    #[test]
    fn several_pauses_each_cut_their_own_span() {
        let mut t = Timeline::default();
        t.pause(1_000);
        t.resume(3_000);
        t.pause(5_000);
        t.resume(10_000);
        t.pause(12_000);
        t.resume(12_000);
        assert_eq!(t.active_ms(2_000), 1_000);
        assert_eq!(t.active_ms(4_000), 2_000);
        assert_eq!(t.active_ms(7_000), 3_000);
        assert_eq!(t.active_ms(11_000), 4_000);
        assert_eq!(t.active_ms(20_000), 13_000);
        assert_eq!(
            t.paused_spans(),
            &[(1_000, 3_000), (5_000, 10_000), (12_000, 12_000)]
        );
    }

    #[test]
    fn pausing_twice_or_resuming_unpaused_changes_nothing() {
        let mut t = Timeline::default();
        assert!(!t.resume(1_000));
        assert!(t.pause(2_000));
        assert!(!t.pause(3_000));
        assert!(t.resume(4_000));
        assert_eq!(t.active_ms(5_000), 3_000);
    }

    #[test]
    fn the_clock_holds_capture_until_the_stream_is_back() {
        let clock = PauseClock::new();
        assert!(clock.pause());
        assert!(clock.is_paused() && clock.wants_paused());
        assert!(clock.request_resume());
        assert!(clock.is_paused(), "still held until the sampler resumes it");
        assert!(
            !clock.wants_paused(),
            "but the pill already shows recording"
        );
        assert!(clock.pause(), "pausing again cancels the pending resume");
        assert!(!clock.resume_requested());
        assert!(clock.request_resume());
        assert!(clock.resume_requested());
        assert!(clock.finish_resume(Instant::now()));
        assert!(!clock.is_paused());
        assert_eq!(clock.paused_spans().len(), 1);
    }

    #[test]
    fn a_pause_while_the_stream_restarts_cancels_the_resume_and_the_next_pause_still_pauses() {
        let clock = PauseClock::new();
        clock.pause();
        clock.request_resume();
        // The sampler has seen the request and is starting the stream; the pill shows recording.
        assert!(clock.resume_requested() && !clock.wants_paused());
        assert!(clock.pause(), "the press cancels the pending resume");
        assert!(clock.wants_paused());
        assert!(
            !clock.finish_resume(Instant::now()),
            "the stream came up too late"
        );
        assert!(clock.is_paused());
        assert!(clock.request_resume());
        assert!(clock.finish_resume(Instant::now()));
        assert!(clock.pause() && clock.is_paused(), "the next pause pauses");
    }

    #[test]
    fn moments_inside_a_pause_have_no_active_time() {
        let mut t = Timeline::default();
        t.pause(1_000);
        t.resume(3_000);
        t.pause(5_000);
        assert_eq!(t.active_at(500), Some(500));
        assert_eq!(t.active_at(2_000), None);
        assert_eq!(t.active_at(3_000), Some(1_000));
        assert_eq!(t.active_at(4_000), Some(2_000));
        assert_eq!(t.active_at(6_000), None, "an open pause runs to now");
    }

    fn seg(name: &str, start_ms: u64, end_ms: Option<u64>) -> Segment {
        Segment {
            path: PathBuf::from(name),
            start_ms,
            end_ms,
        }
    }

    #[test]
    fn joined_segments_sit_where_they_were_on_the_active_timeline() {
        // The movie started 120 ms in; pauses at 4 s and 9 s.
        let segments = [
            seg("a.mp4", 120, Some(4_000)),
            seg("b.mp4", 4_000, Some(9_000)),
            seg("c.mp4", 9_000, None),
        ];
        let plan = join_plan(&segments).unwrap();
        assert_eq!(plan.video_offset_ms, 120);
        assert_eq!(
            plan.parts,
            vec![
                (Path::new("a.mp4"), 0, Some(3_880)),
                (Path::new("b.mp4"), 3_880, Some(5_000)),
                (Path::new("c.mp4"), 8_880, None),
            ]
        );
    }

    #[test]
    fn a_pause_before_the_stream_was_up_drops_its_empty_segment() {
        let segments = [seg("a.mp4", 150, Some(40)), seg("b.mp4", 40, None)];
        let plan = join_plan(&segments).unwrap();
        assert_eq!(plan.video_offset_ms, 40);
        assert_eq!(plan.parts, vec![(Path::new("b.mp4"), 0, None)]);
    }

    #[test]
    fn a_capture_finished_while_paused_trims_its_last_segment() {
        let segments = [
            seg("a.mp4", 100, Some(3_000)),
            seg("b.mp4", 3_000, Some(8_000)),
        ];
        let plan = join_plan(&segments).unwrap();
        assert_eq!(
            plan.parts,
            vec![
                (Path::new("a.mp4"), 0, Some(2_900)),
                (Path::new("b.mp4"), 2_900, Some(5_000)),
            ]
        );
        assert_eq!(join_plan(&[]), None);
    }

    #[test]
    fn a_missing_segment_leaves_a_gap_rather_than_shifting_what_follows() {
        let segments = [seg("a.mp4", 100, Some(3_000)), seg("c.mp4", 6_000, None)];
        let plan = join_plan(&segments).unwrap();
        assert_eq!(plan.parts[1], (Path::new("c.mp4"), 5_900, None));
    }

    #[test]
    fn segments_after_the_first_are_numbered_beside_it() {
        let first = Path::new("/captures/x.scrawl/recording.mp4");
        assert_eq!(segment_path(first, 0), first);
        assert_eq!(
            segment_path(first, 2),
            Path::new("/captures/x.scrawl/recording-3.mp4")
        );
    }
}

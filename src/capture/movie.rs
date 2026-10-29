//! The recording's movie, written from the stream's own frames with AVAssetWriter.
//! ScreenCaptureKit's recording output stops writing for good once the stream's configuration
//! changes, and a window capture changes its crop whenever the window moves.

use std::ffi::c_void;
use std::path::Path;
use std::sync::{Arc, Condvar, Mutex, PoisonError};
use std::time::{Duration, Instant};

use block2::RcBlock;
use objc2::encode::{Encode, Encoding};
use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject};
use objc2::msg_send;
use objc2_foundation::{NSDictionary, NSError, NSNumber, NSString, NSURL};
use screencapturekit::cm::{CMClock, CMSampleBuffer, CMSampleBufferSCExt, CMTime, SCFrameStatus};
use screencapturekit::stream::output_trait::SCStreamOutputTrait;
use screencapturekit::stream::output_type::SCStreamOutputType;

use super::scrawl_log;

#[link(name = "AVFoundation", kind = "framework")]
unsafe extern "C" {
    static AVFileTypeQuickTimeMovie: &'static NSString;
    static AVMediaTypeVideo: &'static NSString;
    static AVVideoCodecKey: &'static NSString;
    static AVVideoCodecTypeH264: &'static NSString;
    static AVVideoWidthKey: &'static NSString;
    static AVVideoHeightKey: &'static NSString;
}

/// `CMTime`, as AVFoundation's methods take it.
#[repr(C)]
#[derive(Clone, Copy)]
struct Time {
    value: i64,
    timescale: i32,
    flags: u32,
    epoch: i64,
}

unsafe impl Encode for Time {
    const ENCODING: Encoding =
        Encoding::Struct("?", &[i64::ENCODING, i32::ENCODING, u32::ENCODING, i64::ENCODING]);
}

impl From<CMTime> for Time {
    fn from(t: CMTime) -> Self {
        Self { value: t.value, timescale: t.timescale, flags: t.flags, epoch: t.epoch }
    }
}

/// Two seconds, `kCMTimeFlags_Valid`.
const FRAGMENT: Time = Time { value: 2, timescale: 1, flags: 1, epoch: 0 };

/// A `CMSampleBufferRef`.
#[repr(transparent)]
struct SampleRef(*mut c_void);

unsafe impl Encode for SampleRef {
    const ENCODING: Encoding = Encoding::Pointer(&Encoding::Struct("opaqueCMSampleBuffer", &[]));
}

#[repr(C)]
struct TimingInfo {
    duration: Time,
    presentation: Time,
    decode: Time,
}

#[link(name = "CoreMedia", kind = "framework")]
unsafe extern "C" {
    fn CMSampleBufferCreateCopyWithNewTiming(
        allocator: *const c_void,
        original: *mut c_void,
        count: isize,
        timing: *const TimingInfo,
        copy: *mut *mut c_void,
    ) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
unsafe extern "C" {
    fn CFRetain(cf: *const c_void) -> *const c_void;
    fn CFRelease(cf: *const c_void);
}

/// How long a still screen goes before its last frame is written again. Frames come only when
/// the screen changes, and a movie fragment with no frame in it fails the whole movie.
const REPEAT_AFTER: Duration = Duration::from_secs(1);

const INVALID: Time = Time { value: 0, timescale: 0, flags: 0, epoch: 0 };

/// `AVAssetWriterStatusWriting`.
const WRITING: isize = 1;

struct Writer {
    writer: Retained<AnyObject>,
    input: Retained<AnyObject>,
    started: bool,
    finished: bool,
    /// The last frame written (retained), to write again while the screen is still.
    last: *mut c_void,
    last_pts: f64,
    last_at: Instant,
}

// AVAssetWriter takes its frames on the stream's queue and is repeated and finished on the
// sampler's thread, one after the other under the lock.
unsafe impl Send for Writer {}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.last.is_null() {
            unsafe { CFRelease(self.last) };
        }
    }
}

impl Writer {
    /// Appends `sample` (a `CMSampleBufferRef`) if the encoder can take it now; in real time a
    /// frame it can't take yet is dropped, as a recording would.
    fn write(&mut self, sample: *mut c_void, pts: f64) {
        let ready: bool = unsafe { msg_send![&*self.input, isReadyForMoreMediaData] };
        if !ready {
            return;
        }
        let ok: bool = unsafe { msg_send![&*self.input, appendSampleBuffer: SampleRef(sample)] };
        if !ok {
            scrawl_log(format!("Movie: a frame was refused ({})", writer_error(&self.writer)));
            self.finished = true;
            return;
        }
        self.last_pts = pts;
        self.last_at = Instant::now();
    }

    /// Writes the last frame again at `now` on the host clock.
    fn repeat(&mut self, now: CMTime) {
        let Some(pts) = now.as_seconds().filter(|&t| t > self.last_pts) else {
            return;
        };
        if self.last.is_null() {
            return;
        }
        let timing = TimingInfo {
            duration: INVALID,
            presentation: Time::from(now),
            decode: INVALID,
        };
        let mut copy = std::ptr::null_mut();
        let status = unsafe {
            CMSampleBufferCreateCopyWithNewTiming(
                std::ptr::null(),
                self.last,
                1,
                &timing,
                &mut copy,
            )
        };
        if status != 0 || copy.is_null() {
            return;
        }
        self.write(copy, pts);
        unsafe { CFRelease(copy) };
    }
}

pub struct Movie {
    writer: Mutex<Writer>,
    /// When the movie's first frame was on screen.
    first_frame: (Mutex<Option<Instant>>, Condvar),
}

impl Movie {
    /// An H.264 movie at `path`, `width` × `height` pixels.
    pub fn create(path: &Path, width: usize, height: usize) -> Result<Arc<Self>, String> {
        let _ = std::fs::remove_file(path);
        let url = NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()));
        let writer_class = AnyClass::get(c"AVAssetWriter").ok_or("no AVAssetWriter")?;
        let input_class = AnyClass::get(c"AVAssetWriterInput").ok_or("no AVAssetWriterInput")?;
        unsafe {
            let writer: Result<Retained<AnyObject>, Retained<NSError>> = msg_send![
                msg_send![writer_class, alloc],
                initWithURL: &*url,
                fileType: AVFileTypeQuickTimeMovie,
                error: _
            ];
            let writer = writer.map_err(|e| e.localizedDescription().to_string())?;
            // Written in fragments, a movie cut off by a crash still plays up to its last one,
            // so `scrawl recover` has something to rebuild from. Fragmented MPEG-4 failed to
            // finish in most test captures; a QuickTime movie never did, and it reads the same
            // under its `.mp4` name.
            let _: () = msg_send![&*writer, setMovieFragmentInterval: FRAGMENT];
            let width = NSNumber::new_usize(width);
            let height = NSNumber::new_usize(height);
            let settings = NSDictionary::<NSString, AnyObject>::from_slices(
                &[AVVideoCodecKey, AVVideoWidthKey, AVVideoHeightKey],
                &[AVVideoCodecTypeH264, &width, &height],
            );
            let input: Retained<AnyObject> = msg_send![
                msg_send![input_class, alloc],
                initWithMediaType: AVMediaTypeVideo,
                outputSettings: &*settings
            ];
            let _: () = msg_send![&*input, setExpectsMediaDataInRealTime: true];
            let can_add: bool = msg_send![&*writer, canAddInput: &*input];
            if !can_add {
                return Err("the movie can't take a video track".into());
            }
            let _: () = msg_send![&*writer, addInput: &*input];
            Ok(Arc::new(Self {
                writer: Mutex::new(Writer {
                    writer,
                    input,
                    started: false,
                    finished: false,
                    last: std::ptr::null_mut(),
                    last_pts: f64::MIN,
                    last_at: Instant::now(),
                }),
                first_frame: (Mutex::new(None), Condvar::new()),
            }))
        }
    }

    fn append(&self, sample: &CMSampleBuffer) {
        // Idle and blank frames carry no picture.
        if sample.frame_status() != Some(SCFrameStatus::Complete) {
            return;
        }
        let mut w = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if w.finished {
            return;
        }
        let time = sample.presentation_timestamp();
        let Some(pts) = time.as_seconds() else {
            return;
        };
        if !w.started {
            let ok: bool = unsafe { msg_send![&*w.writer, startWriting] };
            if !ok {
                scrawl_log(format!("Movie: could not start writing ({})", writer_error(&w.writer)));
                w.finished = true;
                return;
            }
            let _: () =
                unsafe { msg_send![&*w.writer, startSessionAtSourceTime: Time::from(time)] };
            w.started = true;
            // The frame was captured a little before it arrived.
            let age = CMClock::host_time_clock()
                .time()
                .as_seconds()
                .map_or(0.0, |now| (now - pts).clamp(0.0, 1.0));
            let at = Instant::now()
                .checked_sub(Duration::from_secs_f64(age))
                .unwrap_or_else(Instant::now);
            let (first, ready) = &self.first_frame;
            *first.lock().unwrap_or_else(PoisonError::into_inner) = Some(at);
            ready.notify_all();
        }
        // A repeat may have gone in after this frame was captured.
        if pts <= w.last_pts {
            return;
        }
        w.write(sample.as_ptr(), pts);
        if w.last_pts == pts {
            let retained = unsafe { CFRetain(sample.as_ptr()) }.cast_mut();
            let previous = std::mem::replace(&mut w.last, retained);
            if !previous.is_null() {
                unsafe { CFRelease(previous) };
            }
        }
    }

    /// Writes the last frame again once the screen has been still for `REPEAT_AFTER`.
    pub fn keep_alive(&self) {
        let mut w = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
        if w.started && !w.finished && w.last_at.elapsed() >= REPEAT_AFTER {
            w.repeat(CMClock::host_time_clock().time());
        }
    }

    /// When the first frame was on screen, waiting up to `wait` for it to arrive.
    pub fn first_frame_within(&self, wait: Duration) -> Option<Instant> {
        let (first, ready) = &self.first_frame;
        let guard = first.lock().unwrap_or_else(PoisonError::into_inner);
        let (guard, _) = ready
            .wait_timeout_while(guard, wait, |at| at.is_none())
            .unwrap_or_else(PoisonError::into_inner);
        *guard
    }

    /// Ends the movie once the stream has stopped, waiting for the file to be written.
    pub fn finish(&self) {
        let writer = {
            let mut w = self.writer.lock().unwrap_or_else(PoisonError::into_inner);
            if !w.started || w.finished {
                w.finished = true;
                return;
            }
            // A writer that failed mid-capture (a full disk) raises on the session calls below,
            // which would abort the app at the stop.
            let status: isize = unsafe { msg_send![&*w.writer, status] };
            if status != WRITING {
                w.finished = true;
                scrawl_log(format!("Movie: failed while recording ({})", writer_error(&w.writer)));
                return;
            }
            // The movie runs to the stop, not to its last frame.
            let now = CMClock::host_time_clock().time();
            w.repeat(now);
            w.finished = true;
            let _: () = unsafe { msg_send![&*w.writer, endSessionAtSourceTime: Time::from(now)] };
            let _: () = unsafe { msg_send![&*w.input, markAsFinished] };
            w.writer.clone()
        };
        let (done, finished) = std::sync::mpsc::channel();
        let block = RcBlock::new(move || {
            let _ = done.send(());
        });
        let _: () = unsafe { msg_send![&*writer, finishWritingWithCompletionHandler: &*block] };
        if finished.recv_timeout(Duration::from_secs(10)).is_err() {
            scrawl_log("Movie: still finishing after 10 s");
        }
        let status: isize = unsafe { msg_send![&*writer, status] };
        // AVAssetWriterStatusCompleted
        if status != 2 {
            scrawl_log(format!("Movie: finished with status {status} ({})", writer_error(&writer)));
        }
    }
}

fn writer_error(writer: &AnyObject) -> String {
    let error: Option<Retained<NSError>> = unsafe { msg_send![writer, error] };
    error.map_or_else(|| "no error".into(), |e| e.localizedDescription().to_string())
}

/// The stream's screen frames, into the movie.
pub struct MovieOutput(pub Arc<Movie>);

impl SCStreamOutputTrait for MovieOutput {
    fn did_output_sample_buffer(&self, sample: CMSampleBuffer, of_type: SCStreamOutputType) {
        if matches!(of_type, SCStreamOutputType::Screen) {
            self.0.append(&sample);
        }
    }
}

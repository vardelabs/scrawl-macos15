use std::ffi::c_void;
use std::mem::size_of;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};

use core_graphics_types::geometry::CGSize;
use core_video::base::{CVOptionFlags, CVTimeStamp};
use core_video::display_link::{CVDisplayLink, CVDisplayLinkRef};
use core_video::r#return::CVReturn;
use metal::{
    MTLLoadAction, MTLPixelFormat, MTLPrimitiveType, MTLResourceOptions, MTLStoreAction,
    MetalLayer, RenderPassDescriptor, RenderPipelineDescriptor, RenderPipelineState,
};
use objc2::MainThreadMarker;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSColor, NSPanel, NSScreen, NSStatusWindowLevel, NSView,
    NSWindowCollectionBehavior, NSWindowStyleMask,
};
use objc2_foundation::{NSPoint, NSRect};
use objc2_quartz_core::CALayer;

use crate::capture::SharedCaptureTelemetry;
use crate::sketch::{slice_laser_window, smooth_ink_points, smooth_laser_points};

const TAIL_DURATION_SEC: f32 = 0.42;
const MAX_SEGMENTS: usize = 16_384;
const MAX_PINS: usize = 64;
/// Side of a culling tile, in points. Each pixel runs only the segments binned into its tile.
const TILE_PT: f32 = 32.0;
/// How far a segment can colour a pixel, in points. They must stay at or past the shader's
/// cutoffs, or the stroke is clipped at tile edges: the laser glow ends at three sigma
/// (`glow_sigma` <= 6.2 * (0.28 + 0.72 * 1.25) = 7.32), the ink at `r + 2.0` (`r` <= 2.59).
const LASER_REACH_PT: f32 = 22.0;
const INK_REACH_PT: f32 = 4.6;
/// The overlay's drawable never holds more pixels than a 4K screen. A 2x ultrawide (7680×2160,
/// 16.6 MP) redrawn at 120 Hz beside a running capture made the ink lag; the compositor scales
/// the smaller drawable up, which only softens the ink's edge.
const MAX_DRAWABLE_PX: f32 = 3840.0 * 2160.0;
/// A released ink stroke holds for 650 ms, fades for 650 ms, then is gone from the overlay.
const INK_VISIBLE_MS: u64 = 1_300;

/// `scale`, lowered as far as needed to keep a `width`×`height`-point drawable within
/// `MAX_DRAWABLE_PX`, and never below 1.
fn drawable_scale(width: f32, height: f32, scale: f32) -> f32 {
    let pixels = width * height * scale * scale;
    if pixels <= MAX_DRAWABLE_PX {
        return scale;
    }
    (scale * (MAX_DRAWABLE_PX / pixels).sqrt()).max(1.0)
}

/// What a frame shows when no ink, pin, laser tip or glow is on screen: only the target
/// outline, which doesn't change while it stays the same, so such a frame is drawn once.
#[derive(Clone, Copy, PartialEq)]
struct QuietFrame {
    target_rect: [f32; 4],
    constrained: bool,
    mode: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct LaserUniforms {
    resolution: [f32; 2],
    scale: f32,
    now_sec: f32,
    tail_duration: f32,
    tip_active: f32,
    tip_pos: [f32; 2],
    segment_count: u32,
    pin_count: u32,
    constrain_rect: f32,
    _pad1: f32,
    target_rect: [f32; 4],
    glow: [f32; 4],
    tile_cols: u32,
    tile_rows: u32,
    tile_size: f32,
    _pad2: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PathSegment {
    a: [f32; 2],
    b: [f32; 2],
    w0: f32,
    w1: f32,
    kind: f32,
    width: f32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct PinMarker {
    pos: [f32; 2],
    radius: f32,
    _pad: f32,
}

pub struct MetalLaserOverlay {
    window: Retained<NSPanel>,
    display_link: CVDisplayLink,
    display_link_context: *const SharedMetalOverlay,
    /// The screen it covers, global points.
    screen: [f64; 4],
}

/// The screen for a rect in global points: the one holding its centre (else most of it), else
/// the one under the pointer. Returns it with its AppKit frame, scale and display.
fn pick_screen(
    mtm: MainThreadMarker,
    on: Option<[f64; 4]>,
) -> Option<(crate::screens::Screen, NSRect)> {
    let screens = crate::screens::current();
    let index = on
        .and_then(|r| crate::screens::index_for_rect(&screens, r))
        .or_else(|| crate::screens::pointer().and_then(|p| crate::screens::index_at(&screens, p)))
        .or(if screens.is_empty() { None } else { Some(0) })?;
    let screen = screens[index];
    let primary_h = NSScreen::screens(mtm).firstObject()?.frame().size.height;
    let [x, y, w, h] = crate::screens::flip(screen.frame, primary_h);
    Some((
        screen,
        NSRect::new(NSPoint::new(x, y), objc2_foundation::NSSize::new(w, h)),
    ))
}

impl MetalLaserOverlay {
    /// Opens over the screen holding the centre of `on` (global points, top-left origin, like
    /// capture targets), else the screen under the pointer.
    pub fn open(
        telemetry: Arc<SharedCaptureTelemetry>,
        on: Option<[f64; 4]>,
    ) -> Result<Self, String> {
        let mtm = MainThreadMarker::new().ok_or("Metal overlay requires the main thread")?;
        let (screen, frame) =
            pick_screen(mtm, on).ok_or("No screen available for Metal overlay")?;
        let scale = (screen.scale as f32).max(1.0);
        let [ox, oy, _, _] = screen.frame;

        let shared = Arc::new(SharedMetalOverlay::new(
            telemetry,
            [ox as f32, oy as f32],
            frame.size.width as f32,
            frame.size.height as f32,
            scale,
        )?);

        let window = NSPanel::initWithContentRect_styleMask_backing_defer(
            mtm.alloc(),
            frame,
            NSWindowStyleMask::Borderless
                | NSWindowStyleMask::UtilityWindow
                | NSWindowStyleMask::NonactivatingPanel
                | NSWindowStyleMask::FullSizeContentView,
            NSBackingStoreType::Buffered,
            false,
        );
        let view =
            NSView::initWithFrame(mtm.alloc(), NSRect::new(NSPoint::new(0.0, 0.0), frame.size));
        view.setWantsLayer(true);
        let metal_layer = shared.layer_ptr();
        let cocoa_layer = unsafe { &*metal_layer.cast::<CALayer>() };
        cocoa_layer.setOpaque(false);
        view.setLayer(Some(cocoa_layer));
        window.setContentView(Some(&view));
        window.setBackgroundColor(Some(&NSColor::clearColor()));
        window.setOpaque(false);
        window.setIgnoresMouseEvents(true);
        window.setHasShadow(false);
        window.setHidesOnDeactivate(false);
        window.setCanHide(false);
        window.setLevel(NSStatusWindowLevel + 1);
        window.setCollectionBehavior(
            NSWindowCollectionBehavior::CanJoinAllSpaces
                | NSWindowCollectionBehavior::CanJoinAllApplications
                | NSWindowCollectionBehavior::FullScreenAuxiliary
                | NSWindowCollectionBehavior::Stationary
                | NSWindowCollectionBehavior::IgnoresCycle,
        );
        unsafe { window.setReleasedWhenClosed(false) };
        window.setFrame_display(frame, true);
        // Present Frame 0 synchronously so the cutout and border are already on screen
        // before TargetPickerWindow closes (zero 1-frame blink).
        shared.draw();
        window.orderFrontRegardless();

        let display_link = CVDisplayLink::from_active_cg_displays()
            .map_err(|status| format!("Could not create CVDisplayLink: {status}"))?;
        if screen.id != 0 {
            let _ = display_link.set_current_cg_display(screen.id);
        }
        let display_link_context = Arc::into_raw(shared);
        unsafe {
            display_link.set_output_callback(
                display_link_callback,
                display_link_context.cast_mut().cast::<c_void>(),
            )
        }
        .map_err(|status| format!("Could not configure CVDisplayLink callback: {status}"))?;
        let _ = display_link.start();

        Ok(Self {
            window,
            display_link,
            display_link_context,
            screen: screen.frame,
        })
    }

    pub fn window_number(&self) -> u32 {
        self.window.windowNumber().max(0) as u32
    }

    /// The screen it covers, global points.
    pub fn screen(&self) -> [f64; 4] {
        self.screen
    }

    /// Moves to the screen `rect` (global points) belongs to, taking its size, scale and origin.
    /// Returns whether it moved.
    pub fn move_to(&mut self, rect: [f64; 4]) -> bool {
        let Some(mtm) = MainThreadMarker::new() else {
            return false;
        };
        let Some((screen, frame)) = pick_screen(mtm, Some(rect)) else {
            return false;
        };
        if same_rect(screen.frame, self.screen) {
            return false;
        }
        // Window first: AppKit sets the layer's contentsScale for the new screen as it moves,
        // and `place` then sets the size and scale the shader draws at.
        self.window.setFrame_display(frame, true);
        let shared = unsafe { &*self.display_link_context };
        shared.place(
            [screen.frame[0] as f32, screen.frame[1] as f32],
            frame.size.width as f32,
            frame.size.height as f32,
            (screen.scale as f32).max(1.0),
        );
        if screen.id != 0 {
            let _ = self.display_link.set_current_cg_display(screen.id);
        }
        self.screen = screen.frame;
        shared.draw();
        true
    }
}

fn same_rect(a: [f64; 4], b: [f64; 4]) -> bool {
    (0..4).all(|i| (a[i] - b[i]).abs() < 1.0)
}

impl Drop for MetalLaserOverlay {
    fn drop(&mut self) {
        let _ = self.display_link.stop();
        unsafe { drop(Arc::from_raw(self.display_link_context)) };
        self.window.orderOut(None);
        self.window.close();
    }
}

extern "C" fn display_link_callback(
    _display_link: CVDisplayLinkRef,
    _now: *const CVTimeStamp,
    _output_time: *const CVTimeStamp,
    _flags_in: CVOptionFlags,
    _flags_out: *mut CVOptionFlags,
    context: *mut c_void,
) -> CVReturn {
    if !context.is_null() {
        let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let overlay = unsafe { &*context.cast::<SharedMetalOverlay>() };
            overlay.draw();
        }));
    }
    0
}

struct SharedMetalOverlay {
    telemetry: Arc<SharedCaptureTelemetry>,
    renderer: Mutex<MetalLaserRenderer>,
}

unsafe impl Send for SharedMetalOverlay {}
unsafe impl Sync for SharedMetalOverlay {}

struct MetalLaserRenderer {
    layer: MetalLayer,
    device: metal::Device,
    command_queue: metal::CommandQueue,
    pipeline: RenderPipelineState,
    /// The screen's top-left in global points: telemetry is global, the shader draws locally.
    origin: [f32; 2],
    width: f32,
    height: f32,
    scale: f32,
    /// The last frame presented, when it was quiet; `None` after anything moved on screen.
    shown_quiet: Option<QuietFrame>,
}

impl SharedMetalOverlay {
    fn new(
        telemetry: Arc<SharedCaptureTelemetry>,
        origin: [f32; 2],
        width: f32,
        height: f32,
        scale: f32,
    ) -> Result<Self, String> {
        let scale = drawable_scale(width, height, scale);
        let device = metal::Device::system_default().ok_or("Metal is unavailable")?;
        let layer = MetalLayer::new();
        layer.set_device(&device);
        layer.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        layer.set_opaque(false);
        layer.set_presents_with_transaction(false);
        layer.set_display_sync_enabled(true);
        layer.set_contents_scale(f64::from(scale));
        layer.set_drawable_size(CGSize::new(
            f64::from(width * scale),
            f64::from(height * scale),
        ));

        let pipeline = overlay_pipeline(&device)?;

        Ok(Self {
            telemetry,
            renderer: Mutex::new(MetalLaserRenderer {
                command_queue: device.new_command_queue(),
                device,
                layer,
                pipeline,
                origin,
                width,
                height,
                scale,
                shown_quiet: None,
            }),
        })
    }

    /// Takes another screen's origin, size and scale (`MetalLaserOverlay::move_to`).
    fn place(&self, origin: [f32; 2], width: f32, height: f32, scale: f32) {
        let scale = drawable_scale(width, height, scale);
        let mut renderer = self
            .renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        renderer.origin = origin;
        renderer.shown_quiet = None;
        renderer.width = width;
        renderer.height = height;
        renderer.scale = scale;
        renderer.layer.set_contents_scale(f64::from(scale));
        renderer.layer.set_drawable_size(CGSize::new(
            f64::from(width * scale),
            f64::from(height * scale),
        ));
    }

    fn origin(&self) -> [f32; 2] {
        self.renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .origin
    }

    fn layer_ptr(&self) -> *const metal::MetalLayerRef {
        self.renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .layer
            .as_ref()
    }

    fn draw(&self) {
        let is_active = self.telemetry.active.load(Ordering::Acquire);
        let is_selecting = self.telemetry.selecting.load(Ordering::Acquire);
        let is_device_pending = self.telemetry.capture_mode.load(Ordering::Acquire) == 3;
        if !is_active && !is_selecting && !is_device_pending {
            return;
        }
        let now_ms = self.telemetry.elapsed_ms();
        let now_sec = if is_selecting {
            0.0
        } else {
            now_ms as f32 / 1_000.0
        };
        let picker_fade = if is_selecting {
            let elapsed = self
                .telemetry
                .picker_started_at
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .elapsed()
                .as_secs_f32();
            let t = (elapsed / 0.15).clamp(0.0, 1.0);
            1.0 - (1.0 - t).powi(3)
        } else {
            1.0
        };
        let tail_ms = (TAIL_DURATION_SEC * 1_000.0) as u64;
        let min_laser_ms = now_ms.saturating_sub(tail_ms);

        let picker_sel = if is_selecting {
            *self
                .telemetry
                .picker_rect
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
        } else {
            None
        };
        let target_frame = *self
            .telemetry
            .target_frame
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let (tx, ty, tw, th, is_constrained, mode_code) = if is_selecting {
            if let Some(([rx, ry, rw, rh], kind)) = picker_sel {
                (rx, ry, rw.max(1.0), rh.max(1.0), true, f32::from(kind))
            } else {
                (0.0, 0.0, 1.0, 1.0, false, -picker_fade.max(0.001))
            }
        } else {
            let tx = target_frame.x as f32;
            let ty = target_frame.y as f32;
            let tw = (target_frame.width as f32).max(1.0);
            let th = (target_frame.height as f32).max(1.0);
            let mode = self.telemetry.capture_mode.load(Ordering::Acquire) as f32;
            let constrained = mode > 0.5 && tx > -10_000.0;
            (tx, ty, tw, th, constrained, mode)
        };
        let [ox, oy] = self.origin();
        let (tx, ty) = (tx - ox, ty - oy);
        let to_screen = |nx: f32, ny: f32| -> [f32; 2] { [tx + nx * tw, ty + ny * th] };

        // Only what can still show is copied: every stroke of the recording, cloned under the
        // lock the input thread appends to, 120 times a second, was the cost that made long
        // recordings lag. Pins stay; ink is gone 1.3 s after release, the laser after its tail.
        let committed_strokes: Vec<crate::model::Stroke> = self
            .telemetry
            .strokes
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .filter(|stroke| {
                stroke.pin.is_some()
                    || if stroke.laser {
                        stroke.t1 >= min_laser_ms
                    } else {
                        now_ms.saturating_sub(stroke.t1) < INK_VISIBLE_MS
                    }
            })
            .cloned()
            .collect();
        let active_laser = self
            .telemetry
            .active_laser
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        let active_ink = self
            .telemetry
            .live_ink
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();

        let mut strokes: Vec<(crate::model::Stroke, bool)> =
            committed_strokes.into_iter().map(|s| (s, false)).collect();
        if let Some(laser) = active_laser {
            strokes.push((laser, true));
        }
        if let Some(ink) = active_ink {
            strokes.push((ink, true));
        }

        let mut segments = Vec::with_capacity(256);
        let mut pins = Vec::new();

        for (stroke, is_live) in &strokes {
            if stroke.pin.is_some() {
                if let Some([nx, ny]) = stroke.normalized_points().first().copied()
                    && pins.len() < MAX_PINS
                {
                    pins.push(PinMarker {
                        pos: to_screen(nx, ny),
                        radius: 11.5,
                        _pad: 0.0,
                    });
                }
                continue;
            }
            if stroke.laser {
                if stroke.t1 < min_laser_ms {
                    continue;
                }
                let raw = slice_laser_window(&stroke.timed_points(), now_ms, tail_ms, to_screen);
                let smoothed = smooth_laser_points(&raw);
                for pair in smoothed.windows(2) {
                    let (a, w0) = pair[0];
                    let (b, w1) = pair[1];
                    segments.push(PathSegment {
                        a,
                        b,
                        w0,
                        w1,
                        kind: 0.0,
                        width: 3.0,
                    });
                }
            } else {
                // Keep live Shift strokes at 100% while held, then hold for 650ms and fade out over 650ms.
                let fade_alpha = if *is_live {
                    1.0
                } else {
                    let age_ms = now_ms.saturating_sub(stroke.t1);
                    if age_ms >= INK_VISIBLE_MS {
                        continue;
                    } else if age_ms <= 650 {
                        1.0
                    } else {
                        let t = ((age_ms - 650) as f32 / 650.0).clamp(0.0, 1.0);
                        (1.0 - t) * (1.0 - t)
                    }
                };
                let pts: Vec<[f32; 2]> = stroke
                    .normalized_points()
                    .into_iter()
                    .map(|[nx, ny]| to_screen(nx, ny))
                    .collect();
                let smoothed = smooth_ink_points(&pts);
                for pair in smoothed.windows(2) {
                    segments.push(PathSegment {
                        a: pair[0].0,
                        b: pair[1].0,
                        w0: pair[0].1,
                        w1: pair[1].1,
                        kind: 1.0,
                        width: fade_alpha,
                    });
                }
            }
        }

        // Strokes run oldest to newest, the live ones last. Past the cap, drop from the front, so
        // the tip being drawn keeps showing and a long scribble never runs out of ink.
        if segments.len() > MAX_SEGMENTS {
            segments.drain(..segments.len() - MAX_SEGMENTS);
        }

        let shift_held = self.telemetry.shift_held.load(Ordering::Acquire);
        let pointer = {
            let [px, py] = *self
                .telemetry
                .pointer_screen
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            [px - ox, py - oy]
        };

        if segments.is_empty() {
            segments.push(PathSegment::default());
        }
        if pins.is_empty() {
            pins.push(PinMarker::default());
        }

        let mut renderer = self
            .renderer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let nothing_drawn = segments.len() == 1 && segments[0].width == 0.0;
        let no_pins = pins.len() == 1 && pins[0].radius == 0.0;
        let glowing = is_active && self.telemetry.start_glow.load(Ordering::Acquire);
        let quiet = (!is_selecting && !is_device_pending && (!is_active || (nothing_drawn && no_pins && !shift_held && !glowing)))
            .then_some(QuietFrame {
                target_rect: [tx, ty, tw, th],
                constrained: is_constrained,
                mode: mode_code,
            });
        if quiet.is_some() && quiet == renderer.shown_quiet {
            return;
        }
        renderer.shown_quiet = quiet;
        let tile_cols = (renderer.width / TILE_PT).ceil().max(1.0) as u32;
        let tile_rows = (renderer.height / TILE_PT).ceil().max(1.0) as u32;
        objc2::rc::autoreleasepool(|_| {
            let Some(drawable) = renderer.layer.next_drawable() else {
                return;
            };
            let pass = RenderPassDescriptor::new();
            let attachment = pass.color_attachments().object_at(0).unwrap();
            attachment.set_texture(Some(drawable.texture()));
            attachment.set_load_action(MTLLoadAction::Clear);
            attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            attachment.set_store_action(MTLStoreAction::Store);

            let segment_count = if !is_active || (segments.len() == 1 && segments[0].width == 0.0) {
                0
            } else {
                segments.len().min(MAX_SEGMENTS)
            };
            let uniforms = LaserUniforms {
                resolution: [
                    renderer.width * renderer.scale,
                    renderer.height * renderer.scale,
                ],
                scale: renderer.scale,
                now_sec,
                tail_duration: picker_fade,
                tip_active: if is_active && shift_held { 1.0 } else { 0.0 },
                tip_pos: pointer,
                segment_count: segment_count as u32,
                pin_count: if !is_active || (pins.len() == 1 && pins[0].radius == 0.0) {
                    0
                } else {
                    pins.len().min(MAX_PINS) as u32
                },
                constrain_rect: if is_constrained { 1.0 } else { 0.0 },
                _pad1: mode_code,
                target_rect: [tx, ty, tw, th],
                glow: [
                    f32::from(u8::from(
                        is_active && self.telemetry.start_glow.load(Ordering::Acquire),
                    )),
                    0.0,
                    0.0,
                    0.0,
                ],
                tile_cols,
                tile_rows,
                tile_size: TILE_PT,
                _pad2: 0.0,
            };

            let cmd = renderer.command_queue.new_command_buffer();
            encode_overlay(
                cmd,
                pass,
                &renderer.device,
                &renderer.pipeline,
                &uniforms,
                &segments,
                &pins,
            );
            cmd.present_drawable(drawable);
            cmd.commit();
        });
    }
}

/// Sorts segments into a `cols`×`rows` grid of `size`-point tiles by the area each can colour.
/// One buffer: `cols * rows + 1` offsets, then each tile's segment indices from its offset to
/// the next tile's. A segment off the screen lands in no tile.
fn bin_segments(segments: &[PathSegment], cols: u32, rows: u32, size: f32) -> Vec<u32> {
    let tile_count = (cols * rows) as usize;
    let span = |seg: &PathSegment| {
        let reach = if seg.kind < 0.5 {
            LASER_REACH_PT
        } else {
            INK_REACH_PT
        };
        let lo = [
            seg.a[0].min(seg.b[0]) - reach,
            seg.a[1].min(seg.b[1]) - reach,
        ];
        let hi = [
            seg.a[0].max(seg.b[0]) + reach,
            seg.a[1].max(seg.b[1]) + reach,
        ];
        if hi[0] < 0.0
            || hi[1] < 0.0
            || lo[0] >= cols as f32 * size
            || lo[1] >= rows as f32 * size
        {
            return None;
        }
        let cell = |v: f32, n: u32| ((v / size).max(0.0) as u32).min(n - 1);
        Some((
            cell(lo[0], cols)..=cell(hi[0], cols),
            cell(lo[1], rows)..=cell(hi[1], rows),
        ))
    };
    let mut table = vec![0u32; tile_count + 1];
    for (cols_span, rows_span) in segments.iter().filter_map(span) {
        for row in rows_span {
            for col in cols_span.clone() {
                table[(row * cols + col) as usize + 1] += 1;
            }
        }
    }
    for i in 1..=tile_count {
        table[i] += table[i - 1];
    }
    let mut next: Vec<u32> = table[..tile_count].to_vec();
    table.resize(tile_count + 1 + table[tile_count] as usize, 0);
    for (index, seg) in segments.iter().enumerate() {
        let Some((cols_span, rows_span)) = span(seg) else {
            continue;
        };
        for row in rows_span {
            for col in cols_span.clone() {
                let tile = (row * cols + col) as usize;
                table[tile_count + 1 + next[tile] as usize] = index as u32;
                next[tile] += 1;
            }
        }
    }
    table
}

fn overlay_pipeline(device: &metal::DeviceRef) -> Result<RenderPipelineState, String> {
    let library = device
        .new_library_with_data(include_bytes!(concat!(
            env!("OUT_DIR"),
            "/laser_overlay.metallib"
        )))
        .map_err(|e| format!("Could not load laser shader library: {e}"))?;
    let vertex = library
        .get_function("laser_vertex", None)
        .map_err(|e| format!("Could not load laser_vertex: {e}"))?;
    let fragment = library
        .get_function("laser_fragment", None)
        .map_err(|e| format!("Could not load laser_fragment: {e}"))?;

    let descriptor = RenderPipelineDescriptor::new();
    descriptor.set_vertex_function(Some(&vertex));
    descriptor.set_fragment_function(Some(&fragment));
    let attachment = descriptor
        .color_attachments()
        .object_at(0)
        .ok_or("Missing color attachment")?;
    attachment.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
    attachment.set_blending_enabled(false);

    device
        .new_render_pipeline_state(&descriptor)
        .map_err(|e| format!("Could not create laser pipeline: {e}"))
}

fn encode_overlay(
    cmd: &metal::CommandBufferRef,
    pass: &metal::RenderPassDescriptorRef,
    device: &metal::DeviceRef,
    pipeline: &RenderPipelineState,
    uniforms: &LaserUniforms,
    segments: &[PathSegment],
    pins: &[PinMarker],
) {
    let enc = cmd.new_render_command_encoder(pass);
    enc.set_render_pipeline_state(pipeline);
    enc.set_fragment_bytes(
        0,
        size_of::<LaserUniforms>() as u64,
        std::ptr::from_ref(uniforms).cast(),
    );
    // Up to MAX_SEGMENTS * 32 B = 512 KB, past the 4 KB `set_fragment_bytes` limit (128
    // segments), so segments go in a fresh buffer each frame. The command buffer retains it
    // until the GPU is done, so a frame never overwrites one still being read.
    let segment_buffer = device.new_buffer_with_data(
        segments.as_ptr().cast(),
        (segments.len() * size_of::<PathSegment>()) as u64,
        MTLResourceOptions::StorageModeShared,
    );
    enc.set_fragment_buffer(1, Some(&segment_buffer), 0);
    enc.set_fragment_bytes(
        2,
        (pins.len() * size_of::<PinMarker>()) as u64,
        pins.as_ptr().cast(),
    );
    let tiles = bin_segments(
        &segments[..uniforms.segment_count as usize],
        uniforms.tile_cols,
        uniforms.tile_rows,
        uniforms.tile_size,
    );
    let tile_buffer = device.new_buffer_with_data(
        tiles.as_ptr().cast(),
        size_of_val(tiles.as_slice()) as u64,
        MTLResourceOptions::StorageModeShared,
    );
    enc.set_fragment_buffer(3, Some(&tile_buffer), 0);
    enc.draw_primitives(MTLPrimitiveType::Triangle, 0, 3);
    enc.end_encoding();
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `setFragmentBytes` aborts in the driver past 4 KB.
    const SET_BYTES_LIMIT: usize = 4096;

    /// Uniforms and pins still go through `set_fragment_bytes`; segments don't fit and use a buffer.
    #[test]
    fn inline_fragment_bytes_fit_the_metal_limit() {
        assert!(size_of::<LaserUniforms>() <= SET_BYTES_LIMIT);
        assert!(MAX_PINS * size_of::<PinMarker>() <= SET_BYTES_LIMIT);
    }

    /// Renders the real shader offscreen with `MAX_SEGMENTS` segments (512 KB; 48 KB aborted in
    /// `setFragmentBytes`). Only the last segment is on screen, so the ink shows only if the
    /// shader read the whole array.
    #[test]
    fn renders_a_full_segment_buffer() {
        let Some(device) = metal::Device::system_default() else {
            return;
        };
        let pipeline = overlay_pipeline(&device).expect("pipeline");
        let size = 64;
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        descriptor.set_width(size);
        descriptor.set_height(size);
        descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
        descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
        let texture = device.new_texture(&descriptor);

        let off_screen = PathSegment {
            a: [-500.0, -500.0],
            b: [-400.0, -500.0],
            w0: 1.0,
            w1: 1.0,
            kind: 1.0,
            width: 1.0,
        };
        let mut segments = vec![off_screen; MAX_SEGMENTS];
        segments[MAX_SEGMENTS - 1] = PathSegment {
            a: [0.0, 32.0],
            b: [64.0, 32.0],
            ..off_screen
        };
        let uniforms = LaserUniforms {
            resolution: [size as f32, size as f32],
            scale: 1.0,
            now_sec: 1.0,
            tail_duration: 1.0,
            tip_active: 0.0,
            tip_pos: [0.0, 0.0],
            segment_count: MAX_SEGMENTS as u32,
            pin_count: 0,
            constrain_rect: 0.0,
            _pad1: 0.0,
            target_rect: [0.0, 0.0, size as f32, size as f32],
            glow: [0.0; 4],
            tile_cols: 2,
            tile_rows: 2,
            tile_size: TILE_PT,
            _pad2: 0.0,
        };

        let pass = RenderPassDescriptor::new();
        let attachment = pass.color_attachments().object_at(0).unwrap();
        attachment.set_texture(Some(&texture));
        attachment.set_load_action(MTLLoadAction::Clear);
        attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
        attachment.set_store_action(MTLStoreAction::Store);
        let queue = device.new_command_queue();
        let cmd = queue.new_command_buffer();
        encode_overlay(
            cmd,
            pass,
            &device,
            &pipeline,
            &uniforms,
            &segments,
            &[PinMarker::default()],
        );
        cmd.commit();
        cmd.wait_until_completed();

        let mut pixels = vec![0u8; (size * size * 4) as usize];
        texture.get_bytes(
            pixels.as_mut_ptr().cast(),
            size * 4,
            metal::MTLRegion::new_2d(0, 0, size, size),
            0,
        );
        let bgra = |x: u64, y: u64| {
            let i = ((y * size + x) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2], pixels[i + 3]]
        };
        let [b, g, r, a] = bgra(32, 32);
        assert!(a > 200 && r > 200 && g < 100 && b < 100, "stroke pixel {:?}", [b, g, r, a]);
        assert_eq!(bgra(32, 4)[3], 0, "nothing drawn away from the stroke");
    }

    /// Tiles only skip work: the frame matches one where every pixel runs every segment, as
    /// before tiling, byte for byte. A reach shorter than the shader's cutoff fails here as a
    /// seam along a tile edge.
    #[test]
    fn tiled_frame_matches_an_untiled_one() {
        let Some(device) = metal::Device::system_default() else {
            return;
        };
        let pipeline = overlay_pipeline(&device).expect("pipeline");
        let (width, height, scale) = (256.0_f32, 192.0_f32, 2.0_f32);
        let ink: Vec<[f32; 2]> = (0..48)
            .map(|i| {
                let t = i as f32 / 48.0 * std::f32::consts::TAU;
                [128.0 + 90.0 * t.cos(), 96.0 + 60.0 * t.sin()]
            })
            .collect();
        let mut segments: Vec<PathSegment> = smooth_ink_points(&ink)
            .windows(2)
            .map(|pair| PathSegment {
                a: pair[0].0,
                b: pair[1].0,
                w0: pair[0].1,
                w1: pair[1].1,
                kind: 1.0,
                width: 1.0,
            })
            .collect();
        // Ink at full pressure and a laser at full weight reach the furthest, both crossing
        // tile edges both ways.
        segments.extend((0..12).map(|i| PathSegment {
            a: [10.0 + i as f32 * 20.0, 170.0 - i as f32 * 13.0],
            b: [30.0 + i as f32 * 20.0, 157.0 - i as f32 * 13.0],
            w0: 1.15,
            w1: 1.15,
            kind: 1.0,
            width: 1.0,
        }));
        segments.extend((0..12).map(|i| PathSegment {
            a: [20.0 + i as f32 * 18.0, 30.0 + i as f32 * 11.0],
            b: [38.0 + i as f32 * 18.0, 41.0 + i as f32 * 11.0],
            w0: 1.25,
            w1: 1.25,
            kind: 0.0,
            width: 3.0,
        }));
        let n = segments.len() as u32;
        let uniforms = LaserUniforms {
            resolution: [width * scale, height * scale],
            scale,
            now_sec: 5.0,
            tail_duration: 1.0,
            segment_count: n,
            target_rect: [0.0, 0.0, width, height],
            tile_cols: (width / TILE_PT).ceil() as u32,
            tile_rows: (height / TILE_PT).ceil() as u32,
            tile_size: TILE_PT,
            ..LaserUniforms::default()
        };
        let render = |uniforms: &LaserUniforms| {
            let (w, h) = ((width * scale) as u64, (height * scale) as u64);
            let descriptor = metal::TextureDescriptor::new();
            descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
            descriptor.set_width(w);
            descriptor.set_height(h);
            descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
            descriptor.set_storage_mode(metal::MTLStorageMode::Shared);
            let texture = device.new_texture(&descriptor);
            let pass = RenderPassDescriptor::new();
            let attachment = pass.color_attachments().object_at(0).unwrap();
            attachment.set_texture(Some(&texture));
            attachment.set_load_action(MTLLoadAction::Clear);
            attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
            attachment.set_store_action(MTLStoreAction::Store);
            let queue = device.new_command_queue();
            let cmd = queue.new_command_buffer();
            let pins = [PinMarker::default()];
            encode_overlay(cmd, pass, &device, &pipeline, uniforms, &segments, &pins);
            cmd.commit();
            cmd.wait_until_completed();
            let mut pixels = vec![0u8; (w * h * 4) as usize];
            texture.get_bytes(
                pixels.as_mut_ptr().cast(),
                w * 4,
                metal::MTLRegion::new_2d(0, 0, w, h),
                0,
            );
            pixels
        };

        let tiled = render(&uniforms);
        // One tile bigger than the screen holds every segment.
        let untiled = render(&LaserUniforms {
            tile_cols: 1,
            tile_rows: 1,
            tile_size: f32::MAX,
            ..uniforms
        });
        assert!(tiled.iter().any(|&b| b != 0), "the scene draws something");
        let differ = tiled.chunks(4).zip(untiled.chunks(4)).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "{differ} pixels differ from the untiled frame");
    }

    /// Times one overlay frame on a 14" MacBook Pro's built-in display (2056×1329 pt at 2x)
    /// while Shift ink is on screen: a big loop (120 pointer samples, ~900 segments) and a
    /// scribble that fills `MAX_SEGMENTS`. Run with
    /// `cargo test --release overlay_frame_time -- --ignored --nocapture`.
    #[test]
    #[ignore = "benchmark"]
    fn overlay_frame_time() {
        let Some(device) = metal::Device::system_default() else {
            return;
        };
        let pipeline = overlay_pipeline(&device).expect("pipeline");
        let (width, height, scale) = (2056.0_f32, 1329.0_f32, 2.0_f32);
        let descriptor = metal::TextureDescriptor::new();
        descriptor.set_pixel_format(MTLPixelFormat::BGRA8Unorm);
        descriptor.set_width((width * scale) as u64);
        descriptor.set_height((height * scale) as u64);
        descriptor.set_usage(metal::MTLTextureUsage::RenderTarget);
        descriptor.set_storage_mode(metal::MTLStorageMode::Private);
        let texture = device.new_texture(&descriptor);

        // One big loop, then a long fast scribble that fills the whole segment cap.
        let big_loop: Vec<[f32; 2]> = (0..120)
            .map(|i| {
                let t = i as f32 / 120.0 * std::f32::consts::TAU * 1.1;
                [1028.0 + 575.0 * t.cos(), 585.0 + 290.0 * t.sin()]
            })
            .collect();
        let scribble: Vec<[f32; 2]> = (0..2_400)
            .map(|i| {
                let t = i as f32 * 0.05;
                [1028.0 + 600.0 * (t * 1.3).sin(), 600.0 + 330.0 * (t * 1.7).cos()]
            })
            .collect();
        let queue = device.new_command_queue();
        for samples in [big_loop, scribble] {
            let smoothed = smooth_ink_points(&samples);
            let segments: Vec<PathSegment> = smoothed
                .windows(2)
                .skip((smoothed.len() - 1).saturating_sub(MAX_SEGMENTS))
                .map(|pair| PathSegment {
                    a: pair[0].0,
                    b: pair[1].0,
                    w0: pair[0].1,
                    w1: pair[1].1,
                    kind: 1.0,
                    width: 1.0,
                })
                .collect();
            let uniforms = LaserUniforms {
                resolution: [width * scale, height * scale],
                scale,
                now_sec: 5.0,
                tail_duration: 1.0,
                segment_count: segments.len() as u32,
                target_rect: [0.0, 0.0, width, height],
                tile_cols: (width / TILE_PT).ceil() as u32,
                tile_rows: (height / TILE_PT).ceil() as u32,
                tile_size: TILE_PT,
                ..LaserUniforms::default()
            };
            let mut frame_ms = Vec::new();
            for frame in 0..70 {
                let pass = RenderPassDescriptor::new();
                let attachment = pass.color_attachments().object_at(0).unwrap();
                attachment.set_texture(Some(&texture));
                attachment.set_load_action(MTLLoadAction::Clear);
                attachment.set_clear_color(metal::MTLClearColor::new(0.0, 0.0, 0.0, 0.0));
                attachment.set_store_action(MTLStoreAction::Store);
                let started = std::time::Instant::now();
                let cmd = queue.new_command_buffer();
                encode_overlay(
                    cmd,
                    pass,
                    &device,
                    &pipeline,
                    &uniforms,
                    &segments,
                    &[PinMarker::default()],
                );
                cmd.commit();
                cmd.wait_until_completed();
                if frame >= 10 {
                    frame_ms.push(started.elapsed().as_secs_f64() * 1_000.0);
                }
            }
            frame_ms.sort_by(f64::total_cmp);
            println!(
                "{} samples, {} segments at {}x{} px: median {:.2} ms, p95 {:.2} ms, max {:.2} ms",
                samples.len(),
                segments.len(),
                width * scale,
                height * scale,
                frame_ms[frame_ms.len() / 2],
                frame_ms[frame_ms.len() * 95 / 100],
                frame_ms[frame_ms.len() - 1],
            );
        }
    }

    /// The shader must load on the oldest macOS Scrawl supports (`LSMinimumSystemVersion` 15.0).
    /// Without `-mmacosx-version-min`, `xcrun metal` targets the build machine's macOS and the
    /// overlay (ink, laser, edge glow) silently fails to open on older systems.
    #[test]
    fn drawable_scale_caps_big_screens_and_leaves_others_alone() {
        assert_eq!(drawable_scale(1512.0, 982.0, 2.0), 2.0);
        assert_eq!(drawable_scale(1920.0, 1080.0, 2.0), 2.0);
        let ultrawide = drawable_scale(3840.0, 1080.0, 2.0);
        assert!(ultrawide < 2.0 && ultrawide >= 1.0);
        let pixels = 3840.0 * 1080.0 * ultrawide * ultrawide;
        assert!((pixels - MAX_DRAWABLE_PX).abs() / MAX_DRAWABLE_PX < 1e-3, "{pixels}");
        assert_eq!(drawable_scale(7680.0, 4320.0, 2.0), 1.0);
    }

    #[test]
    fn shader_targets_the_minimum_supported_macos() {
        let library = include_bytes!(concat!(env!("OUT_DIR"), "/laser_overlay.metallib"));
        let text = String::from_utf8_lossy(library);
        let marker = "-apple-macosx";
        let at = text.find(marker).expect("metallib names its target");
        let major: u32 = text[at + marker.len()..]
            .split('.')
            .next()
            .and_then(|v| v.parse().ok())
            .expect("target has a major version");
        assert!(major <= 15, "laser shader targets macOS {major}, newer than 15.0");
    }
}

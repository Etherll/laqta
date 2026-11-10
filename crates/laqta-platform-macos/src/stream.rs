//! Recording input (`ScreenStream`): frames and system audio from one `SCStream`.
//!
//! `open` starts a single `SCStream` for the display that holds the area. Its screen output copies
//! each complete frame into a `Bitmap` (the `FrameSource`) and its audio output converts system
//! sound to interleaved 16-bit PCM (the `PcmSource`). The callbacks only fill two small bounded
//! buffers, so a slow encoder drops frames and never stalls capture. Nothing exists before `open`
//! and everything is released by `stop` (or when the last source is dropped). Microphones are
//! not captured here: FFmpeg opens them by name.

use std::collections::VecDeque;
use std::ffi::c_void;
use std::mem::{offset_of, size_of, size_of_val};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::{self, NonNull};
use std::slice;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError, mpsc};
use std::time::{Duration, Instant};

use block2::RcBlock;
use dispatch2::{DispatchQueue, DispatchQueueAttr, DispatchRetained};
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::AudioDevice;
use laqta_domain::{Bitmap, PixelFormat};
use laqta_ports::recording::{AudioInput, FrameSource, PcmFormat, PcmSource, RecordingInput, VideoSource};
use laqta_ports::{BoxFuture, PortError, ScreenStream};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, DefinedClass, define_class, msg_send};
use objc2_av_foundation::{
    AVCaptureDevice, AVCaptureDeviceDiscoverySession, AVCaptureDevicePosition, AVCaptureDeviceTypeBuiltInWideAngleCamera,
    AVCaptureDeviceTypeContinuityCamera, AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeMicrophone, AVMediaTypeAudio, AVMediaTypeVideo,
};
use objc2_core_foundation::{CFDictionary, CFNumber, CFNumberType, CFRetained, CGPoint, CGRect, CGSize};
use objc2_core_graphics::kCGColorSpaceSRGB;
use objc2_core_media::{
    CMAudioFormatDescriptionGetStreamBasicDescription, CMBlockBuffer, CMSampleBuffer, CMTime,
    kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
};
use objc2_core_video::{
    CVPixelBuffer, CVPixelBufferGetBaseAddress, CVPixelBufferGetBytesPerRow, CVPixelBufferGetHeight, CVPixelBufferGetPixelFormatType,
    CVPixelBufferGetWidth, CVPixelBufferLockBaseAddress, CVPixelBufferLockFlags, CVPixelBufferUnlockBaseAddress, kCVPixelFormatType_32BGRA,
    kCVReturnSuccess,
};
use objc2_foundation::{NSArray, NSError, NSObject, NSObjectProtocol, NSString};
use objc2_screen_capture_kit::{
    SCContentFilter, SCFrameStatus, SCRunningApplication, SCShareableContent, SCStream, SCStreamConfiguration, SCStreamDelegate,
    SCStreamErrorCode, SCStreamFrameInfoStatus, SCStreamOutput, SCStreamOutputType, SCWindow,
};

use crate::{displays, permissions};

// <core>
// Everything between `<core>` and `</core>` avoids Objective-C types, so its logic can be tested
// off the Mac.

const SYSTEM_AUDIO_ID: &str = "system";
const SYSTEM_AUDIO_NAME: &str = "System audio";

const RATE: u32 = 48_000;
const CHANNELS: usize = 2;
/// Frames waiting for the encoder. A full queue drops its oldest frame, so after a pause the
/// first frames are never more than this many periods old.
const FRAME_QUEUE: usize = 2;
/// How long the first frame may take before the recording fails.
const FIRST_FRAME_WAIT: Duration = Duration::from_secs(5);
/// One second of audio. A reader that falls further behind loses the oldest samples.
const RING_SAMPLES: usize = RATE as usize * CHANNELS;
/// How long `read` waits before it looks again for samples, silence due or the end.
const READ_WAIT: Duration = Duration::from_millis(100);
/// A reader that was away longer than this was paused: its backlog is stale and is dropped.
const RESUME_GAP: Duration = Duration::from_millis(500);
/// Silence is made only once the output is this many frames (100 ms) behind the wall clock.
const SILENCE_SLACK: u64 = (RATE / 10) as u64;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The state shared by the capture callbacks and the sources.
struct Shared {
    /// The pixel size of every frame.
    size: (u32, u32),
    video: Mutex<Video>,
    video_ready: Condvar,
    audio: Mutex<Audio>,
    audio_ready: Condvar,
    /// Set once a frame was dropped as unusable, so that the log gets one line, not one per frame.
    frame_warned: AtomicBool,
    audio_warned: AtomicBool,
}

#[derive(Default)]
struct Video {
    frames: VecDeque<Bitmap>,
    /// `Ok` once the stream ended cleanly, `Err` with the reason when it failed.
    end: Option<Result<(), String>>,
}

#[derive(Default)]
struct Audio {
    samples: VecDeque<i16>,
    /// Starts with the first `read` and again after a pause.
    clock: Option<Clock>,
    /// Set when the stream or the audio source ends. Once the buffer drains, `read` returns 0.
    end: bool,
}

/// Where the reader is on the wall clock: `frames` handed out since `start`, silence included.
struct Clock {
    start: Instant,
    frames: u64,
    last_read: Instant,
}

impl Audio {
    /// Moves up to `buf.len()` samples out of the buffer.
    fn fill(&mut self, buf: &mut [i16]) -> usize {
        let n = buf.len().min(self.samples.len());
        for (slot, sample) in buf.iter_mut().zip(self.samples.drain(..n)) {
            *slot = sample;
        }
        if let Some(clock) = self.clock.as_mut() {
            clock.frames += (n / CHANNELS) as u64;
            clock.last_read = Instant::now();
        }
        n
    }

    /// Zeros for the time the stream delivered nothing, at most `max` samples. 0 while the output
    /// is not behind the wall clock.
    fn silence(&mut self, max: usize) -> usize {
        let Some(clock) = self.clock.as_mut() else { return 0 };
        let due = frames_since(clock.start);
        if due <= clock.frames + SILENCE_SLACK {
            return 0;
        }
        let frames = (due - clock.frames).min((max / CHANNELS) as u64);
        clock.frames += frames;
        clock.last_read = Instant::now();
        frames as usize * CHANNELS
    }
}

/// The number of audio frames the wall clock has passed since `start`.
fn frames_since(start: Instant) -> u64 {
    (start.elapsed().as_secs_f64() * f64::from(RATE)) as u64
}

impl Shared {
    fn new(size: (u32, u32)) -> Self {
        Shared {
            size,
            video: Mutex::default(),
            video_ready: Condvar::new(),
            audio: Mutex::default(),
            audio_ready: Condvar::new(),
            frame_warned: AtomicBool::new(false),
            audio_warned: AtomicBool::new(false),
        }
    }

    fn push_frame(&self, frame: Bitmap) {
        let mut video = lock(&self.video);
        if video.end.is_some() {
            return;
        }
        if video.frames.len() >= FRAME_QUEUE {
            video.frames.pop_front();
        }
        video.frames.push_back(frame);
        drop(video);
        self.video_ready.notify_one();
    }

    fn push_audio(&self, pcm: &[i16]) {
        let mut audio = lock(&self.audio);
        if audio.end {
            return;
        }
        audio.samples.extend(pcm.iter().copied());
        let excess = audio.samples.len().saturating_sub(RING_SAMPLES);
        audio.samples.drain(..excess);
        drop(audio);
        self.audio_ready.notify_one();
    }

    /// Records how the stream ended. The first call wins, and queued frames are still delivered.
    fn finish(&self, end: Result<(), String>) {
        {
            let mut video = lock(&self.video);
            if video.end.is_none() {
                video.end = Some(end);
            }
        }
        self.end_audio();
        self.video_ready.notify_all();
    }

    /// Ends the PCM side only; the picture goes on.
    fn end_audio(&self) {
        lock(&self.audio).end = true;
        self.audio_ready.notify_all();
    }

    /// The next frame. The screen delivers frames only when something changes, so on a still
    /// picture the last frame repeats after one `period` and the recording keeps its length.
    /// The first frame gets `first_wait`, then the recording fails.
    fn next_frame(&self, last: &mut Option<Bitmap>, period: Duration, first_wait: Duration) -> Result<Option<Bitmap>, PortError> {
        let deadline = Instant::now() + if last.is_some() { period } else { first_wait };
        let mut video = lock(&self.video);
        loop {
            if let Some(frame) = video.frames.pop_front() {
                *last = Some(frame.clone());
                return Ok(Some(frame));
            }
            match &video.end {
                Some(Ok(())) => return Ok(None),
                Some(Err(reason)) => return Err(PortError::Failed(reason.clone())),
                None => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return match last {
                    Some(frame) => Ok(Some(frame.clone())),
                    None => Err(PortError::Failed(format!("no frame from the screen within {} s", first_wait.as_secs()))),
                };
            }
            video = self.video_ready.wait_timeout(video, left).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    /// Fills `buf` with samples and returns how many. It waits until there are some, or the
    /// stream ended (0). While the system plays nothing, ScreenCaptureKit may send nothing, so
    /// silence is made on the wall clock to keep the sound in step with the picture. The clock
    /// starts at the first read, and again after a pause (the reader was away), when the stale
    /// backlog is dropped too.
    fn read_pcm(&self, buf: &mut [i16]) -> usize {
        if buf.is_empty() {
            return 0;
        }
        let mut audio = lock(&self.audio);
        if audio.clock.as_ref().is_none_or(|clock| clock.last_read.elapsed() > RESUME_GAP) {
            audio.samples.clear();
            let now = Instant::now();
            audio.clock = Some(Clock { start: now, frames: 0, last_read: now });
        }
        loop {
            if !audio.samples.is_empty() {
                return audio.fill(buf);
            }
            if audio.end {
                return 0;
            }
            let silence = audio.silence(buf.len());
            if silence > 0 {
                buf[..silence].fill(0);
                return silence;
            }
            audio = self.audio_ready.wait_timeout(audio, READ_WAIT).unwrap_or_else(PoisonError::into_inner).0;
        }
    }

    fn warn_once(flag: &AtomicBool, what: &str, why: &str) {
        if !flag.swap(true, Ordering::Relaxed) {
            tracing::warn!("screen recording: {what} dropped: {why} (further ones are not logged)");
        }
    }
}

/// Copies `rows` rows of `row_bytes` out of `src`, whose rows start `stride` bytes apart, into one
/// tight buffer. The one copy per frame. `None` when `src` is too short.
fn copy_rows(src: &[u8], stride: usize, row_bytes: usize, rows: usize) -> Option<Arc<[u8]>> {
    if rows == 0 || stride < row_bytes || src.len() < stride * (rows - 1) + row_bytes {
        return None;
    }
    // SAFETY: all-zero bytes are valid `u8`s. Zeroed allocation avoids a second pass over the
    // pixels that `Arc::from(Vec)` would need.
    let mut out = unsafe { Arc::<[u8]>::new_zeroed_slice(row_bytes * rows).assume_init() };
    let dst = Arc::get_mut(&mut out)?;
    for (to, from) in dst.chunks_exact_mut(row_bytes).zip(src.chunks(stride)) {
        to.copy_from_slice(&from[..row_bytes]);
    }
    Some(out)
}

/// One channel of float samples inside an audio buffer: `data[offset + frame * stride]`.
struct Channel<'a> {
    data: &'a [f32],
    stride: usize,
    offset: usize,
}

impl Channel<'_> {
    fn frames(&self) -> usize {
        self.data.len().saturating_sub(self.offset).div_ceil(self.stride.max(1))
    }

    fn at(&self, frame: usize) -> f32 {
        self.data.get(frame * self.stride + self.offset).copied().unwrap_or(0.0)
    }
}

fn to_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Appends the first two channels (a mono source is doubled) as interleaved 16-bit samples.
fn stereo_i16(channels: &[Channel<'_>], out: &mut Vec<i16>) {
    let Some(left) = channels.first() else { return };
    let right = channels.get(1).unwrap_or(left);
    let frames = left.frames().min(right.frames());
    out.reserve(frames * CHANNELS);
    for frame in 0..frames {
        out.push(to_i16(left.at(frame)));
        out.push(to_i16(right.at(frame)));
    }
}

/// A rect in points.
#[derive(Clone, Copy, Debug, PartialEq)]
struct PtRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
}

/// The part of a display to record.
#[derive(Debug, PartialEq)]
struct Region {
    /// In points, relative to the display's top-left corner.
    rect: PtRect,
    /// The output size, in pixels, even on both sides (encoders need that).
    pixels: (u32, u32),
}

/// `area` (global points) clipped to `display` (global points, `scale` pixels per point). `None`
/// when less than two pixels are left on a side.
fn local_region(display: PtRect, scale: f64, area: PtRect) -> Option<Region> {
    let x0 = (area.x - display.x).max(0.0);
    let y0 = (area.y - display.y).max(0.0);
    let x1 = (area.x + area.w - display.x).min(display.w);
    let y1 = (area.y + area.h - display.y).min(display.h);
    let even = |points: f64| ((points * scale).round().max(0.0) as u32) & !1;
    let (w, h) = (even(x1 - x0), even(y1 - y0));
    if w < 2 || h < 2 {
        return None;
    }
    // The rect covers exactly the pixels, so SCK does not rescale.
    Some(Region { rect: PtRect { x: x0, y: y0, w: f64::from(w) / scale, h: f64::from(h) / scale }, pixels: (w, h) })
}
// </core>

/// The Screen Recording and audio capture of macOS. It keeps no state: each `open` starts its
/// own stream.
#[derive(Debug, Default)]
pub struct MacStream;

impl ScreenStream for MacStream {
    fn open(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>> {
        let audio = audio.to_vec();
        Box::pin(blocking::unblock(move || open_input(area, show_cursor, fps, &audio)))
    }

    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>> {
        Box::pin(blocking::unblock(|| Ok(list_audio_devices())))
    }

    fn camera_devices(&self) -> BoxFuture<'_, Result<Vec<String>, PortError>> {
        Box::pin(blocking::unblock(|| Ok(list_cameras())))
    }
}

/// Builds the recording input for `area`. Loopback devices share the stream's audio output; every
/// other device is passed through to FFmpeg's `avfoundation` by name.
fn open_input(area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> Result<RecordingInput, PortError> {
    permissions::screen_recording()?;
    let displays = displays::list();
    let (display, gx, gy, gw, gh) =
        displays::to_points(&displays, area).ok_or_else(|| PortError::Failed("no display holds the recording area".into()))?;
    let bounds = PtRect { x: display.x, y: display.y, w: display.width, h: display.height };
    let region = local_region(bounds, display.scale, PtRect { x: gx, y: gy, w: gw, h: gh })
        .ok_or_else(|| PortError::Failed("the area is smaller than two pixels on its display".into()))?;
    let fps = fps.max(1);
    let system_audio = audio.iter().any(|device| device.loopback);

    let session = Session::start(display.id, &region, show_cursor, fps, system_audio)?;
    let mut inputs = Vec::with_capacity(audio.len());
    let mut pcm = system_audio.then(|| Box::new(MacPcm { session: Arc::clone(&session) }));
    for device in audio {
        if !device.loopback {
            inputs.push(AudioInput::Device(device.clone()));
        } else if let Some(source) = pcm.take() {
            // The stream has one system-audio output, however many loopback devices were asked for.
            inputs.push(AudioInput::Pcm(source));
        }
    }
    let frames = MacFrames { session, size: region.pixels, fps, last: None };
    Ok(RecordingInput { video: VideoSource::Frames(Box::new(frames)), audio: inputs })
}

/// A running `SCStream` and the objects that must outlive it.
struct Live {
    stream: Retained<SCStream>,
    _output: Retained<StreamOutput>,
    _queues: [DispatchRetained<DispatchQueue>; 2],
}

// SAFETY: `SCStream` documents no thread affinity for starting, stopping or releasing it, and a
// `Live` is touched only under the `Session` mutex. The output's state is an `Arc` of
// mutex-guarded data and dispatch queues are thread-safe. Nothing else in `Live` is accessed
// after construction.
unsafe impl Send for Live {}

/// The stream of one recording, shared by its frame source and PCM source.
struct Session {
    live: Mutex<Option<Live>>,
    shared: Arc<Shared>,
}

/// How long the stream gets to start or stop before the call fails.
const STREAM_WAIT: Duration = Duration::from_secs(5);

impl Session {
    /// Builds the stream for `display_id`, without our own windows, and starts it.
    fn start(display_id: u32, region: &Region, show_cursor: bool, fps: u32, system_audio: bool) -> Result<Arc<Session>, PortError> {
        // The pointer highlight is one of our windows and belongs in the video, so it has to exist
        // before the windows are listed.
        let highlight = crate::disc::prepare();
        let content = shareable_content()?.0;
        let filter = content_filter(&content, display_id, highlight)?;
        let config = configuration(region, show_cursor, fps, system_audio);
        let shared = Arc::new(Shared::new(region.pixels));
        let output = StreamOutput::new(Arc::clone(&shared));
        let video_queue = DispatchQueue::new("laqta.stream.video", DispatchQueueAttr::SERIAL);
        let audio_queue = DispatchQueue::new("laqta.stream.audio", DispatchQueueAttr::SERIAL);

        // SAFETY: the filter, configuration and delegate are valid for the call, and `alloc` gives
        // a fresh instance for the initialiser.
        let stream = unsafe {
            SCStream::initWithFilter_configuration_delegate(SCStream::alloc(), &filter, &config, Some(ProtocolObject::from_ref(&*output)))
        };
        let sink = ProtocolObject::from_ref(&*output);
        // SAFETY: the stream and the output are valid, and the queue outlives the stream (it is
        // kept in `Live`).
        let screen = unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(sink, SCStreamOutputType::Screen, Some(&*video_queue)) };
        screen.map_err(|e| PortError::Failed(format!("screen output: {}", e.localizedDescription())))?;
        if system_audio {
            // SAFETY: as above.
            let sound =
                unsafe { stream.addStreamOutput_type_sampleHandlerQueue_error(sink, SCStreamOutputType::Audio, Some(&*audio_queue)) };
            sound.map_err(|e| PortError::Failed(format!("audio output: {}", e.localizedDescription())))?;
        }

        let live = Live { stream, _output: output, _queues: [video_queue, audio_queue] };
        let session = Arc::new(Session { live: Mutex::new(Some(live)), shared });
        // On failure the session is dropped here, which stops the half-started stream.
        session.begin()?;
        Ok(session)
    }

    fn begin(&self) -> Result<(), PortError> {
        let live = lock(&self.live);
        let Some(live) = live.as_ref() else { return Err(PortError::Failed("the screen stream was stopped".into())) };
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |error: *mut NSError| {
            let _ = tx.send(completion(error));
        });
        // SAFETY: the stream is valid and the block lives until the call returns; SCK copies it
        // when it needs it for later.
        unsafe { live.stream.startCaptureWithCompletionHandler(Some(&*done)) };
        match rx.recv_timeout(STREAM_WAIT) {
            Ok(Ok(())) => Ok(()),
            Ok(Err((code, text))) => Err(sck_error(code, &text, "start the screen stream")),
            Err(_) => Err(PortError::Failed("the screen stream did not start in time".into())),
        }
    }

    /// Stops the stream and releases it. A second call does nothing. The readers see the end.
    fn stop(&self) {
        self.shared.finish(Ok(()));
        let Some(live) = lock(&self.live).take() else { return };
        let (tx, rx) = mpsc::channel();
        let done = RcBlock::new(move |_error: *mut NSError| {
            let _ = tx.send(());
        });
        // SAFETY: as in `begin`. The stream was never started when `begin` failed, and SCK then
        // reports an error to the handler, which is ignored.
        unsafe { live.stream.stopCaptureWithCompletionHandler(Some(&*done)) };
        if rx.recv_timeout(STREAM_WAIT).is_err() {
            tracing::warn!("the screen stream did not confirm that it stopped");
        }
        // `live` drops here: the stream, its output and the queues are released.
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop();
    }
}

/// The error of a completion handler as `(code, text)`; `Ok` for a null error.
fn completion(error: *mut NSError) -> Result<(), (isize, String)> {
    // SAFETY: SCK passes null or an NSError that is valid during the callback.
    match unsafe { error.as_ref() } {
        None => Ok(()),
        Some(e) => Err((e.code(), e.localizedDescription().to_string())),
    }
}

/// A ScreenCaptureKit error as a port error: the user declining Screen Recording is
/// `PermissionDenied`.
fn sck_error(code: isize, text: &str, what: &str) -> PortError {
    if code == SCStreamErrorCode::UserDeclined.0 {
        PortError::PermissionDenied
    } else {
        PortError::Failed(format!("{what}: {text} (code {code})"))
    }
}

/// An Objective-C object handed to the one thread that waits for it.
struct Handoff<T>(T);

// SAFETY: used only to move a retained ScreenCaptureKit result from the completion handler's
// thread to the thread waiting for it. The sender never touches it again, and these objects are
// immutable snapshots.
unsafe impl<T> Send for Handoff<T> {}

/// The windows, displays and apps ScreenCaptureKit can capture. This is also the call that makes
/// macOS check Screen Recording for the app.
fn shareable_content() -> Result<Handoff<Retained<SCShareableContent>>, PortError> {
    let (tx, rx) = mpsc::channel();
    let done = RcBlock::new(move |content: *mut SCShareableContent, error: *mut NSError| {
        // SAFETY: SCK passes null or valid objects; `retain` keeps the content past the callback.
        let result = match unsafe { Retained::retain(content) } {
            Some(content) => Ok(Handoff(content)),
            None => Err(completion(error).err().unwrap_or((0, "no content".into()))),
        };
        let _ = tx.send(result);
    });
    // SAFETY: the block is valid for the call. Desktop windows are not needed; off-screen windows
    // are kept so that our own app is listed whatever state its windows are in.
    unsafe {
        SCShareableContent::getShareableContentExcludingDesktopWindows_onScreenWindowsOnly_completionHandler(true, false, &done);
    }
    match rx.recv_timeout(STREAM_WAIT) {
        Ok(Ok(content)) => Ok(content),
        Ok(Err((code, text))) => Err(sck_error(code, &text, "list the screens")),
        Err(_) => Err(PortError::Failed("ScreenCaptureKit did not list the screens in time".into())),
    }
}

/// The whole display without this process's windows, so the recording bar and border stay out of
/// the video. The window numbered `keep` (the pointer highlight) stays in.
fn content_filter(content: &SCShareableContent, display_id: u32, keep: Option<u32>) -> Result<Retained<SCContentFilter>, PortError> {
    // SAFETY: property reads on the live content object.
    let (displays, apps) = unsafe { (content.displays().to_vec(), content.applications().to_vec()) };
    // SAFETY: property reads on the displays above.
    let display = displays.iter().find(|display| unsafe { display.displayID() } == display_id);
    let Some(display) = display else {
        return Err(PortError::Failed("the display is not available to ScreenCaptureKit".into()));
    };
    let pid = i32::try_from(std::process::id()).ok();
    // SAFETY: property reads on the apps above.
    let own: Vec<Retained<SCRunningApplication>> = apps.into_iter().filter(|app| Some(unsafe { app.processID() }) == pid).collect();
    if own.is_empty() {
        tracing::warn!("this app is not in the shareable list; its windows will show in the recording");
    }
    let excluded_apps = NSArray::from_retained_slice(&own);
    // SAFETY: property reads on the live content object and its windows.
    let kept: Vec<Retained<SCWindow>> =
        unsafe { content.windows().to_vec() }.into_iter().filter(|window| Some(unsafe { window.windowID() }) == keep).collect();
    let kept_windows = NSArray::from_retained_slice(&kept);
    // SAFETY: valid display and arrays; `alloc` gives a fresh instance for the initialiser.
    Ok(unsafe {
        SCContentFilter::initWithDisplay_excludingApplications_exceptingWindows(
            SCContentFilter::alloc(),
            display,
            &excluded_apps,
            &kept_windows,
        )
    })
}

/// How many frames SCK may queue for us before it drops the oldest.
const QUEUE_DEPTH: isize = 3;

fn configuration(region: &Region, show_cursor: bool, fps: u32, system_audio: bool) -> Retained<SCStreamConfiguration> {
    let PtRect { x, y, w, h } = region.rect;
    // SAFETY: setters on a fresh configuration with plain values. The CMTime is a valid 1/fps
    // interval, and the color space is a constant CFString.
    unsafe {
        let config = SCStreamConfiguration::new();
        config.setWidth(region.pixels.0 as usize);
        config.setHeight(region.pixels.1 as usize);
        config.setSourceRect(CGRect::new(CGPoint::new(x, y), CGSize::new(w, h)));
        config.setMinimumFrameInterval(CMTime::new(1, i32::try_from(fps).unwrap_or(60)));
        config.setQueueDepth(QUEUE_DEPTH);
        config.setPixelFormat(kCVPixelFormatType_32BGRA);
        // sRGB so a wide-gamut display does not tint the video, which FFmpeg tags as BT.709.
        config.setColorSpaceName(kCGColorSpaceSRGB);
        config.setShowsCursor(show_cursor);
        if system_audio {
            config.setCapturesAudio(true);
            config.setExcludesCurrentProcessAudio(true);
            config.setSampleRate(RATE as isize);
            config.setChannelCount(CHANNELS as isize);
        }
        config
    }
}

/// Runs a callback body so that a panic cannot unwind into Objective-C.
fn guarded(what: &str, work: impl FnOnce()) {
    if catch_unwind(AssertUnwindSafe(work)).is_err() {
        tracing::error!("panic in the screen stream {what}");
    }
}

struct Ivars {
    shared: Arc<Shared>,
}

define_class!(
    // SAFETY: NSObject has no subclassing requirements, and the class has no Drop impl.
    #[unsafe(super(NSObject))]
    #[ivars = Ivars]
    struct StreamOutput;

    unsafe impl NSObjectProtocol for StreamOutput {}

    unsafe impl SCStreamOutput for StreamOutput {
        #[unsafe(method(stream:didOutputSampleBuffer:ofType:))]
        fn stream_did_output_sample_buffer(&self, _stream: &SCStream, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
            let shared = &self.ivars().shared;
            guarded("sample callback", || shared.on_sample(buffer, kind));
        }
    }

    unsafe impl SCStreamDelegate for StreamOutput {
        #[unsafe(method(stream:didStopWithError:))]
        fn stream_did_stop_with_error(&self, _stream: &SCStream, error: &NSError) {
            let shared = &self.ivars().shared;
            guarded("stop callback", || shared.on_stopped(error.code(), &error.localizedDescription().to_string()));
        }
    }
);

impl StreamOutput {
    fn new(shared: Arc<Shared>) -> Retained<Self> {
        let this = Self::alloc().set_ivars(Ivars { shared });
        // SAFETY: NSObject's `init` on a freshly allocated instance whose ivars are set.
        unsafe { msg_send![super(this), init] }
    }
}

impl Shared {
    fn on_sample(&self, buffer: &CMSampleBuffer, kind: SCStreamOutputType) {
        match kind {
            SCStreamOutputType::Screen => self.on_screen(buffer),
            SCStreamOutputType::Audio => self.on_audio(buffer),
            _ => {}
        }
    }

    /// The stream stopped by itself: the user chose "Stop sharing", the display went away, or
    /// macOS ended it. The user's own stop is an ordinary end.
    fn on_stopped(&self, code: isize, text: &str) {
        if code == SCStreamErrorCode::UserStopped.0 {
            self.finish(Ok(()));
        } else {
            tracing::warn!("the screen stream stopped: {text} (code {code})");
            self.finish(Err(format!("the screen stream stopped: {text}")));
        }
    }

    fn on_screen(&self, buffer: &CMSampleBuffer) {
        // Idle and blank frames carry no new pixels.
        if !frame_is_complete(buffer) {
            return;
        }
        // SAFETY: the sample buffer is valid for the callback; the image buffer comes back retained.
        let Some(image) = (unsafe { buffer.image_buffer() }) else { return };
        match copy_frame(&image, self.size) {
            Ok(frame) => self.push_frame(frame),
            Err(why) => Self::warn_once(&self.frame_warned, "a frame", why),
        }
    }

    fn on_audio(&self, buffer: &CMSampleBuffer) {
        if lock(&self.audio).end {
            return;
        }
        let mut pcm = Vec::new();
        match read_audio(buffer, &mut pcm) {
            Ok(()) => self.push_audio(&pcm),
            Err(why) => Self::warn_once(&self.audio_warned, "audio", why),
        }
    }
}

/// True when the sample buffer's `SCStreamFrameInfoStatus` attachment says `Complete`.
fn frame_is_complete(buffer: &CMSampleBuffer) -> bool {
    // SAFETY: the attachments array belongs to the buffer and is retained here. Its first element is
    // a CFDictionary, whose `Status` value is a CFNumber (both per the ScreenCaptureKit docs). The
    // key is SCK's own constant, so the lookup compares the same object.
    unsafe {
        let Some(attachments) = buffer.sample_attachments_array(false) else { return false };
        if attachments.count() < 1 {
            return false;
        }
        let dictionary = attachments.value_at_index(0).cast::<CFDictionary>();
        let key: *const NSString = SCStreamFrameInfoStatus;
        let Some(dictionary) = dictionary.as_ref() else { return false };
        let value = dictionary.value(key.cast::<c_void>()).cast::<CFNumber>();
        let Some(number) = value.as_ref() else { return false };
        let mut status = -1_i64;
        number.value(CFNumberType::SInt64Type, (&raw mut status).cast::<c_void>()) && status == SCFrameStatus::Complete.0 as i64
    }
}

/// A read-only lock on a pixel buffer's memory, released on drop.
struct PixelLock<'a>(&'a CVPixelBuffer);

impl<'a> PixelLock<'a> {
    fn new(buffer: &'a CVPixelBuffer) -> Option<Self> {
        // SAFETY: the buffer is alive; the read-only flag matches the unlock below.
        let status = unsafe { CVPixelBufferLockBaseAddress(buffer, CVPixelBufferLockFlags::ReadOnly) };
        if status == kCVReturnSuccess { Some(PixelLock(buffer)) } else { None }
    }
}

impl Drop for PixelLock<'_> {
    fn drop(&mut self) {
        // SAFETY: pairs with the successful read-only lock in `new`.
        unsafe { CVPixelBufferUnlockBaseAddress(self.0, CVPixelBufferLockFlags::ReadOnly) };
    }
}

/// Copies a BGRA pixel buffer of exactly `size` into a tight `Bitmap`, dropping the row padding.
fn copy_frame(image: &CVPixelBuffer, size: (u32, u32)) -> Result<Bitmap, &'static str> {
    if CVPixelBufferGetPixelFormatType(image) != kCVPixelFormatType_32BGRA {
        return Err("the frame is not BGRA");
    }
    let (width, height) = (size.0 as usize, size.1 as usize);
    if (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image)) != (width, height) {
        return Err("the frame has another size than the recording");
    }
    let _lock = PixelLock::new(image).ok_or("the frame memory could not be locked")?;
    let base = CVPixelBufferGetBaseAddress(image).cast::<u8>().cast_const();
    if base.is_null() {
        return Err("the frame has no memory");
    }
    let stride = CVPixelBufferGetBytesPerRow(image);
    let row_bytes = width * 4;
    // SAFETY: a locked single-plane BGRA buffer holds `height` rows of `stride` bytes at `base`,
    // of which the last needs only `row_bytes`. The slice ends before the lock is released.
    let src = unsafe { slice::from_raw_parts(base, stride * height.saturating_sub(1) + row_bytes) };
    let pixels = copy_rows(src, stride, row_bytes, height).ok_or("the frame memory is shorter than its size")?;
    Bitmap::new(size.0, size.1, PixelFormat::Bgra8, pixels).map_err(|_| "the frame does not match its size")
}

/// `kAudioFormatFlagIsFloat`.
const AUDIO_FLAG_FLOAT: u32 = 1;

/// CoreAudio's `AudioBuffer` and `AudioBufferList`, which `objc2-core-media` takes by pointer but
/// does not re-export.
#[repr(C)]
struct RawAudioBuffer {
    channels: u32,
    bytes: u32,
    data: *mut c_void,
}

#[repr(C)]
struct RawAudioBufferList {
    count: u32,
    buffers: [RawAudioBuffer; 1],
}

/// Room for the list header and 15 buffers, as 8-byte words so the list is aligned. SCK sends
/// one buffer per channel at most.
const LIST_WORDS: usize = 32;

/// Appends the samples of an SCK audio buffer as interleaved 16-bit stereo. SCK sends 32-bit
/// float, planar (one buffer per channel) or interleaved (one buffer, several channels).
fn read_audio(buffer: &CMSampleBuffer, out: &mut Vec<i16>) -> Result<(), &'static str> {
    // SAFETY: the format description belongs to the buffer and is retained here. The ASBD pointer
    // is valid while the description is.
    let is_float32 = unsafe {
        let Some(description) = buffer.format_description() else { return Err("the audio has no format") };
        let asbd = CMAudioFormatDescriptionGetStreamBasicDescription(&description);
        !asbd.is_null() && (*asbd).mFormatFlags & AUDIO_FLAG_FLOAT != 0 && (*asbd).mBitsPerChannel == 32
    };
    if !is_float32 {
        return Err("the audio is not 32-bit float");
    }

    let mut storage = [0_u64; LIST_WORDS];
    let mut block: *mut CMBlockBuffer = ptr::null_mut();
    // SAFETY: `storage` is writable, 8-byte aligned and as large as the size passed. CoreMedia
    // fills the list and returns a retained block buffer that owns the sample memory.
    let status = unsafe {
        buffer.audio_buffer_list_with_retained_block_buffer(
            ptr::null_mut(),
            storage.as_mut_ptr().cast(),
            size_of_val(&storage),
            None,
            None,
            kCMSampleBufferFlag_AudioBufferList_Assure16ByteAlignment,
            &raw mut block,
        )
    };
    // SAFETY: a non-null block buffer came back with a +1 retain count; this releases it after use.
    let _block = NonNull::new(block).map(|block| unsafe { CFRetained::from_raw(block) });
    if status != 0 {
        return Err("the audio buffer list could not be read");
    }

    let list = storage.as_ptr().cast::<RawAudioBufferList>();
    // SAFETY: CoreMedia wrote the list into `storage`.
    let count = unsafe { (*list).count } as usize;
    let header = offset_of!(RawAudioBufferList, buffers);
    if count == 0 || header + count * size_of::<RawAudioBuffer>() > size_of_val(&storage) {
        return Err("the audio buffer list has an unexpected size");
    }
    // SAFETY: the buffers start `header` bytes in and the check above keeps all of them inside `storage`.
    let first = unsafe { storage.as_ptr().cast::<u8>().add(header) }.cast::<RawAudioBuffer>();
    let mut channels = Vec::with_capacity(count);
    for index in 0..count {
        // SAFETY: `index < count`, which the size check covers.
        let raw = unsafe { &*first.add(index) };
        let data = raw.data.cast::<f32>().cast_const();
        if data.is_null() || !data.is_aligned() {
            return Err("the audio data is missing or misaligned");
        }
        // SAFETY: the buffer holds `bytes` bytes of samples at `data`, kept alive by `_block`.
        let samples = unsafe { slice::from_raw_parts(data, raw.bytes as usize / size_of::<f32>()) };
        let per_frame = (raw.channels as usize).max(1);
        channels.extend((0..per_frame).map(|offset| Channel { data: samples, stride: per_frame, offset }));
    }
    stereo_i16(&channels, out);
    Ok(())
}

/// The frame source of a recording. Stopping it ends the whole stream.
struct MacFrames {
    session: Arc<Session>,
    size: (u32, u32),
    fps: u32,
    last: Option<Bitmap>,
}

impl FrameSource for MacFrames {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn fps(&self) -> u32 {
        self.fps
    }

    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        let period = Duration::from_secs(1) / self.fps.max(1);
        self.session.shared.next_frame(&mut self.last, period, FIRST_FRAME_WAIT)
    }

    fn stop(&mut self) {
        self.session.stop();
    }
}

impl Drop for MacFrames {
    fn drop(&mut self) {
        self.session.stop();
    }
}

/// The system-audio source. Stopping it ends the sound only: `laqta-media` stops a PCM source it
/// cannot use while the picture goes on.
struct MacPcm {
    session: Arc<Session>,
}

impl PcmSource for MacPcm {
    fn format(&self) -> PcmFormat {
        PcmFormat { sample_rate: RATE, channels: CHANNELS as u16 }
    }

    fn read(&mut self, buf: &mut [i16]) -> Result<usize, PortError> {
        Ok(self.session.shared.read_pcm(buf))
    }

    fn stop(&mut self) {
        self.session.shared.end_audio();
    }
}

impl Drop for MacPcm {
    fn drop(&mut self) {
        self.session.shared.end_audio();
    }
}

/// The devices of one kind that `AVCaptureDevice` discovery finds.
fn discover(kinds: &[&NSString], media: Option<&NSString>) -> Vec<Retained<AVCaptureDevice>> {
    let kinds = NSArray::from_slice(kinds);
    // SAFETY: plain Objective-C calls with valid arguments; discovery needs no permission.
    unsafe {
        AVCaptureDeviceDiscoverySession::discoverySessionWithDeviceTypes_mediaType_position(
            &kinds,
            media,
            AVCaptureDevicePosition::Unspecified,
        )
        .devices()
        .to_vec()
    }
}

/// System audio first, then the microphones. A microphone's id is the unique ID that FFmpeg's
/// `avfoundation` also matches.
fn list_audio_devices() -> Vec<AudioDevice> {
    let mut devices = vec![AudioDevice { id: SYSTEM_AUDIO_ID.into(), name: SYSTEM_AUDIO_NAME.into(), loopback: true }];
    // SAFETY: the extern statics are constant NSStrings.
    let (kinds, media) = unsafe { ([AVCaptureDeviceTypeMicrophone, AVCaptureDeviceTypeExternal], AVMediaTypeAudio) };
    for device in discover(&kinds, media) {
        // SAFETY: property reads on a live device.
        let (id, name) = unsafe { (device.uniqueID().to_string(), device.localizedName().to_string()) };
        if !devices.iter().any(|known| known.id == id) {
            devices.push(AudioDevice { id, name, loopback: false });
        }
    }
    devices
}

/// The camera names, the built-in camera first.
fn list_cameras() -> Vec<String> {
    // SAFETY: the extern statics are constant NSStrings.
    let (kinds, media) = unsafe {
        ([AVCaptureDeviceTypeBuiltInWideAngleCamera, AVCaptureDeviceTypeExternal, AVCaptureDeviceTypeContinuityCamera], AVMediaTypeVideo)
    };
    // SAFETY: property reads on live devices.
    discover(&kinds, media).iter().map(|device| unsafe { device.localizedName() }.to_string()).collect()
}

// <core_tests>
#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn frame(first_byte: u8) -> Bitmap {
        Bitmap::new(1, 1, PixelFormat::Bgra8, Arc::from(vec![first_byte, 0, 0, 255])).unwrap()
    }

    fn first_byte(frame: &Bitmap) -> u8 {
        frame.pixels()[0]
    }

    const SHORT: Duration = Duration::from_millis(20);

    #[test]
    fn a_full_queue_drops_its_oldest_frame() {
        let shared = Shared::new((1, 1));
        for byte in 1..=3 {
            shared.push_frame(frame(byte));
        }
        let mut last = None;
        let one = shared.next_frame(&mut last, SHORT, SHORT).unwrap().unwrap();
        let two = shared.next_frame(&mut last, SHORT, SHORT).unwrap().unwrap();
        assert_eq!((first_byte(&one), first_byte(&two)), (2, 3));
    }

    #[test]
    fn a_still_screen_repeats_the_last_frame() {
        let shared = Shared::new((1, 1));
        shared.push_frame(frame(7));
        let mut last = None;
        shared.next_frame(&mut last, SHORT, SHORT).unwrap();
        let again = shared.next_frame(&mut last, SHORT, SHORT).unwrap().unwrap();
        assert_eq!(first_byte(&again), 7);
    }

    #[test]
    fn no_first_frame_is_an_error() {
        let shared = Shared::new((1, 1));
        assert!(matches!(shared.next_frame(&mut None, SHORT, SHORT), Err(PortError::Failed(_))));
    }

    #[test]
    fn the_end_comes_after_the_queued_frames() {
        let shared = Shared::new((1, 1));
        shared.push_frame(frame(1));
        shared.finish(Ok(()));
        shared.push_frame(frame(2));
        let mut last = None;
        assert!(shared.next_frame(&mut last, SHORT, SHORT).unwrap().is_some());
        assert!(shared.next_frame(&mut last, SHORT, SHORT).unwrap().is_none());
    }

    #[test]
    fn a_failed_stream_is_an_error_once_the_queue_is_empty() {
        let shared = Shared::new((1, 1));
        shared.finish(Err("gone".into()));
        assert!(matches!(shared.next_frame(&mut None, SHORT, SHORT), Err(PortError::Failed(reason)) if reason == "gone"));
    }

    #[test]
    fn row_padding_is_dropped() {
        // Two rows of two pixels (8 bytes) in a 12-byte stride, the last row without padding.
        let src: Vec<u8> = (0..20).collect();
        let tight = copy_rows(&src, 12, 8, 2).unwrap();
        assert_eq!(&*tight, &[0, 1, 2, 3, 4, 5, 6, 7, 12, 13, 14, 15, 16, 17, 18, 19]);
        assert!(copy_rows(&src[..19], 12, 8, 2).is_none());
        assert!(copy_rows(&src, 4, 8, 2).is_none());
    }

    #[test]
    fn planar_and_interleaved_audio_give_the_same_pcm() {
        let left = [0.0_f32, 0.5, -0.5];
        let right = [1.0_f32, -1.0, 0.25];
        let planar = [Channel { data: &left, stride: 1, offset: 0 }, Channel { data: &right, stride: 1, offset: 0 }];
        let packed_data = [0.0_f32, 1.0, 0.5, -1.0, -0.5, 0.25];
        let packed = [Channel { data: &packed_data, stride: 2, offset: 0 }, Channel { data: &packed_data, stride: 2, offset: 1 }];
        let (mut a, mut b) = (Vec::new(), Vec::new());
        stereo_i16(&planar, &mut a);
        stereo_i16(&packed, &mut b);
        assert_eq!(a, b);
        assert_eq!(a, [0, 32767, 16384, -32767, -16384, 8192]);
    }

    #[test]
    fn a_mono_source_is_doubled_and_loud_samples_are_clamped() {
        let mono = [2.0_f32, -2.0];
        let mut out = Vec::new();
        stereo_i16(&[Channel { data: &mono, stride: 1, offset: 0 }], &mut out);
        assert_eq!(out, [32767, 32767, -32767, -32767]);
    }

    /// Starts the reader's clock, as its first `read` would, without dropping what is buffered.
    fn started(shared: &Shared) {
        let now = Instant::now();
        lock(&shared.audio).clock = Some(Clock { start: now, frames: 0, last_read: now });
    }

    #[test]
    fn pcm_is_read_in_order_and_ends_with_the_stream() {
        let shared = Shared::new((1, 1));
        started(&shared);
        shared.push_audio(&[1, 2, 3, 4]);
        let mut buf = [0_i16; 3];
        assert_eq!(shared.read_pcm(&mut buf), 3);
        assert_eq!(buf, [1, 2, 3]);
        shared.finish(Ok(()));
        assert_eq!(shared.read_pcm(&mut buf), 1);
        assert_eq!(shared.read_pcm(&mut buf), 0);
    }

    #[test]
    fn the_audio_buffer_keeps_one_second() {
        let shared = Shared::new((1, 1));
        shared.push_audio(&vec![1; RING_SAMPLES]);
        shared.push_audio(&[2, 2]);
        let audio = lock(&shared.audio);
        assert_eq!(audio.samples.len(), RING_SAMPLES);
        assert_eq!(audio.samples.back(), Some(&2));
    }

    #[test]
    fn silence_fills_a_stream_that_sends_nothing() {
        let shared = Shared::new((1, 1));
        let mut buf = [1_i16; 64];
        // The first read starts the clock; once 100 ms of nothing have passed, silence is due.
        assert_eq!(shared.read_pcm(&mut buf), 64);
        assert!(buf.iter().all(|&sample| sample == 0));
    }

    #[test]
    fn a_reader_that_was_away_loses_the_stale_backlog() {
        let shared = Shared::new((1, 1));
        started(&shared);
        shared.push_audio(&[7, 7]);
        std::thread::sleep(RESUME_GAP + Duration::from_millis(50));
        let (count, got) = std::thread::scope(|scope| {
            let reader = scope.spawn(|| {
                let mut buf = [0_i16; 2];
                (shared.read_pcm(&mut buf), buf)
            });
            // Fresh sound keeps coming; the 7s were buffered before the long absence and are dropped.
            while !reader.is_finished() {
                shared.push_audio(&[5, 5]);
                std::thread::sleep(Duration::from_millis(10));
            }
            reader.join().unwrap()
        });
        assert_eq!((count, got), (2, [5, 5]));
    }

    #[test]
    fn the_area_is_cut_to_the_display_and_to_even_pixels() {
        let retina = PtRect { x: 0.0, y: 0.0, w: 1512.0, h: 982.0 };
        let region = local_region(retina, 2.0, PtRect { x: 100.0, y: 50.0, w: 300.0, h: 200.0 }).unwrap();
        assert_eq!(region, Region { rect: PtRect { x: 100.0, y: 50.0, w: 300.0, h: 200.0 }, pixels: (600, 400) });

        // On a second display the rect is relative to that display, and an odd size loses a pixel.
        let external = PtRect { x: 1512.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let region = local_region(external, 1.0, PtRect { x: 1600.0, y: 100.0, w: 801.0, h: 600.0 }).unwrap();
        assert_eq!(region.rect, PtRect { x: 88.0, y: 100.0, w: 800.0, h: 600.0 });
        assert_eq!(region.pixels, (800, 600));

        // Past the right edge only the part on the display is kept.
        let region = local_region(retina, 2.0, PtRect { x: 1500.0, y: 0.0, w: 100.0, h: 100.0 }).unwrap();
        assert_eq!(region.pixels, (24, 200));

        assert!(local_region(retina, 1.0, PtRect { x: 10.0, y: 10.0, w: 1.0, h: 100.0 }).is_none());
    }
}
// </core_tests>

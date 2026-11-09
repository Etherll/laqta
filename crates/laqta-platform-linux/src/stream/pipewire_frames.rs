//! Frames from a ScreenCast node through PipeWire (ADR 016, M4 D2). A thread of its own runs the
//! PipeWire loop and copies each frame, cropped to the recording area and converted to BGRA, into a
//! queue of two. `next_frame` takes the newest frame from that queue and drops the older ones. It does
//! not pace: frames come as the compositor sends them (at most the requested rate) and the consumer
//! keeps time.
//!
//! PipeWire calls back through C trampolines, where a panic aborts the process. The callbacks
//! therefore avoid indexing, `expect` and `unwrap`, and drop anything that does not fit.

use std::collections::VecDeque;
use std::os::fd::OwnedFd;
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ashpd::desktop::Session;
use ashpd::desktop::screencast::Screencast;
use laqta_domain::{Bitmap, PhysRect, PixelFormat};
use laqta_ports::{FrameSource, PortError};
use pipewire::context::ContextRc;
use pipewire::main_loop::{MainLoopRc, MainLoopWeak};
use pipewire::properties::properties;
use pipewire::spa::buffer::ChunkFlags;
use pipewire::spa::param::ParamType;
use pipewire::spa::param::format::{FormatProperties, MediaSubtype, MediaType};
use pipewire::spa::param::format_utils;
use pipewire::spa::param::video::{VideoFormat, VideoInfoRaw};
use pipewire::spa::pod::serialize::PodSerializer;
use pipewire::spa::pod::{Pod, Value};
use pipewire::spa::utils::{Direction, SpaTypes};
use pipewire::stream::{StreamFlags, StreamRc, StreamState};

use super::screencast::Cast;

/// Frames waiting between the PipeWire thread and `next_frame`. When full, the oldest goes.
const QUEUE_CAPACITY: usize = 2;
/// How long `next_frame` waits for a frame before the recording fails.
const FRAME_TIMEOUT: Duration = Duration::from_secs(2);
/// How long `start` waits for the PipeWire thread to connect to the node.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The screen frames of one ScreenCast stream, cropped to the recording area. Dropping it, or
/// calling `stop`, ends the PipeWire loop and closes the portal session.
pub struct PipeWireFrames {
    shared: Arc<Shared>,
    width: u32,
    height: u32,
    fps: u32,
    quit: Option<pipewire::channel::Sender<()>>,
    thread: Option<JoinHandle<()>>,
    /// The last frame, repeated while the compositor sends nothing new.
    last: Option<Arc<[u8]>>,
    session: Option<Session<Screencast>>,
}

/// Why `PipeWireFrames::start` failed.
#[derive(Debug)]
pub enum StartError {
    /// The recording area is not on the screen the portal shared. The message tells the user.
    AreaOffStream(String),
    Other(PortError),
}

impl From<StartError> for PortError {
    fn from(e: StartError) -> Self {
        match e {
            StartError::AreaOffStream(message) => PortError::Failed(message),
            StartError::Other(e) => e,
        }
    }
}

impl PipeWireFrames {
    /// Connects to the node of `cast` and starts pumping frames. `area` has even dimensions and is
    /// in physical pixels. The stream's position (zero when the portal gives none) is subtracted from
    /// it to find the crop. When the portal did give a position, `start` waits for the stream's size
    /// and fails with `StartError::AreaOffStream` if the crop does not lie inside the stream.
    pub fn start(cast: Cast, area: PhysRect, fps: u32) -> Result<Self, StartError> {
        let (ox, oy) = cast.position.unwrap_or((0, 0));
        let crop =
            Crop { x: i64::from(area.x) - i64::from(ox), y: i64::from(area.y) - i64::from(oy), width: area.width, height: area.height };
        let check_area = cast.position.is_some();
        let placement = Placement { area, position: cast.position, logical_width: cast.size.map(|(w, _)| w) };
        let Cast { session, node_id, fd, .. } = cast;
        let shared = Arc::new(Shared::default());
        let (quit_tx, quit_rx) = pipewire::channel::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let mut frames = PipeWireFrames {
            shared: Arc::clone(&shared),
            width: area.width,
            height: area.height,
            fps,
            quit: Some(quit_tx),
            thread: None,
            session: Some(session.into_inner()),
            last: None,
        };
        let job = Job { crop, placement, fps, check_area };
        let thread = thread::Builder::new()
            .name("laqta-pipewire".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run(fd, node_id, job, shared, quit_rx, ready_tx)
            })
            .map_err(|e| StartError::Other(PortError::Failed(format!("PipeWire thread: {e}"))))?;
        frames.thread = Some(thread);
        match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(Ok(())) => Ok(frames),
            Ok(Err(e)) => Err(e),
            Err(_) => Err(StartError::Other(PortError::Failed("PipeWire did not connect to the screen stream in time".into()))),
        }
    }

    /// Ends the PipeWire loop, waits for its thread and closes the portal session. A second call does
    /// nothing.
    fn shutdown(&mut self) {
        if let Some(quit) = self.quit.take() {
            let _ = quit.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        if let Some(session) = self.session.take() {
            let _ = async_io::block_on(session.close());
        }
        self.shared.finish(Ok(()));
    }
}

impl Drop for PipeWireFrames {
    fn drop(&mut self) {
        self.shutdown();
    }
}

impl FrameSource for PipeWireFrames {
    fn size(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn fps(&self) -> u32 {
        self.fps
    }

    /// The newest frame, with older queued ones dropped. Compositors send frames only on damage, so
    /// on a still screen nothing arrives: after one frame period without news the last frame repeats
    /// (the first frame gets `FRAME_TIMEOUT`, then the recording fails). A stream error is an `Err`.
    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        let wait = match &self.last {
            Some(_) => Duration::from_secs(1) / self.fps.max(1),
            None => FRAME_TIMEOUT,
        };
        let pixels = match self.shared.newest(wait)? {
            Pop::Frame(pixels) => pixels,
            Pop::Ended => return Ok(None),
            Pop::TimedOut => match &self.last {
                Some(last) => Arc::clone(last),
                None => return Err(PortError::Failed(format!("no frame from the screen stream within {} s", FRAME_TIMEOUT.as_secs()))),
            },
        };
        self.last = Some(Arc::clone(&pixels));
        Bitmap::new(self.width, self.height, PixelFormat::Bgra8, pixels)
            .map(Some)
            .map_err(|e| PortError::Failed(format!("screen frame: {e}")))
    }

    fn stop(&mut self) {
        self.shutdown();
    }
}

/// The frame queue shared with the PipeWire thread, and how the stream ended once it has.
#[derive(Default)]
struct Shared {
    queue: Mutex<Queue>,
    ready: Condvar,
}

#[derive(Default)]
struct Queue {
    frames: VecDeque<Arc<[u8]>>,
    /// `Ok` once the stream ended cleanly, `Err` with the reason when it failed.
    end: Option<Result<(), String>>,
}

impl Shared {
    fn push(&self, frame: Arc<[u8]>) {
        let Ok(mut queue) = self.queue.lock() else { return };
        if queue.frames.len() >= QUEUE_CAPACITY {
            queue.frames.pop_front();
        }
        queue.frames.push_back(frame);
        drop(queue);
        self.ready.notify_one();
    }

    /// Records how the stream ended. The first call wins, and queued frames are still delivered.
    fn finish(&self, end: Result<(), String>) {
        if let Ok(mut queue) = self.queue.lock()
            && queue.end.is_none()
        {
            queue.end = Some(end);
        }
        self.ready.notify_all();
    }

    /// The newest frame, dropping the older ones. Waits up to `timeout` for one. Queued frames come
    /// before the end of the stream, whether it ended cleanly or with an error.
    fn newest(&self, timeout: Duration) -> Result<Pop, PortError> {
        let deadline = Instant::now() + timeout;
        let mut queue = self.queue.lock().map_err(|_| poisoned())?;
        loop {
            if let Some(frame) = queue.frames.pop_back() {
                queue.frames.clear();
                return Ok(Pop::Frame(frame));
            }
            match &queue.end {
                Some(Ok(())) => return Ok(Pop::Ended),
                Some(Err(message)) => return Err(PortError::Failed(message.clone())),
                None => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Ok(Pop::TimedOut);
            }
            queue = self.ready.wait_timeout(queue, left).map_err(|_| poisoned())?.0;
        }
    }
}

/// What `Shared::newest` found.
enum Pop {
    Frame(Arc<[u8]>),
    /// The stream ended cleanly and nothing is queued.
    Ended,
    /// Nothing arrived within the wait. This is not a stream error.
    TimedOut,
}

fn poisoned() -> PortError {
    PortError::Failed("screen frame queue is poisoned".into())
}

/// What the PipeWire thread needs besides the socket and the node.
struct Job {
    crop: Crop,
    placement: Placement,
    fps: u32,
    /// Whether to fail when the crop is not inside the stream (the portal gave a position).
    check_area: bool,
}

/// Where the recording area sits on the stream, to place the crop once the stream's pixel size is
/// known. The portal reports the stream's position and size in the compositor's logical space,
/// while the area and the buffers are in pixels.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Placement {
    pub area: PhysRect,
    pub position: Option<(i32, i32)>,
    pub logical_width: Option<i32>,
}

impl Placement {
    /// The crop for a stream `pixel_width` wide: the area minus the stream's origin in pixels.
    pub(crate) fn crop(&self, pixel_width: u32) -> Crop {
        let (ox, oy) = self.position.unwrap_or((0, 0));
        let scale = match self.logical_width {
            Some(w) if w > 0 && pixel_width > 0 => f64::from(pixel_width) / f64::from(w),
            _ => 1.0,
        };
        let origin = |v: i32| (f64::from(v) * scale).round() as i64;
        Crop {
            x: i64::from(self.area.x) - origin(ox),
            y: i64::from(self.area.y) - origin(oy),
            width: self.area.width,
            height: self.area.height,
        }
    }
}

/// The rectangle to keep, in source pixels. `x` and `y` may lie outside the frame, and the pixels
/// that fall outside it come out black.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Crop {
    pub x: i64,
    pub y: i64,
    pub width: u32,
    pub height: u32,
}

/// The byte order of a negotiated 4-byte format. The alpha or padding byte is ignored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Channels {
    /// Blue, green, red (BGRx and BGRA).
    Bgr,
    /// Red, green, blue (RGBx).
    Rgb,
}

/// A frame's pixels: `width` by `height`, with `stride` bytes per row.
pub(crate) struct Source<'a> {
    pub bytes: &'a [u8],
    pub stride: usize,
    pub width: u32,
    pub height: u32,
    pub order: Channels,
}

/// Copies `crop` out of `src` as BGRA with alpha 255. Pixels outside the frame are black, and so is
/// a row that the buffer is too short to hold. Each row of a BGR source is one slice copy.
pub(crate) fn crop_bgra(src: &Source<'_>, crop: Crop) -> Arc<[u8]> {
    let (width, height) = (crop.width as usize, crop.height as usize);
    // Collecting from an exact-size iterator allocates the `Arc` once, so the pixels are never copied again.
    let mut out: Arc<[u8]> = std::iter::repeat_n(0u8, width * height * 4).collect();
    let Some(pixels) = Arc::get_mut(&mut out) else { return out };
    let columns = visible(crop.x, width, src.width);
    let sx = usize::try_from(crop.x + columns.start as i64).unwrap_or(0);
    if !columns.is_empty() && width > 0 {
        for (row, line) in pixels.chunks_exact_mut(width * 4).enumerate() {
            let Some(sy) = visible_index(crop.y + row as i64, src.height) else { continue };
            let from = src.bytes.get(sy * src.stride..).and_then(|from| from.get(sx * 4..(sx + columns.len()) * 4));
            let to = line.get_mut(columns.start * 4..columns.end * 4);
            let (Some(from), Some(to)) = (from, to) else { continue };
            match src.order {
                Channels::Bgr => to.copy_from_slice(from),
                Channels::Rgb => {
                    for (dst, px) in to.as_chunks_mut::<4>().0.iter_mut().zip(from.as_chunks::<4>().0) {
                        *dst = [px[2], px[1], px[0], 0];
                    }
                }
            }
        }
    }
    for px in pixels.as_chunks_mut::<4>().0 {
        px[3] = 255;
    }
    out
}

/// The destination columns (`0..len`) whose source column `origin + column` lies in `0..limit`.
fn visible(origin: i64, len: usize, limit: u32) -> std::ops::Range<usize> {
    let len = i64::try_from(len).unwrap_or(i64::MAX);
    let start = (-origin).clamp(0, len);
    let end = (i64::from(limit) - origin).clamp(0, len);
    if start < end { start as usize..end as usize } else { 0..0 }
}

/// `y` as a row index when it lies in `0..limit`.
fn visible_index(y: i64, limit: u32) -> Option<usize> {
    if (0..i64::from(limit)).contains(&y) { usize::try_from(y).ok() } else { None }
}

/// The format a stream negotiated, as the crop needs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Negotiated {
    order: Channels,
    width: u32,
    height: u32,
}

/// What a buffer's chunk says about its bytes.
#[derive(Clone, Copy, Debug)]
struct BufferInfo {
    offset: usize,
    size: usize,
    stride: i32,
    corrupted: bool,
}

/// The crop of one buffer, or `None` when the buffer should not become a frame: the producer flagged
/// it corrupted, it does not fit the negotiated format, or it holds fewer bytes than the frame needs.
fn crop_buffer(format: Negotiated, crop: Crop, mapped: &[u8], buffer: BufferInfo) -> Option<Arc<[u8]>> {
    if buffer.corrupted {
        return None;
    }
    let row_bytes = (format.width as usize).checked_mul(4)?;
    if row_bytes == 0 || format.height == 0 {
        return None;
    }
    let stride = match usize::try_from(buffer.stride) {
        Ok(stride) if stride > 0 => stride,
        _ => row_bytes,
    };
    if stride < row_bytes {
        return None;
    }
    // Every row but the last is a full stride; the last needs only its pixels.
    let needed = stride.checked_mul(format.height as usize - 1)?.checked_add(row_bytes)?;
    if buffer.size < needed {
        return None;
    }
    let bytes = mapped.get(buffer.offset..buffer.offset.checked_add(buffer.size)?)?;
    let src = Source { bytes, stride, width: format.width, height: format.height, order: format.order };
    Some(crop_bgra(&src, crop))
}

/// Whether `crop` lies inside a stream of `stream` pixels, as `Err` with the message for the user
/// when it does not. The crop is in the stream's own pixel space.
fn check_area(crop: Crop, stream: (u32, u32)) -> Result<(), String> {
    let right = crop.x + i64::from(crop.width);
    let bottom = crop.y + i64::from(crop.height);
    if crop.x >= 0 && crop.y >= 0 && right <= i64::from(stream.0) && bottom <= i64::from(stream.1) {
        return Ok(());
    }
    Err(format!(
        "the area is not on the shared screen ({}x{}); record an area on that screen, or pick the screen the area is on when asked what to share",
        stream.0, stream.1
    ))
}

/// The data the stream listener carries on the PipeWire thread.
struct Frames {
    shared: Arc<Shared>,
    mainloop: MainLoopWeak,
    crop: Crop,
    placement: Placement,
    check_area: bool,
    format: Option<Negotiated>,
    /// Answers `start` once the stream has a usable format, or has failed before it got one.
    ready: Option<mpsc::Sender<Result<(), StartError>>>,
}

impl Frames {
    fn quit(&self) {
        if let Some(mainloop) = self.mainloop.upgrade() {
            mainloop.quit();
        }
    }

    /// Tells `start` how the stream came up. Only the first answer counts.
    fn settle(&mut self, outcome: Result<(), StartError>) {
        if let Some(ready) = self.ready.take() {
            let _ = ready.send(outcome);
        }
    }

    fn end(&mut self, end: Result<(), String>) {
        let outcome = match &end {
            Ok(()) => Err(StartError::Other(PortError::Failed("the screen stream closed before it started".into()))),
            Err(message) => Err(StartError::Other(PortError::Failed(message.clone()))),
        };
        self.settle(outcome);
        self.shared.finish(end);
        self.quit();
    }

    fn negotiate(&mut self, id: u32, param: Option<&Pod>) {
        let Some(param) = param else { return };
        if id != ParamType::Format.as_raw() {
            return;
        }
        let Ok((media_type, media_subtype)) = format_utils::parse_format(param) else { return };
        if media_type != MediaType::Video || media_subtype != MediaSubtype::Raw {
            return;
        }
        let mut info = VideoInfoRaw::default();
        if info.parse(param).is_err() {
            return;
        }
        let format = info.format();
        let order = if format == VideoFormat::BGRx || format == VideoFormat::BGRA {
            Channels::Bgr
        } else if format == VideoFormat::RGBx {
            Channels::Rgb
        } else {
            self.end(Err("PipeWire negotiated a pixel format that Laqta cannot read".into()));
            return;
        };
        let size = info.size();
        self.format = Some(Negotiated { order, width: size.width, height: size.height });
        self.crop = self.placement.crop(size.width);
        // The first format is the stream's size. Later ones (a mode change) only update the crop.
        if self.ready.is_some() {
            let area = if self.check_area { check_area(self.crop, (size.width, size.height)) } else { Ok(()) };
            match area {
                Ok(()) => self.settle(Ok(())),
                Err(message) => {
                    self.settle(Err(StartError::AreaOffStream(message.clone())));
                    self.shared.finish(Err(message));
                    self.quit();
                }
            }
        }
    }

    /// The crop of one buffer, or `None` when the buffer does not become a frame.
    fn frame(&self, mapped: &[u8], buffer: BufferInfo) -> Option<Arc<[u8]>> {
        crop_buffer(self.format?, self.crop, mapped, buffer)
    }
}

/// The PipeWire thread: connects to the node, runs the loop until quit or until the stream ends.
/// `ready` reports whether the stream came up: a failure to connect right away, otherwise the stream's
/// first format (see `Frames::negotiate`). Everything PipeWire is created and dropped here, since
/// its objects are not `Send`.
fn run(
    fd: OwnedFd,
    node_id: u32,
    job: Job,
    shared: Arc<Shared>,
    quit: pipewire::channel::Receiver<()>,
    ready: mpsc::Sender<Result<(), StartError>>,
) {
    pipewire::init();
    let mainloop = match MainLoopRc::new(None) {
        Ok(mainloop) => mainloop,
        Err(e) => return fail(&ready, format!("PipeWire main loop: {e}")),
    };
    let context = match ContextRc::new(&mainloop, None) {
        Ok(context) => context,
        Err(e) => return fail(&ready, format!("PipeWire context: {e}")),
    };
    let core = match context.connect_fd_rc(fd, None) {
        Ok(core) => core,
        Err(e) => return fail(&ready, format!("PipeWire connection: {e}")),
    };
    let stream = match StreamRc::new(
        core,
        "laqta-screencast",
        properties! {
            *pipewire::keys::MEDIA_TYPE => "Video",
            *pipewire::keys::MEDIA_CATEGORY => "Capture",
            *pipewire::keys::MEDIA_ROLE => "Screen",
        },
    ) {
        Ok(stream) => stream,
        Err(e) => return fail(&ready, format!("PipeWire stream: {e}")),
    };
    let _quit_receiver = quit.attach(mainloop.loop_(), {
        let weak = mainloop.downgrade();
        move |()| {
            if let Some(mainloop) = weak.upgrade() {
                mainloop.quit();
            }
        }
    });
    let _listener = match stream
        .add_local_listener_with_user_data(Frames {
            shared: Arc::clone(&shared),
            mainloop: mainloop.downgrade(),
            crop: job.crop,
            placement: job.placement,
            check_area: job.check_area,
            format: None,
            ready: Some(ready.clone()),
        })
        .state_changed(|_, frames, _old, new| match new {
            StreamState::Error(message) => frames.end(Err(message)),
            StreamState::Unconnected => frames.end(Ok(())),
            _ => {}
        })
        .param_changed(|_, frames, id, param| frames.negotiate(id, param))
        .process(|stream, frames| {
            let Some(mut buffer) = stream.dequeue_buffer() else { return };
            let Some(data) = buffer.datas_mut().first_mut() else { return };
            // `chunk()` asserts that the pointer is set, and a panic here would abort the process.
            if data.as_raw().chunk.is_null() {
                return;
            }
            let buffer = {
                let chunk = data.chunk();
                BufferInfo {
                    offset: chunk.offset() as usize,
                    size: chunk.size() as usize,
                    stride: chunk.stride(),
                    corrupted: chunk.flags().contains(ChunkFlags::CORRUPTED),
                }
            };
            let Some(mapped) = data.data() else { return };
            if let Some(frame) = frames.frame(mapped, buffer) {
                frames.shared.push(frame);
            }
        })
        .register()
    {
        Ok(listener) => listener,
        Err(e) => return fail(&ready, format!("PipeWire listener: {e}")),
    };
    let format = match format_pod(job.fps) {
        Ok(bytes) => bytes,
        Err(e) => return fail(&ready, format!("PipeWire format: {e}")),
    };
    let Some(format) = Pod::from_bytes(&format) else {
        return fail(&ready, "PipeWire format pod is malformed".into());
    };
    let mut params = [format];
    if let Err(e) = stream.connect(Direction::Input, Some(node_id), StreamFlags::AUTOCONNECT | StreamFlags::MAP_BUFFERS, &mut params) {
        return fail(&ready, format!("PipeWire connect: {e}"));
    }
    mainloop.run();
    shared.finish(Ok(()));
}

fn fail(ready: &mpsc::Sender<Result<(), StartError>>, message: String) {
    let _ = ready.send(Err(StartError::Other(PortError::Failed(message))));
}

/// The `EnumFormat` pod the stream asks for: raw video in BGRx, BGRA or RGBx at any size and up to
/// the requested rate (as `framerate` and `maxFramerate`). It names no modifier, so the producer hands
/// over shared-memory buffers.
fn format_pod(fps: u32) -> Result<Vec<u8>, String> {
    let fps = fps.max(1);
    let object = pipewire::spa::pod::object!(
        SpaTypes::ObjectParamFormat,
        ParamType::EnumFormat,
        pipewire::spa::pod::property!(FormatProperties::MediaType, Id, MediaType::Video),
        pipewire::spa::pod::property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        pipewire::spa::pod::property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx
        ),
        pipewire::spa::pod::property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            pipewire::spa::utils::Rectangle { width: 1920, height: 1080 },
            pipewire::spa::utils::Rectangle { width: 1, height: 1 },
            pipewire::spa::utils::Rectangle { width: 16384, height: 16384 }
        ),
        pipewire::spa::pod::property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            pipewire::spa::utils::Fraction { num: fps, denom: 1 },
            pipewire::spa::utils::Fraction { num: 0, denom: 1 },
            pipewire::spa::utils::Fraction { num: 1000, denom: 1 }
        ),
        // Asks the compositor not to send faster than the recording rate, which saves the crop work
        // on frames the consumer would drop. A range, as other screen consumers send it, so that a
        // producer offering a higher or lower ceiling still agrees on a format.
        pipewire::spa::pod::property!(
            FormatProperties::VideoMaxFramerate,
            Choice,
            Range,
            Fraction,
            pipewire::spa::utils::Fraction { num: fps, denom: 1 },
            pipewire::spa::utils::Fraction { num: 0, denom: 1 },
            pipewire::spa::utils::Fraction { num: fps, denom: 1 }
        ),
    );
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use pipewire::spa::pod::deserialize::PodDeserializer;
    use pipewire::spa::pod::{ChoiceValue, Property};
    use pipewire::spa::utils::{Choice, ChoiceEnum, Fraction};

    use super::*;

    fn source(bytes: &[u8], stride: usize, width: u32, height: u32, order: Channels) -> Source<'_> {
        Source { bytes, stride, width, height, order }
    }

    #[test]
    fn the_crop_scales_the_logical_stream_origin_to_pixels() {
        // A 125% monitor right of a 2048-wide one: logical origin 2048, 2560 pixels into the desktop.
        let area = PhysRect { x: 2660, y: 50, width: 640, height: 400 };
        let placement = Placement { area, position: Some((2048, 0)), logical_width: Some(2048) };
        assert_eq!(placement.crop(2560), Crop { x: 100, y: 50, width: 640, height: 400 });
        let unscaled = Placement { area, position: Some((2048, 0)), logical_width: None };
        assert_eq!(unscaled.crop(2560).x, 612);
    }

    #[test]
    fn copies_a_padded_frame_as_bgra() {
        // 2x2 BGRx with 4 bytes of padding per row. The padding and the x byte are not copied.
        let bytes = [1, 2, 3, 9, 4, 5, 6, 9, 0xEE, 0xEE, 0xEE, 0xEE, 7, 8, 9, 9, 10, 11, 12, 9, 0xEE, 0xEE, 0xEE, 0xEE];
        let crop = Crop { x: 0, y: 0, width: 2, height: 2 };
        let out = crop_bgra(&source(&bytes, 12, 2, 2, Channels::Bgr), crop);
        assert_eq!(&*out, [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);
    }

    #[test]
    fn rgbx_swaps_red_and_blue() {
        let crop = Crop { x: 0, y: 0, width: 1, height: 1 };
        let out = crop_bgra(&source(&[1, 2, 3, 0], 4, 1, 1, Channels::Rgb), crop);
        assert_eq!(&*out, [3, 2, 1, 255]);
    }

    #[test]
    fn crops_an_offset_window() {
        // 3x2 frame: pixel (c, r) holds [c, r, 0, 0].
        let bytes: Vec<u8> = (0..2u8).flat_map(|r| (0..3u8).flat_map(move |c| [c, r, 0, 0])).collect();
        let crop = Crop { x: 1, y: 1, width: 2, height: 1 };
        let out = crop_bgra(&source(&bytes, 12, 3, 2, Channels::Bgr), crop);
        assert_eq!(&*out, [1, 1, 0, 255, 2, 1, 0, 255]);
    }

    #[test]
    fn crops_an_offset_window_of_an_rgb_frame() {
        let bytes: Vec<u8> = (0..2u8).flat_map(|r| (0..3u8).flat_map(move |c| [c, r, 7, 0])).collect();
        let crop = Crop { x: 1, y: 1, width: 2, height: 1 };
        let out = crop_bgra(&source(&bytes, 12, 3, 2, Channels::Rgb), crop);
        assert_eq!(&*out, [7, 1, 1, 255, 7, 1, 2, 255]);
    }

    #[test]
    fn pixels_outside_the_frame_are_black() {
        // Crop starts one column left of a 1x1 frame and is two columns wide: column 0 is black.
        let crop = Crop { x: -1, y: 0, width: 2, height: 1 };
        let out = crop_bgra(&source(&[5, 6, 7, 0], 4, 1, 1, Channels::Bgr), crop);
        assert_eq!(&*out, [0, 0, 0, 255, 5, 6, 7, 255]);
        let below = Crop { x: 0, y: 3, width: 1, height: 1 };
        assert_eq!(&*crop_bgra(&source(&[5, 6, 7, 0], 4, 1, 1, Channels::Bgr), below), [0, 0, 0, 255]);
        // The crop reaches past the right edge: the last column is black.
        let right = Crop { x: 0, y: 0, width: 2, height: 1 };
        assert_eq!(&*crop_bgra(&source(&[5, 6, 7, 0], 4, 1, 1, Channels::Bgr), right), [5, 6, 7, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn a_short_buffer_leaves_its_rows_black_instead_of_panicking() {
        let crop = Crop { x: 0, y: 0, width: 2, height: 2 };
        let out = crop_bgra(&source(&[1, 2, 3, 0, 4, 5, 6, 0], 8, 2, 2, Channels::Bgr), crop);
        assert_eq!(&*out, [1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn format_pod_serializes() {
        let bytes = format_pod(30).expect("the format pod serializes");
        assert!(Pod::from_bytes(&bytes).is_some());
    }

    #[test]
    fn format_pod_caps_the_frame_rate_at_the_recording_rate() {
        let bytes = format_pod(15).expect("the format pod serializes");
        let (_, value) = PodDeserializer::deserialize_any_from(&bytes).expect("the pod parses");
        let Value::Object(object) = value else { panic!("the format is an object") };
        let property: Option<&Property> = object.properties.iter().find(|p| p.key == FormatProperties::VideoMaxFramerate.as_raw());
        let Some(Value::Choice(ChoiceValue::Fraction(Choice(_, ChoiceEnum::Range { default, min, max })))) = property.map(|p| &p.value)
        else {
            panic!("maxFramerate is a range")
        };
        assert_eq!((*default, *min, *max), (Fraction { num: 15, denom: 1 }, Fraction { num: 0, denom: 1 }, Fraction { num: 15, denom: 1 }));
    }

    fn frames(fps: u32) -> PipeWireFrames {
        PipeWireFrames {
            shared: Arc::new(Shared::default()),
            width: 1,
            height: 1,
            fps,
            quit: None,
            thread: None,
            last: None,
            session: None,
        }
    }

    fn frame(value: u8) -> Arc<[u8]> {
        Arc::from([value, value, value, 255])
    }

    fn pixels(frame: Result<Option<Bitmap>, PortError>) -> Vec<u8> {
        frame.expect("a frame").expect("not ended").pixels().to_vec()
    }

    #[test]
    fn the_newest_queued_frame_wins() {
        let mut source = frames(100);
        source.shared.push(frame(1));
        source.shared.push(frame(2));
        assert_eq!(pixels(source.next_frame()), [2, 2, 2, 255]);
        // The older frame was dropped, not kept for the next call: with nothing new, the last repeats.
        assert!(source.shared.queue.lock().expect("lock").frames.is_empty());
    }

    #[test]
    fn the_last_frame_repeats_only_when_nothing_arrives() {
        let mut source = frames(100);
        source.shared.push(frame(1));
        assert_eq!(pixels(source.next_frame()), [1, 1, 1, 255]);
        let started = Instant::now();
        assert_eq!(pixels(source.next_frame()), [1, 1, 1, 255]);
        assert!(started.elapsed() >= Duration::from_millis(8), "the repeat waits one frame period");
        source.shared.push(frame(3));
        assert_eq!(pixels(source.next_frame()), [3, 3, 3, 255]);
    }

    #[test]
    fn a_timeout_is_not_an_error_for_the_queue() {
        let shared = Shared::default();
        assert!(matches!(shared.newest(Duration::from_millis(5)), Ok(Pop::TimedOut)));
    }

    #[test]
    fn a_stream_error_is_an_error_even_after_a_frame() {
        let mut source = frames(100);
        source.shared.push(frame(1));
        assert_eq!(pixels(source.next_frame()), [1, 1, 1, 255]);
        source.shared.finish(Err("stream broke".into()));
        match source.next_frame() {
            Err(PortError::Failed(message)) => assert_eq!(message, "stream broke"),
            other => panic!("expected the stream error, got {other:?}"),
        }
        // And it stays an error; the last frame is not repeated.
        assert!(source.next_frame().is_err());
    }

    #[test]
    fn queued_frames_come_before_the_end_of_the_stream() {
        let mut source = frames(100);
        source.shared.push(frame(4));
        source.shared.finish(Err("stream broke".into()));
        assert_eq!(pixels(source.next_frame()), [4, 4, 4, 255]);
        assert!(source.next_frame().is_err());
        let mut clean = frames(100);
        clean.shared.finish(Ok(()));
        assert!(matches!(clean.next_frame(), Ok(None)));
    }

    #[test]
    fn a_poisoned_queue_is_an_error_even_after_a_frame() {
        let mut source = frames(100);
        source.shared.push(frame(1));
        assert_eq!(pixels(source.next_frame()), [1, 1, 1, 255]);
        let shared = Arc::clone(&source.shared);
        let _ = thread::spawn(move || {
            let _guard = shared.queue.lock().expect("lock");
            panic!("poison the queue");
        })
        .join();
        assert!(matches!(source.next_frame(), Err(PortError::Failed(_))));
    }

    fn negotiated(width: u32, height: u32) -> Negotiated {
        Negotiated { order: Channels::Bgr, width, height }
    }

    fn buffer(size: usize) -> BufferInfo {
        BufferInfo { offset: 0, size, stride: 0, corrupted: false }
    }

    const WHOLE: Crop = Crop { x: 0, y: 0, width: 2, height: 2 };

    #[test]
    fn a_corrupted_buffer_is_skipped() {
        let bytes = [7u8; 16];
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &bytes, buffer(16)).is_some());
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &bytes, BufferInfo { corrupted: true, ..buffer(16) }).is_none());
    }

    #[test]
    fn a_buffer_shorter_than_the_frame_is_skipped() {
        let bytes = [7u8; 16];
        // The chunk says 12 bytes of the 16 that 2x2 needs, and an empty chunk has nothing at all.
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &bytes, buffer(12)).is_none());
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &bytes, buffer(0)).is_none());
        // The chunk is long enough but the mapping is not.
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &bytes[..8], buffer(16)).is_none());
        // Padding after the last row is not needed: 2 rows of stride 12, the last one 8 bytes.
        let padded = BufferInfo { stride: 12, ..buffer(20) };
        assert!(crop_buffer(negotiated(2, 2), WHOLE, &[7u8; 24], padded).is_some());
    }

    #[test]
    fn the_crop_of_a_buffer_reads_at_its_offset() {
        let mut bytes = vec![0xEE; 4];
        bytes.extend([1, 2, 3, 0]);
        let crop = Crop { x: 0, y: 0, width: 1, height: 1 };
        let out = crop_buffer(negotiated(1, 1), crop, &bytes, BufferInfo { offset: 4, ..buffer(4) });
        assert_eq!(out.as_deref(), Some(&[1, 2, 3, 255][..]));
    }

    #[test]
    fn an_area_inside_the_stream_passes() {
        assert!(check_area(Crop { x: 0, y: 0, width: 1920, height: 1080 }, (1920, 1080)).is_ok());
        assert!(check_area(Crop { x: 100, y: 50, width: 200, height: 100 }, (1920, 1080)).is_ok());
    }

    #[test]
    fn an_area_on_another_screen_fails_with_a_message_for_the_user() {
        // A stream at (1920, 0), and an area on the monitor to its left: the crop is at x = -1920.
        let message = check_area(Crop { x: -1920, y: 0, width: 800, height: 600 }, (1920, 1080)).expect_err("outside");
        assert!(message.starts_with("the area is not on the shared screen"), "{message}");
        // Past the right and bottom edges, and partly outside.
        assert!(check_area(Crop { x: 1920, y: 0, width: 800, height: 600 }, (1920, 1080)).is_err());
        assert!(check_area(Crop { x: 0, y: 1000, width: 800, height: 600 }, (1920, 1080)).is_err());
        assert!(check_area(Crop { x: -2, y: 0, width: 800, height: 600 }, (1920, 1080)).is_err());
    }

    #[test]
    fn an_off_stream_area_becomes_a_failed_port_error() {
        let error = PortError::from(StartError::AreaOffStream("the area is not on the shared screen".into()));
        assert!(matches!(error, PortError::Failed(message) if message.starts_with("the area is not on")));
    }
}

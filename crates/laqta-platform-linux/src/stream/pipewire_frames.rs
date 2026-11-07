//! Frames from a ScreenCast node through PipeWire (ADR 016, M4 D2). A thread of its own runs the
//! PipeWire loop and copies each frame, cropped to the recording area and converted to BGRA, into a
//! queue of two. `next_frame` takes the oldest frame from that queue.
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

impl PipeWireFrames {
    /// Connects to the node of `cast` and starts pumping frames. `area` has even dimensions and is
    /// in physical pixels. The stream's position (zero when the portal gives none) is subtracted from
    /// it to find the crop.
    pub fn start(cast: Cast, area: PhysRect, fps: u32) -> Result<Self, PortError> {
        let (ox, oy) = cast.position.unwrap_or((0, 0));
        let crop =
            Crop { x: i64::from(area.x) - i64::from(ox), y: i64::from(area.y) - i64::from(oy), width: area.width, height: area.height };
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
            session: Some(session),
            last: None,
        };
        let job = Job { crop, fps };
        let thread = thread::Builder::new()
            .name("laqta-pipewire".into())
            .spawn({
                let shared = Arc::clone(&shared);
                move || run(fd, node_id, job, shared, quit_rx, ready_tx)
            })
            .map_err(|e| PortError::Failed(format!("PipeWire thread: {e}")))?;
        frames.thread = Some(thread);
        match ready_rx.recv_timeout(CONNECT_TIMEOUT) {
            Ok(Ok(())) => Ok(frames),
            Ok(Err(message)) => Err(PortError::Failed(message)),
            Err(_) => Err(PortError::Failed("PipeWire did not connect to the screen stream in time".into())),
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

    /// Paced at the stream's frame rate: compositors send frames only on damage, so on a still
    /// screen the last frame repeats instead of the recording timing out.
    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        let wait = match &self.last {
            Some(_) => Duration::from_secs(1) / self.fps.max(1),
            None => FRAME_TIMEOUT,
        };
        let pixels = match self.shared.pop(wait) {
            Ok(Some(pixels)) => pixels,
            Ok(None) => return Ok(None),
            Err(e) => match &self.last {
                Some(last) => last.clone(),
                None => return Err(e),
            },
        };
        self.last = Some(pixels.clone());
        Bitmap::new(self.width, self.height, PixelFormat::Bgra8, pixels).map(Some).map_err(|e| PortError::Failed(format!("screen frame: {e}")))
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

    /// The next frame, or `None` once the stream ended cleanly. Waits up to `timeout` for a frame.
    fn pop(&self, timeout: Duration) -> Result<Option<Arc<[u8]>>, PortError> {
        let deadline = Instant::now() + timeout;
        let mut queue = self.queue.lock().map_err(|_| poisoned())?;
        loop {
            if let Some(frame) = queue.frames.pop_front() {
                return Ok(Some(frame));
            }
            match &queue.end {
                Some(Ok(())) => return Ok(None),
                Some(Err(message)) => return Err(PortError::Failed(message.clone())),
                None => {}
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(PortError::Failed(format!("no frame from the screen stream within {} s", timeout.as_secs())));
            }
            queue = self.ready.wait_timeout(queue, left).map_err(|_| poisoned())?.0;
        }
    }
}

fn poisoned() -> PortError {
    PortError::Failed("screen frame queue is poisoned".into())
}

/// What the PipeWire thread needs besides the socket and the node.
struct Job {
    crop: Crop,
    fps: u32,
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

/// Copies `crop` out of `src` as BGRA with alpha 255. Pixels outside the frame are black.
pub(crate) fn crop_bgra(src: &Source<'_>, crop: Crop) -> Vec<u8> {
    let (width, height) = (crop.width as usize, crop.height as usize);
    let mut out = [0, 0, 0, 255].repeat(width * height);
    let columns = visible(crop.x, width, src.width);
    for row in 0..height {
        let Some(sy) = visible_index(crop.y + row as i64, src.height) else { continue };
        let line = src.bytes.get(sy * src.stride..).unwrap_or(&[]);
        for col in columns.clone() {
            let Ok(sx) = usize::try_from(crop.x + col as i64) else { continue };
            let Some(px) = line.get(sx * 4..sx * 4 + 4).and_then(|px| <[u8; 4]>::try_from(px).ok()) else { continue };
            let at = (row * width + col) * 4;
            if let Some(dst) = out.get_mut(at..at + 4) {
                dst.copy_from_slice(&convert(px, src.order));
            }
        }
    }
    out
}

/// One 4-byte pixel as BGRA with alpha 255.
fn convert(px: [u8; 4], order: Channels) -> [u8; 4] {
    match order {
        Channels::Bgr => [px[0], px[1], px[2], 255],
        Channels::Rgb => [px[2], px[1], px[0], 255],
    }
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

/// The data the stream listener carries on the PipeWire thread.
struct Frames {
    shared: Arc<Shared>,
    mainloop: MainLoopWeak,
    crop: Crop,
    format: Option<Negotiated>,
}

impl Frames {
    fn end(&self, end: Result<(), String>) {
        self.shared.finish(end);
        if let Some(mainloop) = self.mainloop.upgrade() {
            mainloop.quit();
        }
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
    }

    /// The crop of one buffer, or `None` when the buffer does not match the negotiated format.
    fn frame(&self, mapped: &[u8], offset: usize, size: usize, stride: i32) -> Option<Arc<[u8]>> {
        let format = self.format?;
        let row_bytes = (format.width as usize).checked_mul(4)?;
        if row_bytes == 0 || format.height == 0 {
            return None;
        }
        let stride = match usize::try_from(stride) {
            Ok(stride) if stride > 0 => stride,
            _ => row_bytes,
        };
        if stride < row_bytes {
            return None;
        }
        let bytes = mapped.get(offset..offset.checked_add(size)?)?;
        let src = Source { bytes, stride, width: format.width, height: format.height, order: format.order };
        Some(Arc::from(crop_bgra(&src, self.crop)))
    }
}

/// The PipeWire thread: connects to the node, runs the loop until quit or until the stream ends.
/// `ready` reports the outcome of connecting. Everything PipeWire is created and dropped here, since
/// its objects are not `Send`.
fn run(
    fd: OwnedFd,
    node_id: u32,
    job: Job,
    shared: Arc<Shared>,
    quit: pipewire::channel::Receiver<()>,
    ready: mpsc::Sender<Result<(), String>>,
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
            format: None,
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
            let (offset, size, stride) = {
                let chunk = data.chunk();
                (chunk.offset() as usize, chunk.size() as usize, chunk.stride())
            };
            let Some(mapped) = data.data() else { return };
            if let Some(frame) = frames.frame(mapped, offset, size, stride) {
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
    let _ = ready.send(Ok(()));
    mainloop.run();
    shared.finish(Ok(()));
}

fn fail(ready: &mpsc::Sender<Result<(), String>>, message: String) {
    let _ = ready.send(Err(message));
}

/// The `EnumFormat` pod the stream asks for: raw video in BGRx, BGRA or RGBx at any size and up to
/// the requested rate. It names no modifier, so the producer hands over shared-memory buffers.
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
    );
    PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &Value::Object(object))
        .map(|(cursor, _)| cursor.into_inner())
        .map_err(|e| format!("{e:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(bytes: &[u8], stride: usize, width: u32, height: u32, order: Channels) -> Source<'_> {
        Source { bytes, stride, width, height, order }
    }

    #[test]
    fn copies_a_padded_frame_as_bgra() {
        // 2x2 BGRx with 4 bytes of padding per row. The padding and the x byte are not copied.
        let bytes = [1, 2, 3, 9, 4, 5, 6, 9, 0xEE, 0xEE, 0xEE, 0xEE, 7, 8, 9, 9, 10, 11, 12, 9, 0xEE, 0xEE, 0xEE, 0xEE];
        let crop = Crop { x: 0, y: 0, width: 2, height: 2 };
        let out = crop_bgra(&source(&bytes, 12, 2, 2, Channels::Bgr), crop);
        assert_eq!(out, [1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]);
    }

    #[test]
    fn rgbx_swaps_red_and_blue() {
        let crop = Crop { x: 0, y: 0, width: 1, height: 1 };
        let out = crop_bgra(&source(&[1, 2, 3, 0], 4, 1, 1, Channels::Rgb), crop);
        assert_eq!(out, [3, 2, 1, 255]);
    }

    #[test]
    fn crops_an_offset_window() {
        // 3x2 frame: pixel (c, r) holds [c, r, 0, 0].
        let bytes: Vec<u8> = (0..2u8).flat_map(|r| (0..3u8).flat_map(move |c| [c, r, 0, 0])).collect();
        let crop = Crop { x: 1, y: 1, width: 2, height: 1 };
        let out = crop_bgra(&source(&bytes, 12, 3, 2, Channels::Bgr), crop);
        assert_eq!(out, [1, 1, 0, 255, 2, 1, 0, 255]);
    }

    #[test]
    fn pixels_outside_the_frame_are_black() {
        // Crop starts one column left of a 1x1 frame and is two columns wide: column 0 is black.
        let crop = Crop { x: -1, y: 0, width: 2, height: 1 };
        let out = crop_bgra(&source(&[5, 6, 7, 0], 4, 1, 1, Channels::Bgr), crop);
        assert_eq!(out, [0, 0, 0, 255, 5, 6, 7, 255]);
        let below = Crop { x: 0, y: 3, width: 1, height: 1 };
        assert_eq!(crop_bgra(&source(&[5, 6, 7, 0], 4, 1, 1, Channels::Bgr), below), [0, 0, 0, 255]);
    }

    #[test]
    fn a_short_buffer_leaves_its_rows_black_instead_of_panicking() {
        let crop = Crop { x: 0, y: 0, width: 2, height: 2 };
        let out = crop_bgra(&source(&[1, 2, 3, 0, 4, 5, 6, 0], 8, 2, 2, Channels::Bgr), crop);
        assert_eq!(out, [1, 2, 3, 255, 4, 5, 6, 255, 0, 0, 0, 255, 0, 0, 0, 255]);
    }

    #[test]
    fn format_pod_serializes() {
        let bytes = format_pod(30).expect("the format pod serializes");
        assert!(Pod::from_bytes(&bytes).is_some());
    }
}

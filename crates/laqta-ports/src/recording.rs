//! Recording (ADR 016): the platform's `ScreenStream` and the `Media` encoder port.

use std::path::{Path, PathBuf};

use laqta_domain::Bitmap;
use laqta_domain::geometry::PhysRect;
use laqta_domain::record::{AudioDevice, Quality, RecordSpec, VideoCodec};

use crate::{BoxFuture, PortError};

/// What a platform adapter knows about a screen, for an encoder that captures it itself.
/// Only platform adapters build it and only `laqta-media` reads it (ADR 016).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OsScreenRef(OsScreen);

#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OsScreen {
    /// A DXGI output: the adapter (enumeration index) that owns it and its index on that adapter,
    /// and the area to capture relative to the output's top-left corner (M4 D1).
    Windows { adapter_index: u32, output_index: u32, offset_x: i32, offset_y: i32, width: u32, height: u32, desktop_x: i32, desktop_y: i32 },
    /// An X11 display (`:0`) and the area in root-window pixels.
    X11 { display: String, x: i32, y: i32, width: u32, height: u32 },
}

impl OsScreenRef {
    #[doc(hidden)]
    pub fn new(screen: OsScreen) -> Self {
        OsScreenRef(screen)
    }
    #[doc(hidden)]
    pub fn screen(&self) -> &OsScreen {
        &self.0
    }
}

/// Frames Laqta pumps to the encoder (Wayland PipeWire, Windows WGC). Blocking calls; run them
/// off the async worker.
pub trait FrameSource: Send {
    fn size(&self) -> (u32, u32);
    fn fps(&self) -> u32;
    /// The next frame (BGRA or RGBA, exactly `size()`), or `None` once the source ended.
    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError>;
    fn stop(&mut self);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PcmFormat {
    pub sample_rate: u32,
    pub channels: u16,
}

/// Interleaved 16-bit PCM captured by the platform (Windows system audio).
pub trait PcmSource: Send {
    fn format(&self) -> PcmFormat;
    /// Fills `buf` and returns the samples written; 0 means the source ended.
    fn read(&mut self, buf: &mut [i16]) -> Result<usize, PortError>;
    fn stop(&mut self);
}

pub enum VideoSource {
    Target(OsScreenRef),
    Frames(Box<dyn FrameSource>),
}

pub enum AudioInput {
    Device(AudioDevice),
    Pcm(Box<dyn PcmSource>),
}

impl std::fmt::Debug for VideoSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            VideoSource::Target(t) => f.debug_tuple("Target").field(t).finish(),
            VideoSource::Frames(s) => f.debug_tuple("Frames").field(&s.size()).finish(),
        }
    }
}

impl std::fmt::Debug for AudioInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AudioInput::Device(d) => f.debug_tuple("Device").field(d).finish(),
            AudioInput::Pcm(p) => f.debug_tuple("Pcm").field(&p.format()).finish(),
        }
    }
}

pub struct RecordingInput {
    pub video: VideoSource,
    pub audio: Vec<AudioInput>,
}

impl std::fmt::Debug for RecordingInput {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RecordingInput").field("video", &self.video).field("audio", &self.audio).finish()
    }
}

pub trait ScreenStream: Send + Sync {
    /// Opens a source for `area` (physical desktop pixels) with the requested audio.
    fn open(&self, area: PhysRect, show_cursor: bool, fps: u32, audio: &[AudioDevice]) -> BoxFuture<'_, Result<RecordingInput, PortError>>;
    /// Microphones and loopback (system sound) devices.
    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<AudioDevice>, PortError>>;
}

/// A recording in progress.
pub trait RecordingHandle: Send {
    fn pause(&mut self) -> BoxFuture<'_, Result<(), PortError>>;
    fn resume(&mut self) -> BoxFuture<'_, Result<(), PortError>>;
    /// Finishes and returns the output file.
    fn stop(self: Box<Self>) -> BoxFuture<'static, Result<PathBuf, PortError>>;
    /// Stops and deletes everything.
    fn abort(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>>;
    /// False once the encoder exited on its own (crash or error).
    fn is_alive(&mut self) -> bool;
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct MediaInfo {
    pub duration_secs: f64,
    pub width: u32,
    pub height: u32,
    pub fps: f64,
    pub video_codec: String,
    pub audio_codec: Option<String>,
    pub size_bytes: u64,
}

/// A file-to-file conversion (video converter, trimmer, GIF maker, thumbnailer).
#[derive(Clone, Debug, PartialEq, Default)]
pub struct TranscodeSpec {
    pub codec: Option<VideoCodec>,
    pub quality: Quality,
    pub fps: Option<u32>,
    pub start_secs: Option<f64>,
    pub duration_secs: Option<f64>,
    /// Scale to this width (height keeps the aspect ratio).
    pub width: Option<u32>,
    /// Extract one frame (thumbnail) at `start_secs` instead of converting.
    pub single_frame: bool,
    pub extra_args: String,
}

pub trait Media: Send + Sync {
    fn start(&self, input: RecordingInput, spec: RecordSpec, output: PathBuf) -> BoxFuture<'_, Result<Box<dyn RecordingHandle>, PortError>>;
    fn transcode(&self, input: &Path, output: &Path, spec: TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>>;
    fn probe(&self, input: &Path) -> BoxFuture<'_, Result<MediaInfo, PortError>>;
}

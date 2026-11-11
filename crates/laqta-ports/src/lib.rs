//! Port traits and the plain types they carry (ARCHITECTURE §2.3, ADR 024).

pub mod desktop;
pub mod error;
pub mod history;
pub mod notify;
pub mod overlay;
pub mod recording;
pub mod runtime;
pub mod screen;
pub mod task;
pub mod tools;
pub mod upload;

#[cfg(feature = "fakes")]
pub mod fakes;

pub use desktop::{
    ActionCallback, AssignedCallback, ChordCallback, Clipboard, ClipboardContent, HotkeyCallback, HotkeyRegistrar, KeyRecording,
    Permission, RejectedCallback, Rejection, Shell, Tray, TrayItem, TrayMenu,
};
pub use error::PortError;
pub use history::HistoryEdit;
pub use notify::{HostInfo, Notifier, Rng, Sound, Toast};
pub use overlay::{EditorHost, HoverSource, InputCallback, Overlay, RegionView, TextLayer, ToolHost, ToolKind, ToolWindow};
pub use recording::{
    AudioInput, FrameSource, Media, MediaInfo, OsScreenRef, PcmFormat, PcmSource, RecordingHandle, RecordingInput, ScreenStream,
    TranscodeSpec, VideoSource,
};
pub use runtime::{Clock, Spawner};
pub use screen::{CaptureRequest, ScreenSource};
pub use task::{AppError, TaskEvent, TaskId, TaskStatus};
pub use tools::{
    Autostart, BackgroundRemover, FileWatcher, ImageAnalyzer, Inpainter, InputSynth, Ocr, OcrLine, OcrResult, OcrWord, PointerHighlighter,
    RemoteEntry, RemoteStorage, ShellIntegration, WatchCallback, WatchHandle, WindowOps,
};
pub use upload::{
    Destination, ProgressCallback, Prompt, SecretStore, Sharer, Shortener, UploadData, UploadError, UploadKind, UploadRequest,
    UploadResult, Uploaders,
};

/// A boxed future, so async ports work behind `Arc<dyn Port>`.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

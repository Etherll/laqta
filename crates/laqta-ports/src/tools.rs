//! Ports used by the tools (M5).

use laqta_domain::Bitmap;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::HoverTarget;

use crate::{BoxFuture, PortError};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OcrLine {
    pub rect: PhysRect,
    pub text: String,
    pub confidence: f32,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct OcrResult {
    pub lines: Vec<OcrLine>,
}

impl OcrResult {
    /// The lines joined top to bottom.
    pub fn text(&self) -> String {
        self.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>().join("\n")
    }
}

/// Text recognition (ADR 023): `laqta-ml`'s model families, or Windows.Media.Ocr.
pub trait Ocr: Send + Sync {
    /// `language` is a BCP-47 tag or the model's script name; empty = the default model.
    fn recognize(&self, image: Bitmap, language: String) -> BoxFuture<'_, Result<OcrResult, PortError>>;
    /// Languages or scripts that are ready (installed) now.
    fn languages(&self) -> Vec<String>;
}

/// Window tools (M5 D7).
pub trait WindowOps: Send + Sync {
    fn set_borderless(&self, window: &HoverTarget, borderless: bool) -> Result<(), PortError>;
    fn set_topmost(&self, window: &HoverTarget, topmost: bool) -> Result<(), PortError>;
    /// Properties for the inspector: (name, value) pairs such as class, pid, style flags.
    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError>;
}

/// Background removal (ADR 023, M7 D6–D8): `laqta-ml` model families or remove.bg (`laqta-cloud`).
pub trait BackgroundRemover: Send + Sync {
    /// The image with its background made transparent: same size, straight alpha.
    fn remove(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>>;
}

/// AI image analysis (M7 D7): one OpenAI-compatible client in `laqta-cloud`.
pub trait ImageAnalyzer: Send + Sync {
    /// The model's answer to `prompt` about `image`.
    fn analyze(&self, image: Bitmap, prompt: String) -> BoxFuture<'_, Result<String, PortError>>;
}

/// Inpainting for the photo editor (M7 D4): the `lama` family in `laqta-ml`.
pub trait Inpainter: Send + Sync {
    /// Fills the pixels where `mask` (one byte per pixel, row-major, non-zero = fill) is set.
    fn inpaint(&self, image: Bitmap, mask: Vec<u8>) -> BoxFuture<'_, Result<Bitmap, PortError>>;
}

/// One entry in a remote folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoteEntry {
    pub name: String,
    /// The full remote path (S3: the key).
    pub path: String,
    pub is_dir: bool,
    pub size: Option<u64>,
    /// As the server reports it (RFC 3339 when known).
    pub modified: Option<String>,
}

/// The remote storage browser (M7 D11): S3, FTP/FTPS and SFTP in `laqta-upload`.
pub trait RemoteStorage: Send + Sync {
    fn list(&self, dir: String) -> BoxFuture<'_, Result<Vec<RemoteEntry>, PortError>>;
    fn download(&self, path: String, dest: std::path::PathBuf) -> BoxFuture<'_, Result<(), PortError>>;
    /// Returns the new remote path.
    fn upload(&self, src: std::path::PathBuf, dir: String) -> BoxFuture<'_, Result<String, PortError>>;
    fn rename(&self, from: String, to: String) -> BoxFuture<'_, Result<(), PortError>>;
    fn delete(&self, path: String) -> BoxFuture<'_, Result<(), PortError>>;
    fn mkdir(&self, path: String) -> BoxFuture<'_, Result<(), PortError>>;
}

/// The mouse highlighter (M5 D9): a click-through, always-on-top circle that follows the pointer.
/// Windows: a layered `WS_EX_TRANSPARENT` window; X11: an override-redirect window with an empty
/// input shape; Wayland returns `Unsupported` (no global pointer position).
pub trait PointerHighlighter: Send + Sync {
    fn set_enabled(&self, on: bool) -> Result<(), PortError>;
    fn is_enabled(&self) -> bool;
}

/// Synthetic input for scrolling capture (M6 D4): Windows `SendInput`, X11 XTEST; Wayland returns
/// `Unsupported` (no portal grants input injection without the RemoteDesktop portal).
pub trait InputSynth: Send + Sync {
    /// Scrolls the window under the point (`x`, `y`) in physical desktop pixels down by `amount` steps.
    fn scroll(&self, x: i32, y: i32, method: laqta_domain::capture::ScrollMethod, amount: u32) -> Result<(), PortError>;
}

/// Called with each file that appeared or changed in a watched folder, once it has settled.
pub type WatchCallback = std::sync::Arc<dyn Fn(std::path::PathBuf) + Send + Sync>;

/// Stops its watch when dropped.
pub struct WatchHandle(pub Box<dyn std::any::Any + Send + Sync>);

impl std::fmt::Debug for WatchHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("WatchHandle")
    }
}

/// Folder watching (M6 D6): `laqta-library`'s `notify` adapter; the fake drives `laqta-app::watch` tests.
pub trait FileWatcher: Send + Sync {
    /// Calls `on_file` for files created or modified under `folder` (and its subfolders when
    /// `recursive`), after writes have stopped for a short settle time.
    fn watch(&self, folder: &std::path::Path, recursive: bool, on_file: WatchCallback) -> Result<WatchHandle, PortError>;
}

/// Run at login (M6 D10): Windows `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`, Linux
/// `~/.config/autostart/laqta.desktop`.
pub trait Autostart: Send + Sync {
    fn is_enabled(&self) -> Result<bool, PortError>;
    fn set_enabled(&self, enabled: bool, executable: &std::path::Path) -> Result<(), PortError>;
}

/// "Upload with Laqta" in the file manager and Send To (M6 D11).
pub trait ShellIntegration: Send + Sync {
    fn is_installed(&self) -> Result<bool, PortError>;
    fn install(&self, executable: &std::path::Path) -> Result<(), PortError>;
    fn uninstall(&self) -> Result<(), PortError>;
    /// Registers the browser native-messaging host manifest (Chrome and Firefox) for `nmh`.
    fn install_native_messaging(&self, nmh: &std::path::Path) -> Result<(), PortError>;
}

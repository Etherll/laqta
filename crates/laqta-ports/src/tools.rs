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

/// Synthetic input for scrolling capture (M6 D4): Windows `SendInput`, X11 XTEST; Wayland returns
/// `Unsupported` (no portal grants input injection without the RemoteDesktop portal).
pub trait InputSynth: Send + Sync {
    /// Scrolls the window under `at` (physical desktop pixels) down by `amount` steps.
    fn scroll(&self, at: laqta_domain::geometry::PhysPoint, method: laqta_domain::capture::ScrollMethod, amount: u32) -> Result<(), PortError>;
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

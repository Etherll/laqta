use std::path::Path;
use std::sync::Arc;

use laqta_domain::{Action, Bitmap, KeyChord};

use crate::PortError;

pub trait Clipboard: Send + Sync {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError>;
    fn set_text(&self, text: &str) -> Result<(), PortError> {
        let _ = text;
        Err(PortError::Unsupported("clipboard text".into()))
    }
    /// Puts files on the clipboard (a file list, as a file manager copy does).
    fn set_files(&self, paths: &[std::path::PathBuf]) -> Result<(), PortError> {
        let _ = paths;
        Err(PortError::Unsupported("clipboard files".into()))
    }
    /// What is on the clipboard now, for "upload from clipboard". `Ok(None)` when it's empty.
    fn get(&self) -> Result<Option<ClipboardContent>, PortError> {
        Err(PortError::Unsupported("reading the clipboard".into()))
    }
}

pub trait Shell: Send + Sync {
    fn open_path(&self, path: &Path) -> Result<(), PortError>;
    /// Shows the file selected in the file manager; falls back to opening its folder.
    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        match path.parent() {
            Some(dir) => self.open_path(dir),
            None => self.open_path(path),
        }
    }
    fn open_url(&self, url: &str) -> Result<(), PortError> {
        let _ = url;
        Err(PortError::Unsupported("open url".into()))
    }
    /// Sends a file to the default printer (`SEND_IMAGE_TO_PRINTER`).
    fn print(&self, path: &Path) -> Result<(), PortError> {
        let _ = path;
        Err(PortError::Unsupported("printing".into()))
    }
    /// Runs a program with an argument list (never through a shell) and returns its exit code.
    fn run(&self, program: &Path, args: &[String], hidden: bool) -> crate::BoxFuture<'_, Result<i32, PortError>> {
        let _ = (program, args, hidden);
        Box::pin(async { Err(PortError::Unsupported("running programs".into())) })
    }
}

/// Clipboard contents, in the order Laqta prefers them.
#[derive(Clone, Debug)]
pub enum ClipboardContent {
    Image(Bitmap),
    Files(Vec<std::path::PathBuf>),
    Text(String),
}

pub type ActionCallback = Arc<dyn Fn(Action) + Send + Sync>;

/// Called with the chord that fired, so the app can pick the workflow bound to that chord.
pub type HotkeyCallback = Arc<dyn Fn(KeyChord) + Send + Sync>;

/// Called with the key the desktop assigned to each chord, as `(chord, key text)`. An empty key
/// text means the desktop has none for that chord.
pub type AssignedCallback = Arc<dyn Fn(Vec<(KeyChord, String)>) + Send + Sync>;

/// Called when the desktop turns down chords that `register` reported as registered (a
/// GlobalShortcuts bind that fails or is cancelled after its dialog).
pub type RejectedCallback = Arc<dyn Fn(Rejection) + Send + Sync>;

/// Chords the desktop turned down after `register` reported them registered, and why.
pub struct Rejection {
    pub chords: Vec<KeyChord>,
    pub reason: String,
    current: Box<dyn Fn() -> bool + Send + Sync>,
}

impl std::fmt::Debug for Rejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Rejection").field("chords", &self.chords).field("reason", &self.reason).finish_non_exhaustive()
    }
}

impl Rejection {
    /// `current` answers whether the registration this rejects is still the live one.
    pub fn new(chords: Vec<KeyChord>, reason: String, current: impl Fn() -> bool + Send + Sync + 'static) -> Self {
        Rejection { chords, reason, current: Box::new(current) }
    }

    /// False once `unregister_all` ended the registration this rejects. Check it while holding the
    /// lock the caller registers under, so a newer registration can't start in between.
    pub fn is_current(&self) -> bool {
        (self.current)()
    }
}

/// Called with each key combination pressed while a [`KeyRecording`] lives.
pub type ChordCallback = Arc<dyn Fn(KeyChord) + Send + Sync>;

/// A running key recording; dropping it stops the recording.
pub struct KeyRecording(#[allow(dead_code)] Box<dyn Send>);

impl KeyRecording {
    /// Wraps the adapter's stop guard: whatever it holds is dropped when the recording ends.
    pub fn new(guard: impl Send + 'static) -> Self {
        KeyRecording(Box::new(guard))
    }
}

impl std::fmt::Debug for KeyRecording {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KeyRecording")
    }
}

pub trait HotkeyRegistrar: Send + Sync {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: HotkeyCallback) -> Vec<(KeyChord, Result<(), PortError>)>;
    /// Drops every registration so `register` can be called again with new bindings.
    fn unregister_all(&self) {}
    /// True when a registered chord's key events go only to the registrar, never to the focused
    /// window (Windows `RegisterHotKey`, X11 `XGrabKey`, macOS Carbon hotkeys). The app then drops
    /// its registrations while a shortcut is being recorded, so a bound chord can be recorded again.
    fn grabs_keys(&self) -> bool {
        false
    }
    /// Called with the keys the desktop assigned to the registered chords, when it reports them
    /// (KDE Wayland, after the user changes a shortcut in the desktop's settings). Call before
    /// `register`; the callback may run at any time after that.
    fn on_assigned(&self, _assigned: AssignedCallback) {}
    /// Called when the desktop turns the registered chords down after `register` reported them
    /// registered: the GlobalShortcuts portal binds after `register` returns, as the bind can wait on
    /// a dialog. None of the chords is active then. Call before `register`.
    fn on_rejected(&self, _rejected: RejectedCallback) {}
    /// Reports the key combinations pressed while the returned recording lives, for the keys a
    /// window can't see on this platform (Windows: Print Screen, Pause, Scroll Lock and media
    /// keys). The keys still reach the system as usual.
    fn record_keys(&self, _on_chord: ChordCallback) -> Result<KeyRecording, PortError> {
        Err(PortError::Unsupported("windows see every key here".into()))
    }
    /// Opens the desktop's shortcut settings, where it has them.
    fn configure(&self) -> Result<(), PortError> {
        Err(PortError::Unsupported("the desktop has no shortcut settings to open".into()))
    }
}

/// A privacy permission the OS grants each app on its own (macOS: Privacy & Security). Only
/// macOS has these; the onboarding window lists them (ADR 030).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Permission {
    /// Screenshots, recording, and the titles of other apps' windows. Laqta can't capture without it.
    ScreenRecording,
    /// Hovering a window's parts in the region overlay, and scrolling another app's window.
    Accessibility,
    /// Seeing clicks in other apps, for the pointer highlighter.
    InputMonitoring,
}

impl Permission {
    pub const ALL: [Permission; 3] = [Permission::ScreenRecording, Permission::Accessibility, Permission::InputMonitoring];
}

#[derive(Clone, Debug)]
pub struct TrayItem {
    pub label: String,
    pub action: Action,
}

#[derive(Clone, Debug)]
pub struct TrayMenu {
    pub tooltip: String,
    pub default_action: Action,
    pub items: Vec<TrayItem>,
}

pub trait Tray: Send + Sync {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError>;
}

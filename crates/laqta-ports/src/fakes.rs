//! Fakes for every port, plus a manual clock. Compiled only with the `fakes` feature.
//! Fakes run work inline, so tests are deterministic.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laqta_domain::{Action, Bitmap, Frame, KeyChord};

use laqta_domain::region::{HoverTarget, RegionInput};

use crate::{ActionCallback, BoxFuture, Clipboard, Clock, HotkeyRegistrar, HoverSource, InputCallback, Notifier, Overlay, PortError, RegionView, Rng, ScreenSource, Shell, Spawner, Toast, Tray, TrayMenu};

/// Returns the configured frames (or error) on every call.
#[derive(Debug)]
pub struct FakeScreen {
    pub result: Mutex<Result<Vec<Frame>, PortError>>,
    pub calls: Mutex<u32>,
}

impl FakeScreen {
    pub fn new(frames: Vec<Frame>) -> Self {
        FakeScreen { result: Mutex::new(Ok(frames)), calls: Mutex::new(0) }
    }
    pub fn failing(error: PortError) -> Self {
        FakeScreen { result: Mutex::new(Err(error)), calls: Mutex::new(0) }
    }
}

impl ScreenSource for FakeScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        *self.calls.lock().unwrap() += 1;
        let r = self.result.lock().unwrap().clone();
        Box::pin(async move { r })
    }
}

/// What the fake clipboard does on `set_image`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBehaviour {
    Succeed,
    Fail,
    Panic,
}

#[derive(Debug)]
pub struct FakeClipboard {
    pub behaviour: Mutex<ClipboardBehaviour>,
    pub images: Mutex<Vec<Bitmap>>,
    pub texts: Mutex<Vec<String>>,
    pub files: Mutex<Vec<Vec<PathBuf>>>,
}

impl FakeClipboard {
    pub fn new(behaviour: ClipboardBehaviour) -> Self {
        FakeClipboard { behaviour: Mutex::new(behaviour), images: Mutex::new(Vec::new()), texts: Mutex::new(Vec::new()), files: Mutex::new(Vec::new()) }
    }
}

impl Clipboard for FakeClipboard {
    fn set_image(&self, image: &Bitmap) -> Result<(), PortError> {
        let b = *self.behaviour.lock().unwrap();
        match b {
            ClipboardBehaviour::Succeed => {
                self.images.lock().unwrap().push(image.clone());
                Ok(())
            }
            ClipboardBehaviour::Fail => Err(PortError::Failed("clipboard unavailable".into())),
            ClipboardBehaviour::Panic => panic!("fake clipboard panicked"),
        }
    }
    fn set_text(&self, text: &str) -> Result<(), PortError> {
        self.texts.lock().unwrap().push(text.to_string());
        Ok(())
    }
    fn set_files(&self, paths: &[PathBuf]) -> Result<(), PortError> {
        self.files.lock().unwrap().push(paths.to_vec());
        Ok(())
    }
}

#[derive(Debug, Default)]
pub struct FakeShell {
    pub opened: Mutex<Vec<PathBuf>>,
    pub revealed: Mutex<Vec<PathBuf>>,
    pub urls: Mutex<Vec<String>>,
}

impl Shell for FakeShell {
    fn open_path(&self, path: &Path) -> Result<(), PortError> {
        self.opened.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn reveal(&self, path: &Path) -> Result<(), PortError> {
        self.revealed.lock().unwrap().push(path.to_path_buf());
        Ok(())
    }
    fn open_url(&self, url: &str) -> Result<(), PortError> {
        self.urls.lock().unwrap().push(url.to_string());
        Ok(())
    }
}

/// Records bindings and lets a test fire them.
#[derive(Default)]
pub struct FakeHotkeys {
    pub callback: Mutex<Option<ActionCallback>>,
    pub bindings: Mutex<Vec<(KeyChord, Action)>>,
}

impl std::fmt::Debug for FakeHotkeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeHotkeys").field("bindings", &self.bindings).finish()
    }
}

impl FakeHotkeys {
    pub fn fire(&self, action: Action) {
        let cb = self.callback.lock().unwrap().clone();
        if let Some(cb) = cb {
            cb(action);
        }
    }
}

impl HotkeyRegistrar for FakeHotkeys {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        *self.callback.lock().unwrap() = Some(on_fire);
        self.bindings.lock().unwrap().extend_from_slice(bindings);
        bindings.iter().map(|(c, _)| (*c, Ok(()))).collect()
    }
}

#[derive(Default)]
pub struct FakeTray {
    pub menu: Mutex<Option<TrayMenu>>,
    pub callback: Mutex<Option<ActionCallback>>,
}

impl std::fmt::Debug for FakeTray {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeTray").field("menu", &self.menu).finish()
    }
}

impl FakeTray {
    pub fn select(&self, action: Action) {
        let cb = self.callback.lock().unwrap().clone();
        if let Some(cb) = cb {
            cb(action);
        }
    }
}

impl Tray for FakeTray {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError> {
        *self.menu.lock().unwrap() = Some(menu);
        *self.callback.lock().unwrap() = Some(on_select);
        Ok(())
    }
}

/// A clock whose time only moves when the test says so. `sleep` completes immediately and
/// advances the clock by the duration.
#[derive(Debug)]
pub struct ManualClock {
    now: Arc<Mutex<jiff::civil::DateTime>>,
}

impl ManualClock {
    pub fn new(now: jiff::civil::DateTime) -> Self {
        ManualClock { now: Arc::new(Mutex::new(now)) }
    }
    pub fn set(&self, now: jiff::civil::DateTime) {
        *self.now.lock().unwrap() = now;
    }
    pub fn advance(&self, by: Duration) {
        let mut n = self.now.lock().unwrap();
        *n = n.checked_add(by).unwrap();
    }
}

impl Clock for ManualClock {
    fn now(&self) -> jiff::civil::DateTime {
        *self.now.lock().unwrap()
    }
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        self.advance(duration);
        Box::pin(async {})
    }
}

/// Runs every piece of work inline on the calling thread. `spawn` hands futures to a queue that
/// the test drives (`spawned()`), so nothing runs in the background. Counts calls per kind.
#[derive(Default)]
pub struct InlineSpawner {
    pub spawned: Mutex<Vec<BoxFuture<'static, ()>>>,
    pub blocking_calls: Mutex<u32>,
    pub compute_calls: Mutex<u32>,
}

impl std::fmt::Debug for InlineSpawner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InlineSpawner")
            .field("blocking_calls", &self.blocking_calls)
            .field("compute_calls", &self.compute_calls)
            .finish()
    }
}

impl InlineSpawner {
    /// Takes every future spawned so far.
    pub fn take_spawned(&self) -> Vec<BoxFuture<'static, ()>> {
        std::mem::take(&mut *self.spawned.lock().unwrap())
    }
}

impl Spawner for InlineSpawner {
    fn spawn(&self, task: BoxFuture<'static, ()>) {
        self.spawned.lock().unwrap().push(task);
    }
    fn run_blocking(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        *self.blocking_calls.lock().unwrap() += 1;
        work();
        Box::pin(async {})
    }
    fn run_compute(&self, work: Box<dyn FnOnce() + Send>) -> BoxFuture<'static, ()> {
        *self.compute_calls.lock().unwrap() += 1;
        work();
        Box::pin(async {})
    }
}

/// Records what the overlay was asked to show; tests feed input with `send`.
#[derive(Default)]
pub struct FakeOverlay {
    pub shown: Mutex<Option<(Vec<Frame>, RegionView)>>,
    pub views: Mutex<Vec<RegionView>>,
    pub closed: Mutex<bool>,
    pub fail_show: Mutex<Option<PortError>>,
    callback: Mutex<Option<InputCallback>>,
}

impl std::fmt::Debug for FakeOverlay {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeOverlay").field("closed", &self.closed).finish_non_exhaustive()
    }
}

impl FakeOverlay {
    /// Delivers one input as if the user did it. Returns false if the overlay isn't open.
    pub fn send(&self, input: RegionInput) -> bool {
        let cb = self.callback.lock().unwrap().clone();
        match cb {
            Some(cb) if !*self.closed.lock().unwrap() => {
                cb(input);
                true
            }
            _ => false,
        }
    }
    pub fn is_open(&self) -> bool {
        self.callback.lock().unwrap().is_some() && !*self.closed.lock().unwrap()
    }
    pub fn last_view(&self) -> Option<RegionView> {
        self.views.lock().unwrap().last().cloned()
    }
}

impl Overlay for FakeOverlay {
    fn show(&self, frames: Vec<Frame>, view: RegionView, on_input: InputCallback) -> Result<(), PortError> {
        if let Some(e) = self.fail_show.lock().unwrap().clone() {
            return Err(e);
        }
        *self.shown.lock().unwrap() = Some((frames, view.clone()));
        self.views.lock().unwrap().push(view);
        *self.callback.lock().unwrap() = Some(on_input);
        *self.closed.lock().unwrap() = false;
        Ok(())
    }
    fn update(&self, view: RegionView) {
        self.views.lock().unwrap().push(view);
    }
    fn close(&self) {
        *self.closed.lock().unwrap() = true;
    }
}

/// Returns fixed hover targets (or an error) and an optional active window.
#[derive(Debug)]
pub struct FakeHoverSource {
    pub targets: Mutex<Result<Vec<HoverTarget>, PortError>>,
    pub active: Mutex<Option<HoverTarget>>,
}

impl FakeHoverSource {
    pub fn new(targets: Vec<HoverTarget>) -> Self {
        FakeHoverSource { targets: Mutex::new(Ok(targets)), active: Mutex::new(None) }
    }
    pub fn unsupported() -> Self {
        FakeHoverSource { targets: Mutex::new(Err(PortError::Unsupported("hover targets".into()))), active: Mutex::new(None) }
    }
}

impl HoverSource for FakeHoverSource {
    fn targets(&self) -> BoxFuture<'_, Result<Vec<HoverTarget>, PortError>> {
        let r = self.targets.lock().unwrap().clone();
        Box::pin(async move { r })
    }
    fn active_window(&self) -> BoxFuture<'_, Result<Option<HoverTarget>, PortError>> {
        let r = self.active.lock().unwrap().clone();
        Box::pin(async move { Ok(r) })
    }
}

#[derive(Debug, Default)]
pub struct FakeNotifier {
    pub toasts: Mutex<Vec<Toast>>,
}

impl Notifier for FakeNotifier {
    fn toast(&self, toast: Toast) -> Result<(), PortError> {
        self.toasts.lock().unwrap().push(toast);
        Ok(())
    }
}

/// Deterministic: returns 0, 1, 2, …
#[derive(Debug, Default)]
pub struct CountingRng {
    next: Mutex<u64>,
}

impl Rng for CountingRng {
    fn next_u64(&self) -> u64 {
        let mut n = self.next.lock().unwrap();
        let v = *n;
        *n += 1;
        v
    }
}

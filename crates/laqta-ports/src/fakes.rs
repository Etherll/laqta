//! Fakes for every port, plus a manual clock. Compiled only with the `fakes` feature.
//! Fakes run work inline, so tests are deterministic.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laqta_domain::{Action, Bitmap, Frame, KeyChord};

use crate::{ActionCallback, BoxFuture, Clipboard, Clock, HotkeyRegistrar, PortError, ScreenSource, Shell, Spawner, Tray, TrayMenu};

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
}

impl FakeClipboard {
    pub fn new(behaviour: ClipboardBehaviour) -> Self {
        FakeClipboard { behaviour: Mutex::new(behaviour), images: Mutex::new(Vec::new()) }
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
}

#[derive(Debug, Default)]
pub struct FakeShell {
    pub opened: Mutex<Vec<PathBuf>>,
}

impl Shell for FakeShell {
    fn open_path(&self, path: &Path) -> Result<(), PortError> {
        self.opened.lock().unwrap().push(path.to_path_buf());
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

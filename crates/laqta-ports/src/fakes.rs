//! Fakes for every port, plus a manual clock. Compiled only with the `fakes` feature.
//! Fakes run work inline, so tests are deterministic.
#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laqta_domain::{Action, Bitmap, Frame, KeyChord};

use laqta_domain::region::{HoverTarget, RegionInput};

use crate::{
    ActionCallback, BoxFuture, CaptureRequest, Clipboard, Clock, HotkeyRegistrar, HoverSource, InputCallback, Notifier, Overlay, PortError,
    RegionView, Rng, ScreenSource, Shell, Spawner, Toast, Tray, TrayMenu,
};

/// Returns the configured frames (or error) on every call. `capture_window` returns `window_image`
/// when it is set, and is unsupported otherwise.
#[derive(Debug)]
pub struct FakeScreen {
    pub result: Mutex<Result<Vec<Frame>, PortError>>,
    pub calls: Mutex<u32>,
    pub window_image: Mutex<Option<Bitmap>>,
}

impl FakeScreen {
    pub fn new(frames: Vec<Frame>) -> Self {
        FakeScreen { result: Mutex::new(Ok(frames)), calls: Mutex::new(0), window_image: Mutex::new(None) }
    }
    pub fn failing(error: PortError) -> Self {
        FakeScreen { result: Mutex::new(Err(error)), calls: Mutex::new(0), window_image: Mutex::new(None) }
    }
}

impl ScreenSource for FakeScreen {
    fn capture_all(&self) -> BoxFuture<'_, Result<Vec<Frame>, PortError>> {
        *self.calls.lock().unwrap() += 1;
        let r = self.result.lock().unwrap().clone();
        Box::pin(async move { r })
    }

    fn capture_window(&self, _target: &HoverTarget, _request: CaptureRequest) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        let image = self.window_image.lock().unwrap().clone();
        Box::pin(async move { image.ok_or_else(|| PortError::Unsupported("transparent window capture".into())) })
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
    pub contents: Mutex<Option<crate::ClipboardContent>>,
    pub behaviour: Mutex<ClipboardBehaviour>,
    pub images: Mutex<Vec<Bitmap>>,
    pub texts: Mutex<Vec<String>>,
    pub files: Mutex<Vec<Vec<PathBuf>>>,
}

impl FakeClipboard {
    pub fn new(behaviour: ClipboardBehaviour) -> Self {
        FakeClipboard {
            contents: Mutex::new(None),
            behaviour: Mutex::new(behaviour),
            images: Mutex::new(Vec::new()),
            texts: Mutex::new(Vec::new()),
            files: Mutex::new(Vec::new()),
        }
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
    fn get(&self) -> Result<Option<crate::ClipboardContent>, PortError> {
        Ok(self.contents.lock().unwrap().clone())
    }
}

#[derive(Debug, Default)]
pub struct FakeShell {
    pub opened: Mutex<Vec<PathBuf>>,
    pub revealed: Mutex<Vec<PathBuf>>,
    pub urls: Mutex<Vec<String>>,
    /// Every `run` call: the program, its arguments and the hidden flag.
    pub runs: Mutex<Vec<(PathBuf, Vec<String>, bool)>>,
    /// Exit codes for the next `run` calls, in order. An empty queue exits with 0.
    pub exit_codes: Mutex<std::collections::VecDeque<i32>>,
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
    fn run(&self, program: &Path, args: &[String], hidden: bool) -> BoxFuture<'_, Result<i32, PortError>> {
        self.runs.lock().unwrap().push((program.to_path_buf(), args.to_vec(), hidden));
        let code = self.exit_codes.lock().unwrap().pop_front().unwrap_or(0);
        Box::pin(async move { Ok(code) })
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
        f.debug_struct("InlineSpawner").field("blocking_calls", &self.blocking_calls).field("compute_calls", &self.compute_calls).finish()
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
    pub sounds: Mutex<Vec<crate::Sound>>,
}

impl Notifier for FakeNotifier {
    fn toast(&self, toast: Toast) -> Result<(), PortError> {
        self.toasts.lock().unwrap().push(toast);
        Ok(())
    }
    fn sound(&self, sound: crate::Sound, _custom: Option<&std::path::Path>) -> Result<(), PortError> {
        self.sounds.lock().unwrap().push(sound);
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

#[derive(Debug, Default)]
pub struct FakeToolHost {
    pub opened: Mutex<Vec<crate::ToolWindow>>,
}

impl crate::ToolHost for FakeToolHost {
    fn open(&self, window: crate::ToolWindow) -> Result<(), PortError> {
        self.opened.lock().unwrap().push(window);
        Ok(())
    }
}

/// Answers prompts from a script: each `ask_text` pops the next text answer, each `choose` the next index.
/// Each file, window and destination prompt pops its own queue; an empty queue does what the trait default does
/// (`pick_files` and `save_path` are unsupported, the after-capture window passes its flags through, and the
/// before-upload window keeps the current destination).
#[derive(Debug, Default)]
pub struct FakePrompt {
    pub texts: Mutex<std::collections::VecDeque<String>>,
    pub choices: Mutex<std::collections::VecDeque<usize>>,
    pub confirms: Mutex<std::collections::VecDeque<bool>>,
    pub shown: Mutex<Vec<(String, String)>>,
    /// The title and option labels of every `choose` call.
    pub chooses: Mutex<Vec<(String, Vec<String>)>>,
    /// Answers for `pick_files`: the files, or an error such as `Cancelled`.
    pub files: Mutex<std::collections::VecDeque<Result<Vec<std::path::PathBuf>, PortError>>>,
    /// Answers for `pick_folder`: the folder, or an error such as `Cancelled`.
    pub folders: Mutex<std::collections::VecDeque<Result<std::path::PathBuf, PortError>>>,
    /// Answers for `save_path`.
    pub save_paths: Mutex<std::collections::VecDeque<Result<std::path::PathBuf, PortError>>>,
    /// The suggested path of every `save_path` call.
    pub suggested: Mutex<Vec<std::path::PathBuf>>,
    /// Answers for `after_capture`.
    pub windows: Mutex<std::collections::VecDeque<Result<(u32, u32), PortError>>>,
    /// Every `after_capture` call: image width, image height, after-capture bits and after-upload bits.
    pub window_calls: Mutex<Vec<(u32, u32, u32, u32)>>,
    /// Answers for `before_upload`: the destination id, or an error.
    pub destinations: Mutex<std::collections::VecDeque<Result<String, PortError>>>,
    /// Every `before_upload` call.
    pub destination_calls: Mutex<Vec<BeforeUploadCall>>,
}

/// One `before_upload` call: the file name, the destinations as (id, name), and the current destination id.
pub type BeforeUploadCall = (String, Vec<(String, String)>, String);

impl crate::Prompt for FakePrompt {
    fn ask_text(&self, _title: &str, default: &str) -> BoxFuture<'_, Result<String, PortError>> {
        let v = self.texts.lock().unwrap().pop_front().unwrap_or_else(|| default.to_string());
        Box::pin(async move { Ok(v) })
    }
    fn choose(&self, title: &str, options: &[String]) -> BoxFuture<'_, Result<usize, PortError>> {
        self.chooses.lock().unwrap().push((title.to_string(), options.to_vec()));
        let v = self.choices.lock().unwrap().pop_front().unwrap_or(0);
        Box::pin(async move { Ok(v) })
    }
    fn show_text(&self, title: &str, text: &str) -> BoxFuture<'_, Result<(), PortError>> {
        self.shown.lock().unwrap().push((title.to_string(), text.to_string()));
        Box::pin(async { Ok(()) })
    }
    fn confirm(&self, _title: &str, _text: &str) -> BoxFuture<'_, Result<bool, PortError>> {
        let v = self.confirms.lock().unwrap().pop_front().unwrap_or(true);
        Box::pin(async move { Ok(v) })
    }
    fn pick_files(&self, _title: &str) -> BoxFuture<'_, Result<Vec<std::path::PathBuf>, PortError>> {
        let next = self.files.lock().unwrap().pop_front();
        Box::pin(async move { next.unwrap_or_else(|| Err(PortError::Unsupported("file picker".into()))) })
    }
    fn pick_folder(&self, _title: &str) -> BoxFuture<'_, Result<std::path::PathBuf, PortError>> {
        let next = self.folders.lock().unwrap().pop_front();
        Box::pin(async move { next.unwrap_or_else(|| Err(PortError::Unsupported("folder picker".into()))) })
    }
    fn after_capture(&self, image: &Bitmap, after_capture: u32, after_upload: u32) -> BoxFuture<'_, Result<(u32, u32), PortError>> {
        self.window_calls.lock().unwrap().push((image.width(), image.height(), after_capture, after_upload));
        let next = self.windows.lock().unwrap().pop_front().unwrap_or(Ok((after_capture, after_upload)));
        Box::pin(async move { next })
    }
    fn before_upload(&self, file_name: &str, destinations: &[(String, String)], current: &str) -> BoxFuture<'_, Result<String, PortError>> {
        self.destination_calls.lock().unwrap().push((file_name.to_string(), destinations.to_vec(), current.to_string()));
        let next = self.destinations.lock().unwrap().pop_front().unwrap_or_else(|| Ok(current.to_string()));
        Box::pin(async move { next })
    }
    fn save_path(&self, suggested: &std::path::Path) -> BoxFuture<'_, Result<std::path::PathBuf, PortError>> {
        self.suggested.lock().unwrap().push(suggested.to_path_buf());
        let next = self.save_paths.lock().unwrap().pop_front().unwrap_or_else(|| Err(PortError::Unsupported("save dialog".into())));
        Box::pin(async move { next })
    }
}

#[derive(Debug, Default)]
pub struct FakeSecretStore {
    pub secrets: Mutex<std::collections::HashMap<String, String>>,
}

impl crate::SecretStore for FakeSecretStore {
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        Ok(self.secrets.lock().unwrap().get(key).cloned())
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        self.secrets.lock().unwrap().insert(key.to_string(), secret.to_string());
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), PortError> {
        self.secrets.lock().unwrap().remove(key);
        Ok(())
    }
}

/// A destination that returns a scripted sequence of results and records requests.
pub struct FakeDestination {
    pub id: String,
    pub results: Mutex<std::collections::VecDeque<Result<crate::UploadResult, crate::UploadError>>>,
    pub requests: Mutex<Vec<crate::UploadRequest>>,
}

impl std::fmt::Debug for FakeDestination {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeDestination").field("id", &self.id).finish_non_exhaustive()
    }
}

impl FakeDestination {
    pub fn new(id: &str, results: Vec<Result<crate::UploadResult, crate::UploadError>>) -> Self {
        FakeDestination { id: id.to_string(), results: Mutex::new(results.into()), requests: Mutex::new(Vec::new()) }
    }
}

impl crate::Destination for FakeDestination {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.id
    }
    fn upload(
        &self,
        request: crate::UploadRequest,
        _progress: crate::ProgressCallback,
    ) -> BoxFuture<'_, Result<crate::UploadResult, crate::UploadError>> {
        self.requests.lock().unwrap().push(request);
        let r = self.results.lock().unwrap().pop_front().unwrap_or_else(|| Err(crate::UploadError::Network("no scripted result".into())));
        Box::pin(async move { r })
    }
}

/// Appends "/s" to the URL.
#[derive(Debug, Default)]
pub struct FakeShortener;

impl crate::Shortener for FakeShortener {
    fn id(&self) -> &str {
        "fake"
    }
    fn shorten(&self, url: &str) -> BoxFuture<'_, Result<String, crate::UploadError>> {
        let s = format!("{url}/s");
        Box::pin(async move { Ok(s) })
    }
}

/// Holds fake destinations and shorteners by id; downloads return a fixed payload.
#[derive(Default)]
pub struct FakeUploaders {
    pub destinations: Mutex<Vec<Arc<dyn crate::Destination>>>,
    pub shorteners: Mutex<Vec<Arc<dyn crate::Shortener>>>,
    pub download: Mutex<Option<crate::upload::Downloaded>>,
}

impl std::fmt::Debug for FakeUploaders {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeUploaders").finish_non_exhaustive()
    }
}

impl crate::Uploaders for FakeUploaders {
    fn destination(&self, id: &str) -> Option<Arc<dyn crate::Destination>> {
        self.destinations.lock().unwrap().iter().find(|d| d.id() == id).cloned()
    }
    fn shortener(&self, id: &str) -> Option<Arc<dyn crate::Shortener>> {
        self.shorteners.lock().unwrap().iter().find(|d| d.id() == id).cloned()
    }
    fn sharer(&self, _id: &str) -> Option<Arc<dyn crate::Sharer>> {
        None
    }
    fn destination_list(&self) -> Vec<(String, String)> {
        self.destinations.lock().unwrap().iter().map(|d| (d.id().to_string(), d.name().to_string())).collect()
    }
    fn download(&self, url: &str) -> BoxFuture<'_, Result<crate::upload::Downloaded, crate::UploadError>> {
        let r = self.download.lock().unwrap().clone().ok_or_else(|| crate::UploadError::Network(format!("no fake download for {url}")));
        Box::pin(async move { r })
    }
}

/// Frames of a fixed colour, `count` of them.
#[derive(Debug)]
pub struct FakeFrames {
    pub size: (u32, u32),
    pub left: u32,
}

impl crate::FrameSource for FakeFrames {
    fn size(&self) -> (u32, u32) {
        self.size
    }
    fn fps(&self) -> u32 {
        30
    }
    fn next_frame(&mut self) -> Result<Option<Bitmap>, PortError> {
        if self.left == 0 {
            return Ok(None);
        }
        self.left -= 1;
        let (w, h) = self.size;
        Ok(Some(Bitmap::new(w, h, laqta_domain::PixelFormat::Bgra8, vec![128; (w * h * 4) as usize].into()).unwrap()))
    }
    fn stop(&mut self) {
        self.left = 0;
    }
}

/// Opens `FakeFrames` (or fails), and lists fixed devices.
#[derive(Debug, Default)]
pub struct FakeScreenStream {
    pub fail: Mutex<Option<PortError>>,
    pub opened: Mutex<Vec<laqta_domain::geometry::PhysRect>>,
}

impl crate::ScreenStream for FakeScreenStream {
    fn open(&self, area: laqta_domain::geometry::PhysRect, _show_cursor: bool, _fps: u32, _audio: &[laqta_domain::record::AudioDevice]) -> BoxFuture<'_, Result<crate::RecordingInput, PortError>> {
        self.opened.lock().unwrap().push(area);
        let r = match self.fail.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(crate::RecordingInput { video: crate::VideoSource::Frames(Box::new(FakeFrames { size: (area.width, area.height), left: 3 })), audio: Vec::new() }),
        };
        Box::pin(async move { r })
    }
    fn audio_devices(&self) -> BoxFuture<'_, Result<Vec<laqta_domain::record::AudioDevice>, PortError>> {
        Box::pin(async { Ok(vec![laqta_domain::record::AudioDevice { id: "mic".into(), name: "Microphone".into(), loopback: false }]) })
    }
}

/// Records calls; `stop` writes a tiny file at the output path (via the returned path only — no I/O)
/// and returns it. `crash_after_start` makes `is_alive` false.
#[derive(Debug, Default)]
pub struct FakeMedia {
    pub calls: Arc<Mutex<Vec<String>>>,
    pub crash: Mutex<bool>,
}

#[derive(Debug)]
struct FakeHandle {
    calls: Arc<Mutex<Vec<String>>>,
    output: PathBuf,
    crashed: bool,
}

impl crate::RecordingHandle for FakeHandle {
    fn pause(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        self.calls.lock().unwrap().push("pause".into());
        Box::pin(async { Ok(()) })
    }
    fn resume(&mut self) -> BoxFuture<'_, Result<(), PortError>> {
        self.calls.lock().unwrap().push("resume".into());
        Box::pin(async { Ok(()) })
    }
    fn stop(self: Box<Self>) -> BoxFuture<'static, Result<PathBuf, PortError>> {
        self.calls.lock().unwrap().push("stop".into());
        let out = self.output.clone();
        Box::pin(async move { Ok(out) })
    }
    fn abort(self: Box<Self>) -> BoxFuture<'static, Result<(), PortError>> {
        self.calls.lock().unwrap().push("abort".into());
        Box::pin(async { Ok(()) })
    }
    fn is_alive(&mut self) -> bool {
        !self.crashed
    }
}

impl crate::Media for FakeMedia {
    fn start(&self, _input: crate::RecordingInput, spec: laqta_domain::record::RecordSpec, output: PathBuf) -> BoxFuture<'_, Result<Box<dyn crate::RecordingHandle>, PortError>> {
        self.calls.lock().unwrap().push(format!("start {:?} {}x{}", spec.codec, spec.area.width, spec.area.height));
        let h: Box<dyn crate::RecordingHandle> = Box::new(FakeHandle { calls: self.calls.clone(), output, crashed: *self.crash.lock().unwrap() });
        Box::pin(async move { Ok(h) })
    }
    fn transcode(&self, input: &Path, output: &Path, _spec: crate::TranscodeSpec) -> BoxFuture<'_, Result<(), PortError>> {
        self.calls.lock().unwrap().push(format!("transcode {} {}", input.display(), output.display()));
        Box::pin(async { Ok(()) })
    }
    fn probe(&self, _input: &Path) -> BoxFuture<'_, Result<crate::MediaInfo, PortError>> {
        Box::pin(async { Ok(crate::MediaInfo { duration_secs: 3.0, width: 640, height: 480, fps: 30.0, video_codec: "h264".into(), audio_codec: None, size_bytes: 1000 }) })
    }
}

/// Returns the image it was given (or a fixed one), counting calls.
#[derive(Debug, Default)]
pub struct FakeEditorHost {
    pub calls: Mutex<u32>,
    pub result: Mutex<Option<Result<Bitmap, PortError>>>,
}

impl crate::EditorHost for FakeEditorHost {
    fn edit(&self, image: Bitmap) -> BoxFuture<'_, Result<Bitmap, PortError>> {
        *self.calls.lock().unwrap() += 1;
        let r = self.result.lock().unwrap().clone().unwrap_or(Ok(image));
        Box::pin(async move { r })
    }
}

/// Returns a fixed result.
#[derive(Debug, Default)]
pub struct FakeOcr {
    pub result: Mutex<crate::OcrResult>,
}

impl crate::Ocr for FakeOcr {
    fn recognize(&self, _image: Bitmap, _language: String) -> BoxFuture<'_, Result<crate::OcrResult, PortError>> {
        let r = self.result.lock().unwrap().clone();
        Box::pin(async move { Ok(r) })
    }
    fn languages(&self) -> Vec<String> {
        vec!["en".into()]
    }
}

/// Records window operations.
#[derive(Debug, Default)]
pub struct FakeWindowOps {
    pub calls: Mutex<Vec<String>>,
}

impl crate::WindowOps for FakeWindowOps {
    fn set_borderless(&self, window: &HoverTarget, borderless: bool) -> Result<(), PortError> {
        self.calls.lock().unwrap().push(format!("borderless {} {borderless}", window.title));
        Ok(())
    }
    fn set_topmost(&self, window: &HoverTarget, topmost: bool) -> Result<(), PortError> {
        self.calls.lock().unwrap().push(format!("topmost {} {topmost}", window.title));
        Ok(())
    }
    fn inspect(&self, window: &HoverTarget) -> Result<Vec<(String, String)>, PortError> {
        Ok(vec![("Title".into(), window.title.clone())])
    }
}

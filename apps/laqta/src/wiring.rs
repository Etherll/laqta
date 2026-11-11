//! Builds the ports for this platform and session, loads settings and history, starts every entry
//! point, and runs the UI loop (ARCHITECTURE §2.5).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use laqta_app::{App, Dispatched, Ports, UploadSource};
use laqta_domain::settings::SETTINGS_VERSION;
use laqta_domain::{Action, KeyChord, Settings, WorkflowId};
use laqta_library::{History, SettingsStore};
use laqta_ports::{
    ActionCallback, Clipboard, Clock, HotkeyCallback, HotkeyRegistrar, HoverSource, PortError, ScreenSource, Shell, Spawner, Tray, TrayMenu,
};
use laqta_ui::{UiHandle, UiServices};

use crate::spawner::{AppSpawner, SystemClock};
use crate::stubs;

struct Platform {
    screen: Arc<dyn ScreenSource>,
    clipboard: Arc<dyn Clipboard>,
    shell: Arc<dyn Shell>,
    hover: Arc<dyn HoverSource>,
    hotkeys: Arc<dyn HotkeyRegistrar>,
    tray: Arc<dyn Tray>,
    secrets: Arc<dyn laqta_ports::SecretStore>,
    stream: Arc<dyn laqta_ports::ScreenStream>,
    window_ops: Arc<dyn laqta_ports::WindowOps>,
    autostart: Arc<dyn laqta_ports::Autostart>,
    integration: Arc<dyn laqta_ports::ShellIntegration>,
    input: Arc<dyn laqta_ports::InputSynth>,
    /// The OS OCR engine, where there is one (Windows).
    os_ocr: Option<Arc<dyn laqta_ports::Ocr>>,
    highlighter: Arc<dyn laqta_ports::PointerHighlighter>,
    native_recorder: Option<Arc<dyn laqta_ports::Media>>,
}

/// Where the Wayland ScreenCast restore token is kept, so the portal asks for permission once,
/// not on every launch (ADR 019).
#[cfg(target_os = "linux")]
fn screencast_token_path(data: &std::path::Path) -> std::path::PathBuf {
    data.join("screencast-token")
}

#[cfg(target_os = "linux")]
fn platform(data: &std::path::Path) -> Result<Platform, String> {
    use laqta_platform_linux::Session;
    let session = Session::detect().ok_or("no display: neither WAYLAND_DISPLAY nor DISPLAY is set")?;
    tracing::info!("session: {session:?}");
    // The portals need an app id before the first portal call (KDE's shortcuts refuse a session
    // without one).
    if session == Session::Wayland
        && let Ok(exe) = std::env::current_exe()
    {
        laqta_platform_linux::app_id::register(&exe);
    }
    let path = screencast_token_path(data);
    let token = std::fs::read_to_string(&path).ok().map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
    let save: Arc<dyn Fn(String) + Send + Sync> = Arc::new(move |t: String| {
        if let Err(e) = write_private(&path, &t) {
            tracing::warn!("screencast token not saved: {e}");
        }
    });
    let p = laqta_platform_linux::ports_with_token(session, token, save);
    Ok(Platform {
        screen: p.screen,
        clipboard: p.clipboard,
        shell: p.shell,
        hover: p.hover,
        hotkeys: p.hotkeys,
        tray: p.tray,
        secrets: p.secrets,
        stream: p.stream,
        window_ops: p.window_ops,
        autostart: p.autostart,
        integration: p.integration,
        input: p.input,
        os_ocr: None,
        highlighter: p.highlighter,
        native_recorder: None,
    })
}

/// Writes a file only this user can read (the token grants screen access without asking).
#[cfg(target_os = "linux")]
fn write_private(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    use std::io::Write as _;
    use std::os::unix::fs::OpenOptionsExt as _;
    let tmp = path.with_extension("tmp");
    let mut file = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
    file.write_all(text.as_bytes())?;
    std::fs::rename(&tmp, path)
}

#[cfg(windows)]
fn platform(_data: &std::path::Path) -> Result<Platform, String> {
    laqta_platform_windows::set_dpi_awareness();
    let p = laqta_platform_windows::ports();
    Ok(Platform {
        screen: p.screen,
        clipboard: p.clipboard,
        shell: p.shell,
        hover: p.hover,
        hotkeys: p.hotkeys,
        tray: p.tray,
        secrets: p.secrets,
        stream: p.stream,
        window_ops: p.window_ops,
        autostart: p.autostart,
        integration: p.integration,
        input: p.input,
        os_ocr: Some(p.ocr),
        highlighter: p.highlighter,
        native_recorder: Some(p.native_recorder),
    })
}

#[cfg(target_os = "macos")]
fn platform(_data: &std::path::Path) -> Result<Platform, String> {
    let p = laqta_platform_macos::ports();
    Ok(Platform {
        screen: p.screen,
        clipboard: p.clipboard,
        shell: p.shell,
        hover: p.hover,
        hotkeys: p.hotkeys,
        tray: p.tray,
        secrets: p.secrets,
        stream: p.stream,
        window_ops: p.window_ops,
        autostart: p.autostart,
        integration: p.integration,
        input: p.input,
        os_ocr: Some(p.ocr),
        highlighter: p.highlighter,
        native_recorder: None,
    })
}

#[cfg(not(any(target_os = "linux", windows, target_os = "macos")))]
fn platform(_data: &std::path::Path) -> Result<Platform, String> {
    Err("this platform is not supported (ADR 001)".into())
}

/// First-run settings, with the hotkeys this OS can type.
fn default_settings() -> Settings {
    use laqta_domain::record::Os;
    let os = if cfg!(windows) {
        Os::Windows
    } else if cfg!(target_os = "macos") {
        Os::MacOs
    } else {
        Os::Linux
    };
    Settings::default_for_os(pictures_dir(), os)
}

fn pictures_dir() -> std::path::PathBuf {
    dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))).unwrap_or_else(|| "Pictures".into())
}

/// `-sandbox`: a fresh temporary folder, removed on exit. `-portable`: `LaqtaData` next to the
/// binary, as the `portable` marker file or a "portable" executable name does.
fn data_dir(args: &crate::cli::Args) -> std::path::PathBuf {
    let exe = std::env::current_exe().ok();
    let exe_dir = exe.as_deref().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if args.sandbox {
        return std::env::temp_dir().join(format!("laqta-sandbox-{}", std::process::id()));
    }
    let default = dirs::data_local_dir().unwrap_or_else(|| ".".into()).join("Laqta");
    let chosen = match (&exe_dir, args.portable) {
        (Some(dir), true) => dir.join("LaqtaData"),
        _ => laqta_library::data_dir(exe.as_deref(), default.clone()),
    };
    if chosen != default && !writable(&chosen) {
        // A portable copy on read-only media, or in a folder this account can't write.
        eprintln!("laqta: cannot write {}; keeping data in {}", chosen.display(), default.display());
        return default;
    }
    chosen
}

/// Hands a GPUI window's native handle to the Windows or macOS adapter. A platform answers false
/// to a call it does not need.
fn native_window(window: &gpui::Window, what: laqta_ui::NativeWindow) -> bool {
    #[cfg(windows)]
    {
        use laqta_ui::NativeWindow;
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::Win32(win32) = handle.as_raw()
        {
            let hwnd = win32.hwnd.get();
            match what {
                NativeWindow::PlainPopup => laqta_platform_windows::plain_popup(hwnd),
                NativeWindow::BringToFront => laqta_platform_windows::bring_to_front(hwnd),
                NativeWindow::HideForCapture => laqta_platform_windows::hide_for_capture(hwnd),
                NativeWindow::CoverScreen => return false,
            }
            return true;
        }
    }
    #[cfg(target_os = "macos")]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        if what == laqta_ui::NativeWindow::CoverScreen
            && let Ok(handle) = HasWindowHandle::window_handle(window)
            && let RawWindowHandle::AppKit(appkit) = handle.as_raw()
        {
            return laqta_platform_macos::cover_screen(appkit.ns_view);
        }
    }
    let _ = (window, what);
    false
}

/// The platform's mapping from physical to logical desktop pixels, where it has one (macOS).
fn logical_rect() -> Option<laqta_ui::LogicalRectHook> {
    #[cfg(target_os = "macos")]
    {
        Some(Arc::new(laqta_platform_macos::logical_rect))
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// True when `dir` exists or can be made, and a file can be created in it.
fn writable(dir: &std::path::Path) -> bool {
    let probe = dir.join(format!(".laqta-write-{}", std::process::id()));
    let ok = std::fs::create_dir_all(dir).is_ok() && std::fs::write(&probe, b"").is_ok();
    let _ = std::fs::remove_file(&probe);
    ok
}

/// What a dispatch means to the caller of a request: the task it started, or `Ok(None)` to quit.
fn dispatched(result: Result<Dispatched, laqta_ports::AppError>) -> Result<Option<Vec<laqta_ports::TaskId>>, String> {
    match result {
        Ok(Dispatched::Exit) => Ok(None),
        Ok(Dispatched::Task(id)) => Ok(Some(vec![id])),
        Ok(Dispatched::Tasks(ids)) => Ok(Some(ids)),
        Ok(_) | Err(laqta_ports::AppError::Cancelled) => Ok(Some(vec![])),
        Err(e) => Err(e.to_string()),
    }
}

/// Runs one request and returns the tasks it started; `Ok(None)` means quit.
async fn run_request(
    app: &Arc<App>,
    importer: &crate::imports::Importer,
    request: laqta_ipc::Request,
) -> Result<Option<Vec<laqta_ports::TaskId>>, String> {
    use crate::requests::Route;
    match crate::requests::route(request, &app.settings())? {
        Route::Dispatch(action, workflow) => dispatched(app.dispatch(action, workflow).await),
        Route::DispatchFile(action, path, workflow) => {
            dispatched(app.dispatch_with(action, laqta_app::ActionInput::Files(vec![path]), workflow).await)
        }
        // Uploads enter through `dispatch` too (ADR 015), as the action with its input.
        Route::Upload(source, workflow) => {
            use laqta_app::ActionInput;
            let (action, input) = match source {
                UploadSource::File(path) | UploadSource::SavedFile(path) => (Action::FileUpload, ActionInput::Files(vec![path])),
                UploadSource::Text(text) => (Action::UploadText, ActionInput::Text(text)),
                UploadSource::Url(url) => (Action::UploadUrl, ActionInput::Url(url)),
                UploadSource::ShortenUrl(url) => (Action::ShortenUrl, ActionInput::Url(url)),
            };
            dispatched(app.dispatch_with(action, input, workflow).await)
        }
        Route::ImportUploader(path) => importer.uploader(&path).map(|id| {
            tracing::info!(%id, "custom uploader imported");
            Some(vec![])
        }),
        Route::ImportBackup(path) => importer.sharex(&path).map(|summary| {
            tracing::info!(%summary, "ShareX backup imported");
            Some(vec![])
        }),
        Route::ImportEffect(path) => importer.effect(&path).map(|name| {
            tracing::info!(%name, "effect preset imported");
            Some(vec![])
        }),
    }
}

fn load_settings(store: &SettingsStore) -> Settings {
    if !store.path().exists() {
        return default_settings();
    }
    match store.load::<Settings>(SETTINGS_VERSION, &[]) {
        Ok(mut s) => {
            // A file without `screenshots_dir` gets the serde default, a relative path that would put captures in
            // the folder Laqta was started from.
            if s.screenshots_dir == Settings::default().screenshots_dir {
                s.screenshots_dir = default_settings().screenshots_dir;
            } else if s.screenshots_dir.is_relative()
                && let Some(home) = dirs::home_dir()
            {
                s.screenshots_dir = home.join(&s.screenshots_dir);
            }
            s
        }
        Err(e) => {
            tracing::error!("settings could not be loaded ({e}); using defaults");
            default_settings()
        }
    }
}

/// The bindings last registered with the platform. Registration can take seconds (Wayland's portal), so it runs on a
/// thread that may block, and this lets an unchanged save skip it.
type RegisteredBindings = Arc<Mutex<Option<Vec<(KeyChord, Action)>>>>;

/// What the last registration reported, read by the Shortcuts page.
type HotkeyReports = Arc<Mutex<laqta_ui::HotkeyReport>>;

static HOTKEY_REPORT: std::sync::OnceLock<HotkeyReports> = std::sync::OnceLock::new();
/// Set while the settings window records a shortcut: a fired hotkey is ignored then.
static HOTKEYS_HELD: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
/// The UI to tell when the report changes, so the settings window shows the new status at once.
static HOTKEY_WATCHER: std::sync::OnceLock<UiHandle> = std::sync::OnceLock::new();

fn hotkeys_changed() {
    if let Some(ui) = HOTKEY_WATCHER.get() {
        ui.hotkeys_changed();
    }
}

fn hotkey_reports() -> HotkeyReports {
    HOTKEY_REPORT.get_or_init(|| Arc::new(Mutex::new(laqta_ui::HotkeyReport { pending: true, ..Default::default() }))).clone()
}

/// Stores the keys the desktop assigned: each chord in `keys` replaces its old key, and the other chords keep theirs.
fn merge_system_keys(keys: Vec<(KeyChord, String)>) {
    let reports = hotkey_reports();
    let mut report = reports.lock().unwrap_or_else(|e| e.into_inner());
    for (chord, key) in keys {
        match report.system_keys.iter_mut().find(|(known, _)| *known == chord) {
            Some(entry) => entry.1 = key,
            None => report.system_keys.push((chord, key)),
        }
    }
    drop(report);
    hotkeys_changed();
}

/// The desktop turned down chords the last registration reported as registered: they show as failed,
/// and the next sync registers them again.
fn reject_hotkeys(registered: &RegisteredBindings, why: &str) {
    // Not registered after all, so an unchanged save must not skip the next try.
    registered.lock().unwrap_or_else(|e| e.into_inner()).take();
    let reports = hotkey_reports();
    let mut report = reports.lock().unwrap_or_else(|e| e.into_inner());
    let chords = std::mem::take(&mut report.registered);
    report.failed.extend(chords.into_iter().map(|chord| (chord, why.to_owned())));
    report.system_keys.clear();
    drop(report);
    hotkeys_changed();
}

/// Drops every hotkey while a shortcut is being recorded. A partial registration is cached as `None`
/// (so the next save retries it) while its other chords stay grabbed, so the release does not go by
/// the cache.
fn release_for_recording(current: &mut Option<Vec<(KeyChord, Action)>>, hotkeys: &dyn HotkeyRegistrar) {
    current.take();
    hotkeys.unregister_all();
    tracing::info!("hotkeys released while a shortcut is being recorded");
}

/// Makes the platform's hotkeys match the app's current bindings unless they already do. Returns whether the platform
/// refused every chord as unsupported; false when the bindings were already registered. The settings are read under the lock,
/// so the last run to take it leaves the latest bindings registered. While a shortcut is being recorded, a platform that grabs
/// its keys has them released instead, so the settings window sees a bound chord; the next run after the recording ends
/// registers them again.
fn sync_hotkeys(app: &App, hotkeys: &dyn HotkeyRegistrar, registered: &RegisteredBindings, on_fire: HotkeyCallback) -> bool {
    let mut current = registered.lock().unwrap_or_else(|e| e.into_inner());
    if HOTKEYS_HELD.load(std::sync::atomic::Ordering::SeqCst) && hotkeys.grabs_keys() {
        release_for_recording(&mut current, hotkeys);
        return false;
    }
    let bindings = app.settings().hotkey_bindings();
    if current.as_ref() == Some(&bindings) {
        return false;
    }
    hotkeys.unregister_all();
    let results = hotkeys.register(&bindings, on_fire);
    // Only a platform that refuses every chord is "unsupported"; a single refused chord is a failed row.
    let unsupported = !results.is_empty() && results.iter().all(|(_, r)| matches!(r, Err(PortError::Unsupported(_))));
    // A chord that failed, or was unsupported beside registered chords, is retried on the next save.
    let mut retry = false;
    let mut report = laqta_ui::HotkeyReport::default();
    for (chord, result) in results {
        match result {
            Ok(()) => {
                tracing::info!(%chord, "hotkey registered");
                report.registered.push(chord);
            }
            Err(PortError::Unsupported(why)) if unsupported => {
                tracing::warn!(%chord, "global hotkeys unsupported here ({why}); bind `laqta --action <name>` in your desktop's keyboard settings");
                report.unsupported = Some(why);
            }
            Err(e) => {
                retry = true;
                tracing::warn!(%chord, "hotkey registration failed: {e}");
                report.failed.push((chord, e.to_string()));
            }
        }
    }
    // The desktop's keys arrive through `on_assigned`, not this registration, so the new report keeps them.
    // Only chords still bound keep theirs.
    let reports = hotkey_reports();
    let mut shared = reports.lock().unwrap_or_else(|e| e.into_inner());
    report.system_keys = std::mem::take(&mut shared.system_keys);
    report.system_keys.retain(|(chord, _)| report.registered.contains(chord));
    *shared = report;
    drop(shared);
    hotkeys_changed();
    *current = if retry { None } else { Some(bindings) };
    unsupported
}

pub fn run(server: Option<laqta_ipc::Server>, args: crate::cli::Args) -> Result<(), String> {
    let mut startup: Vec<laqta_ipc::Request> = Vec::new();
    // Absolute, as the second instance sends them: the working folder can change before they are read.
    let import = |path: &std::path::PathBuf| laqta_ipc::Request::Upload(std::path::absolute(path).unwrap_or_else(|_| path.clone()));
    startup.extend(args.import_uploaders.iter().map(import));
    startup.extend(args.import_effects.iter().map(import));
    startup.extend(args.requests.iter().cloned());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let spawner = AppSpawner::start(clock.clone()).map_err(|e| format!("cannot start the app worker: {e}"))?;
    // Releases the AI models' sessions after 60 s without use (ARCHITECTURE §1.4).
    let releaser = crate::models::ModelReleaser::new(clock.clone(), spawner.clone());
    let data = data_dir(&args);
    let mut platform = platform(&data)?;
    if args.sandbox {
        platform.secrets = Arc::new(stubs::MemorySecrets::default());
    } else {
        // With no keyring, password and key credentials last for the session (ADR 018).
        platform.secrets = Arc::new(stubs::SessionSecrets::new(platform.secrets.clone()));
    }
    let store = Arc::new(SettingsStore::new(data.clone()));
    let settings = load_settings(&store);
    let history = if settings.history.enabled && !args.sandbox {
        match History::open(&data.join("history.db")) {
            Ok(h) => Some(Arc::new(h)),
            Err(e) => {
                tracing::error!("history unavailable: {e}");
                None
            }
        }
    } else {
        None
    };

    let (ui, ui_rx) = UiHandle::channel();
    let _ = HOTKEY_WATCHER.set(ui.clone());
    let rng = stubs::system_rng();
    let uploaders = crate::uploads::registry(&data, Arc::new(ui.clone()), rng.clone());
    let host_deps = crate::uploads::host_deps(platform.secrets.clone(), platform.shell.clone());
    // Upload accounts read the system keyring (always, for Imgur's token): a D-Bus round trip, or
    // an unlock prompt that never returns on its own. They load in the startup task below, before
    // the startup requests that may upload, never on the main thread.
    let accounts: Arc<Mutex<()>> = Arc::default();
    let cloud_options: crate::cloud::SharedCloudOptions = Arc::new(std::sync::RwLock::new(settings.cloud.clone()));
    let recorder_backend = Arc::new(std::sync::RwLock::new(settings.defaults.recording.backend));
    let model_options: crate::models::SharedModelOptions = Arc::new(std::sync::RwLock::new(settings.models.clone()));
    let ports = Ports {
        screen: platform.screen,
        clipboard: platform.clipboard,
        shell: platform.shell.clone(),
        clock: clock.clone(),
        spawner: spawner.clone(),
        overlay: Arc::new(ui.clone()),
        tools: Arc::new(ui.clone()),
        prompt: Arc::new(ui.clone()),
        editor: Arc::new(ui.clone()),
        media: Arc::new(crate::media::SwitchingMedia {
            ffmpeg: Arc::new(crate::media::LazyMedia::new(
                data.clone(),
                {
                    let s = settings.defaults.recording.ffmpeg_path.clone();
                    Arc::new(move || s.clone())
                },
                Arc::new(ui.clone()),
            )),
            native: platform.native_recorder.clone(),
            stream: platform.stream.clone(),
            backend: recorder_backend.clone(),
        }),
        stream: platform.stream.clone(),
        ocr: Arc::new(crate::ocr::LazyOcr::new(
            data.clone(),
            Arc::new(ui.clone()),
            model_options.clone(),
            platform.os_ocr.clone(),
            releaser.clone(),
        )),
        window_ops: platform.window_ops.clone(),
        input: platform.input.clone(),
        highlighter: platform.highlighter.clone(),
        background: Arc::new(crate::cloud::Background::new(
            cloud_options.clone(),
            Arc::new(crate::local_models::LocalBackground::new(
                data.clone(),
                model_options.clone(),
                Arc::new(ui.clone()),
                releaser.clone(),
            )),
            platform.secrets.clone(),
            spawner.clone(),
            laqta_imaging::Codecs::builtin(),
        )),
        analyzer: Arc::new(crate::cloud::Analyzer {
            options: cloud_options.clone(),
            secrets: platform.secrets.clone(),
            spawner: spawner.clone(),
        }),
        inpainter: Arc::new(crate::local_models::LocalInpainter::new(
            data.clone(),
            model_options.clone(),
            Arc::new(ui.clone()),
            releaser.clone(),
        )),
        watcher: Arc::new(laqta_library::NotifyWatcher::default()),
        uploaders: uploaders.clone(),
        hover: platform.hover,
        notifier: Arc::new(crate::notifier::AppNotifier { ui: ui.clone() }),
        rng,
        host: stubs::host_info(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let app = App::new(ports, settings);
    let (quit_tx, quit_rx) = mpsc::channel::<()>();
    let hotkeys = platform.hotkeys;
    hotkeys.on_assigned(Arc::new(merge_system_keys));
    let registered: RegisteredBindings = Arc::new(Mutex::new(None));
    {
        let registered = registered.clone();
        hotkeys.on_rejected(Arc::new(move |why| reject_hotkeys(&registered, &why)));
    }

    // Task events: the log and the history writer.
    let mut events = app.events();
    spawner.spawn(Box::pin(async move {
        loop {
            match events.recv().await {
                Ok(event) => tracing::info!(?event, "task event"),
                Err(async_broadcast::RecvError::Overflowed(skipped)) => tracing::warn!(skipped, "task events were skipped"),
                Err(async_broadcast::RecvError::Closed) => break,
            }
        }
    }));
    if let Some(h) = &history {
        h.release_when_idle(clock.clone(), spawner.clone());
        app.attach_history(h.clone());
        let settings = app.clone();
        spawner.spawn(Box::pin(laqta_library::record_task_events(h.clone(), app.events(), clock.clone(), move || {
            settings.settings().history.max_items
        })));
    }
    // One font set for the windows and for the text of drawing effects (ADR 024).
    let fonts = crate::fonts::fonts();
    {
        let fonts = fonts.clone();
        app.attach_effect_text(Arc::new(move |spec: &laqta_domain::effects::TextSpec| {
            let font = laqta_editor::FontStyle {
                family: spec.font_family.clone(),
                size: spec.size,
                bold: spec.bold,
                italic: spec.italic,
                color: spec.color,
                padding: 0.0,
                ..laqta_editor::FontStyle::default()
            };
            fonts.render_text(&spec.text, &font)
        }));
    }

    // Every entry point goes through `dispatch` (ADR 015). The tray runs an action with the first enabled
    // workflow bound to it; a hotkey runs the workflow bound to the chord that fired, so that workflow's
    // settings apply.
    let hotkey_slot: Arc<Mutex<Option<HotkeyCallback>>> = Arc::new(Mutex::new(None));
    let run_action: Arc<dyn Fn(Action, Option<WorkflowId>) + Send + Sync> = {
        let spawner = spawner.clone();
        let quit_tx = quit_tx.clone();
        let hotkeys = hotkeys.clone();
        let registered = registered.clone();
        let slot = hotkey_slot.clone();
        let app = app.clone();
        Arc::new(move |action: Action, workflow: Option<WorkflowId>| {
            let app = app.clone();
            let quit_tx = quit_tx.clone();
            let hotkeys = hotkeys.clone();
            let registered = registered.clone();
            let slot = slot.clone();
            let blocking = spawner.clone();
            spawner.spawn(Box::pin(async move {
                match app.dispatch(action, workflow).await {
                    Ok(Dispatched::Exit) => {
                        let _ = quit_tx.send(());
                    }
                    Ok(Dispatched::HotkeysToggled(off)) => {
                        let cb = slot.lock().ok().and_then(|s| s.clone());
                        if let Some(cb) = cb {
                            // The portal round trip can take seconds: keep it off the async worker.
                            let app = app.clone();
                            blocking
                                .run_blocking(Box::new(move || {
                                    sync_hotkeys(&app, hotkeys.as_ref(), &registered, cb);
                                }))
                                .await;
                        }
                        tracing::info!(off, "hotkeys toggled");
                    }
                    Ok(d) => tracing::debug!(?action, ?d, "dispatched"),
                    Err(laqta_ports::AppError::Cancelled) => tracing::debug!(?action, "cancelled"),
                    Err(e) => tracing::warn!(?action, "action failed: {e}"),
                }
            }));
        })
    };
    // The tray runs an action with the first enabled workflow bound to it.
    let dispatch: ActionCallback = {
        let app = app.clone();
        let run_action = run_action.clone();
        Arc::new(move |action: Action| {
            let workflow = app.settings().workflows.iter().find(|w| w.enabled && w.action == action).map(|w| w.id);
            run_action(action, workflow);
        })
    };
    // A hotkey runs the workflow bound to the chord that fired. A chord with no enabled workflow is dropped.
    let hotkey_dispatch: HotkeyCallback = {
        let app = app.clone();
        let run_action = run_action.clone();
        Arc::new(move |chord: KeyChord| {
            if HOTKEYS_HELD.load(std::sync::atomic::Ordering::SeqCst) {
                tracing::debug!(%chord, "hotkey ignored while a shortcut is being recorded");
                return;
            }
            match app.settings().workflow_for_hotkey(chord) {
                Some(workflow) => run_action(workflow.action, Some(workflow.id)),
                None => tracing::debug!(%chord, "no enabled workflow is bound to this hotkey"),
            }
        })
    };
    if let Ok(mut s) = hotkey_slot.lock() {
        *s = Some(hotkey_dispatch.clone());
    }

    // Sandbox mode writes no settings; `-NoHotkeys` registers none.
    let use_hotkeys = !args.no_hotkeys;
    let save_settings: crate::imports::SaveSettings = {
        let model_options = model_options.clone();
        let cloud_options = cloud_options.clone();
        let recorder_backend = recorder_backend.clone();
        let app = app.clone();
        let store = store.clone();
        let clock = clock.clone();
        let hotkeys = hotkeys.clone();
        let registered = registered.clone();
        let accounts = accounts.clone();
        let hotkey_dispatch = hotkey_dispatch.clone();
        let uploaders = uploaders.clone();
        let host_deps = host_deps.clone();
        let spawner = spawner.clone();
        let sandbox = args.sandbox;
        Arc::new(move |s: Settings| {
            if !sandbox {
                store.save(&s, SETTINGS_VERSION, clock.now()).map_err(|e| e.to_string())?;
            }
            laqta_ui::i18n::set_language(s.language.as_deref());
            if let Ok(mut m) = model_options.write() {
                *m = s.models.clone();
            }
            if let Ok(mut c) = cloud_options.write() {
                *c = s.cloud.clone();
            }
            if let Ok(mut b) = recorder_backend.write() {
                *b = s.defaults.recording.backend;
            }
            app.set_settings(s.clone());
            // Keyring reads and the hotkey portal can each take seconds; the caller (often the UI
            // thread) returns at once. Hotkeys re-register only when the bindings changed.
            let (app_b, hotkeys, registered, hotkey_dispatch) = (app.clone(), hotkeys.clone(), registered.clone(), hotkey_dispatch.clone());
            let (uploaders, host_deps, accounts) = (uploaders.clone(), host_deps.clone(), accounts.clone());
            let blocking = spawner.clone();
            spawner.spawn(Box::pin(async move {
                blocking
                    .run_blocking(Box::new(move || {
                        // Serialized, and from the latest settings, so two quick saves can't leave the older accounts.
                        {
                            let _one_at_a_time = accounts.lock().unwrap_or_else(|e| e.into_inner());
                            crate::uploads::apply_accounts(&uploaders, &app_b.settings().hosts, &host_deps);
                        }
                        if use_hotkeys {
                            sync_hotkeys(&app_b, hotkeys.as_ref(), &registered, hotkey_dispatch);
                        }
                        // Starting a watch lists the folder's files (all its subfolders' too), so it
                        // stays off the thread that saved the settings.
                        app_b.start_watch_folders();
                    }))
                    .await;
            }));
            Ok(())
        })
    };
    let importer = crate::imports::Importer {
        app: app.clone(),
        save: save_settings.clone(),
        uploaders: uploaders.clone(),
        uploaders_dir: data.join("uploaders"),
        secrets: platform.secrets.clone(),
        history: history.clone(),
    };

    // IPC: secondary instances and the native-messaging host; replies after the request has run.
    if let Some(server) = server {
        let app = app.clone();
        let quit_tx = quit_tx.clone();
        let importer = importer.clone();
        server.serve_requests(move |request| match futures_lite::future::block_on(run_request(&app, &importer, request)) {
            Ok(None) => {
                let _ = quit_tx.send(());
                Ok(())
            }
            Ok(Some(_)) => Ok(()),
            Err(e) => Err(e),
        });
    }

    let settings = app.settings();
    let language = laqta_ui::i18n::set_language(settings.language.as_deref());
    tracing::info!(language, "UI language");
    let mut menu: TrayMenu = laqta_ui::presenters::tray::tray_menu(&settings);
    // Hotkey registration can wait seconds on Wayland's portal, and the tray tooltip depends on its
    // result, so both run on a startup thread while the main thread goes on to the UI.
    let start_shell = {
        let (app, hotkeys, registered, hotkey_dispatch, dispatch, tray) =
            (app.clone(), hotkeys.clone(), registered.clone(), hotkey_dispatch.clone(), dispatch.clone(), platform.tray.clone());
        move || {
            if use_hotkeys && sync_hotkeys(&app, hotkeys.as_ref(), &registered, hotkey_dispatch.clone()) {
                menu.tooltip = laqta_ui::presenters::tray::TOOLTIP_NO_HOTKEY.to_string();
            }
            // With no tray icon the main window is the only way in, so it opens even on a silent
            // start. On Windows the icon is added once Explorer is back.
            match tray.show(menu, dispatch.clone()) {
                Ok(()) => tracing::info!("tray icon shown"),
                Err(e) => {
                    tracing::warn!("no tray icon ({e}); opening the main window");
                    dispatch(Action::OpenMainWindow);
                }
            }
        }
    };
    let start_shell = Arc::new(Mutex::new(Some(start_shell)));
    let on_thread = start_shell.clone();
    let spawned = std::thread::Builder::new().name("laqta-startup".into()).spawn(move || {
        if let Some(start) = on_thread.lock().ok().and_then(|mut s| s.take()) {
            start();
        }
    });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the startup thread ({e}); registering hotkeys inline");
        if let Some(start) = start_shell.lock().ok().and_then(|mut s| s.take()) {
            start();
        }
    }

    app.start_watch_folders();

    // Startup requests run in order. `-autoclose` quits once they and the tasks they started are done.
    // Without `-silent`, a plain start opens the main window.
    if startup.is_empty() && !args.silent && !args.autoclose {
        dispatch(Action::OpenMainWindow);
    }
    {
        let app = app.clone();
        let importer = importer.clone();
        let quit_tx = quit_tx.clone();
        let autoclose = args.autoclose;
        let mut events = app.events();
        let (uploaders, host_deps, accounts, blocking) = (uploaders.clone(), host_deps.clone(), accounts.clone(), spawner.clone());
        spawner.spawn(Box::pin(async move {
            let settings_app = app.clone();
            blocking
                .run_blocking(Box::new(move || {
                    let _one_at_a_time = accounts.lock().unwrap_or_else(|e| e.into_inner());
                    crate::uploads::apply_accounts(&uploaders, &settings_app.settings().hosts, &host_deps);
                }))
                .await;
            let mut pending: std::collections::HashSet<laqta_ports::TaskId> = Default::default();
            for request in startup {
                match run_request(&app, &importer, request).await {
                    Ok(None) => {
                        let _ = quit_tx.send(());
                        return;
                    }
                    Ok(Some(ids)) => pending.extend(ids),
                    Err(e) => tracing::warn!("startup request failed: {e}"),
                }
            }
            if !autoclose {
                return;
            }
            // A recording's file reaches Task as a new task when it stops, so new tasks join the wait.
            while !pending.is_empty() || app.is_recording() {
                match events.recv().await {
                    Ok(laqta_ports::TaskEvent::StatusChanged { id, status }) if status.is_settled() => {
                        pending.remove(&id);
                    }
                    Ok(laqta_ports::TaskEvent::StatusChanged { id, status: laqta_ports::TaskStatus::InQueue }) => {
                        pending.insert(id);
                    }
                    Ok(_) => {}
                    // Events were dropped, so `pending` may hold tasks that ended or miss new ones:
                    // take the list from Task itself.
                    Err(async_broadcast::RecvError::Overflowed(_)) => pending = app.unfinished_tasks().into_iter().collect(),
                    Err(async_broadcast::RecvError::Closed) => break,
                }
            }
            let _ = quit_tx.send(());
        }));
    }

    if std::env::var_os("LAQTA_NO_GPUI").is_some_and(|v| v == "1") {
        drop(ui_rx);
        let _ = quit_rx.recv();
    } else {
        // Quit from any source (tray, IPC, hotkey) reaches the GPUI loop.
        let ui_quit = ui.clone();
        std::thread::Builder::new()
            .name("laqta-quit".into())
            .spawn(move || {
                let _ = quit_rx.recv();
                ui_quit.quit();
            })
            .map_err(|e| e.to_string())?;
        let last_release: Arc<Mutex<Option<laqta_update::Release>>> = Arc::new(Mutex::new(None));
        let quit_ui = quit_tx.clone();
        let services = UiServices {
            hotkey_report: Arc::new(|| hotkey_reports().lock().unwrap_or_else(|e| e.into_inner()).clone()),
            default_settings: Arc::new(default_settings),
            configure_shortcuts: cfg!(target_os = "linux").then(|| {
                let hotkeys = hotkeys.clone();
                Arc::new(move || {
                    if let Err(e) = hotkeys.configure() {
                        tracing::warn!("cannot open the desktop's shortcut settings: {e}");
                    }
                }) as Arc<dyn Fn() + Send + Sync>
            }),
            free_print_screen: {
                #[cfg(windows)]
                let free: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>> =
                    Some(Arc::new(|| laqta_platform_windows::set_print_screen_for_snipping(false).map_err(|e| e.to_string())));
                #[cfg(not(windows))]
                let free: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>> = None;
                free
            },
            hold_hotkeys: {
                let (app, hotkeys, registered, hotkey_dispatch, spawner) =
                    (app.clone(), hotkeys.clone(), registered.clone(), hotkey_dispatch.clone(), spawner.clone());
                Arc::new(move |held| {
                    if HOTKEYS_HELD.swap(held, std::sync::atomic::Ordering::SeqCst) == held || !use_hotkeys || !hotkeys.grabs_keys() {
                        return;
                    }
                    // Releases the grabs for the recording, or registers them again after it. Each run reads the
                    // flag under the lock, so the last one leaves the right state.
                    let (app, hotkeys, registered, hotkey_dispatch) =
                        (app.clone(), hotkeys.clone(), registered.clone(), hotkey_dispatch.clone());
                    let blocking = spawner.clone();
                    spawner.spawn(Box::pin(async move {
                        blocking
                            .run_blocking(Box::new(move || {
                                sync_hotkeys(&app, hotkeys.as_ref(), &registered, hotkey_dispatch);
                            }))
                            .await;
                    }));
                })
            },
            record_keys: cfg!(windows).then(|| {
                let hotkeys = hotkeys.clone();
                Arc::new(move |on_chord: laqta_ports::ChordCallback| match hotkeys.record_keys(on_chord) {
                    Ok(recording) => Some(recording),
                    Err(e) => {
                        tracing::warn!("cannot record special keys: {e}");
                        None
                    }
                }) as Arc<dyn Fn(laqta_ports::ChordCallback) -> Option<laqta_ports::KeyRecording> + Send + Sync>
            }),
            app: app.clone(),
            history: history.clone(),
            shell: platform.shell.clone(),
            clock: clock.clone(),
            host: stubs::host_info(),
            dispatch: dispatch.clone(),
            save_settings,
            data_dir: data.clone(),
            uploaders: uploaders.clone(),
            secrets: platform.secrets.clone(),
            prompt: Arc::new(ui.clone()),
            native_window: Arc::new(native_window),
            logical_rect: logical_rect(),
            fonts: fonts.clone(),
            spawner: spawner.clone(),
            codecs: laqta_imaging::Codecs::builtin(),
            import_sharex: {
                let importer = importer.clone();
                Arc::new(move |path: std::path::PathBuf| importer.sharex(&path))
            },
            remote_storages: {
                let app = app.clone();
                let deps = host_deps.clone();
                Arc::new(move || laqta_upload::remote_storages(&app.settings().hosts, &deps))
            },
            models: Arc::new(crate::models::Models { data: data.clone(), options: model_options.clone() }),
            check_update: {
                let last = last_release.clone();
                Arc::new(move || {
                    let last = last.clone();
                    Box::pin(async move {
                        let found = laqta_http::run(crate::updates::check()).await?;
                        let msg = crate::updates::describe(&found);
                        let installable = found.is_some() && crate::updates::update_exe().is_some();
                        if let Ok(mut l) = last.lock() {
                            *l = found;
                        }
                        Ok((msg, installable))
                    })
                })
            },
            install_update: {
                let last = last_release.clone();
                let data = data.clone();
                let quit = quit_ui.clone();
                Arc::new(move || {
                    let release = last.lock().ok().and_then(|l| l.clone());
                    let data = data.clone();
                    let quit = quit.clone();
                    Box::pin(async move {
                        let release = release.ok_or("check for updates first")?;
                        laqta_http::run(crate::updates::install(release, data)).await?;
                        let _ = quit.send(());
                        Ok(())
                    })
                })
            },
            sxie_read: Arc::new(|bytes: &[u8]| laqta_import::parse_sxie(bytes).map_err(|e| e.to_string())),
            sxie_write: Arc::new(|preset: &laqta_domain::effects::Preset| laqta_import::write_sxie(preset).map_err(|e| e.to_string())),
            autostart: platform.autostart.clone(),
            integration: platform.integration.clone(),
            add_custom_uploader: {
                let uploaders = uploaders.clone();
                let dir = data.join("uploaders");
                Arc::new(move |json: String| crate::uploads::add_custom(&uploaders, &dir, &json))
            },
        };
        laqta_ui::run(services, ui_rx);
    }
    tracing::info!("Laqta exiting");
    if args.sandbox {
        let _ = std::fs::remove_dir_all(&data);
    }
    #[cfg(windows)]
    laqta_platform_windows::shutdown(); // removes the tray icon, so Explorer leaves no ghost
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use laqta_ports::{HotkeyCallback, HotkeyRegistrar, PortError};

    use super::*;

    #[derive(Default)]
    struct Grabbing {
        released: AtomicUsize,
    }

    impl HotkeyRegistrar for Grabbing {
        fn register(&self, _: &[(KeyChord, Action)], _: HotkeyCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
            Vec::new()
        }
        fn unregister_all(&self) {
            self.released.fetch_add(1, Ordering::SeqCst);
        }
        fn grabs_keys(&self) -> bool {
            true
        }
    }

    /// A bind the desktop turned down after `register` said yes: the chords show as failed, and the
    /// cache is cleared so the next save registers them again.
    #[test]
    fn a_rejected_bind_shows_the_chords_as_failed() {
        let chord: KeyChord = "Ctrl+Shift+F9".parse().expect("a chord");
        let registered: RegisteredBindings = Arc::new(Mutex::new(Some(vec![(chord, Action::PrintScreen)])));
        *hotkey_reports().lock().unwrap() = laqta_ui::HotkeyReport { registered: vec![chord], ..Default::default() };

        reject_hotkeys(&registered, "cancelled");

        assert!(registered.lock().unwrap().is_none(), "the next sync registers again");
        let report = hotkey_reports().lock().unwrap().clone();
        assert!(report.registered.is_empty());
        assert_eq!(report.failed, vec![(chord, "cancelled".to_owned())]);
    }

    /// A partial registration is cached as `None`, yet its other chords are still grabbed: recording
    /// releases them all the same.
    #[test]
    fn recording_releases_a_partial_registration() {
        let hotkeys = Grabbing::default();
        let mut current = None;
        release_for_recording(&mut current, &hotkeys);
        assert_eq!(hotkeys.released.load(Ordering::SeqCst), 1);
        assert!(current.is_none());
    }
}

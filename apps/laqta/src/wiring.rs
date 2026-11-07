//! Builds the ports for this platform and session, loads settings and history, starts every entry
//! point, and runs the UI loop (ARCHITECTURE §2.5).

use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use laqta_app::{App, Dispatched, Ports};
use laqta_domain::settings::SETTINGS_VERSION;
use laqta_domain::{Action, Settings};
use laqta_library::{History, SettingsStore};
use laqta_ports::{
    ActionCallback, Clipboard, Clock, HotkeyRegistrar, HoverSource, PortError, ScreenSource, Shell, Spawner, Tray, TrayMenu,
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
}

#[cfg(target_os = "linux")]
fn platform() -> Result<Platform, String> {
    use laqta_platform_linux::Session;
    let session = Session::detect().ok_or("no display: neither WAYLAND_DISPLAY nor DISPLAY is set")?;
    tracing::info!("session: {session:?}");
    let p = laqta_platform_linux::ports(session);
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
    })
}

#[cfg(windows)]
fn platform() -> Result<Platform, String> {
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
    })
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform() -> Result<Platform, String> {
    Err("this platform is not supported (ADR 001)".into())
}

fn pictures_dir() -> std::path::PathBuf {
    dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))).unwrap_or_else(|| "Pictures".into())
}

/// `-sandbox` (M6 D8): a fresh temporary folder, removed on exit. `-portable`: `LaqtaData` next to the
/// binary, as the `portable` marker file does.
fn data_dir(args: &crate::cli::Args) -> std::path::PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    if args.sandbox {
        return std::env::temp_dir().join(format!("laqta-sandbox-{}", std::process::id()));
    }
    if args.portable
        && let Some(dir) = &exe_dir
    {
        return dir.join("LaqtaData");
    }
    let default = dirs::data_local_dir().unwrap_or_else(|| ".".into()).join("Laqta");
    laqta_library::data_dir(exe_dir.as_deref(), default)
}

/// Runs one request and returns the tasks it started; `Ok(None)` means quit.
async fn run_request(
    app: &Arc<App>,
    importer: &crate::imports::Importer,
    request: laqta_ipc::Request,
) -> Result<Option<Vec<laqta_ports::TaskId>>, String> {
    use crate::requests::Route;
    match crate::requests::route(request, &app.settings())? {
        Route::Dispatch(action, workflow) => match app.dispatch(action, workflow).await {
            Ok(Dispatched::Exit) => Ok(None),
            Ok(Dispatched::Task(id)) => Ok(Some(vec![id])),
            Ok(_) | Err(laqta_ports::AppError::Cancelled) => Ok(Some(vec![])),
            Err(e) => Err(e.to_string()),
        },
        Route::Upload(source, workflow) => Ok(Some(vec![app.upload_source(source, workflow)])),
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
        return Settings::default_for(pictures_dir());
    }
    match store.load::<Settings>(SETTINGS_VERSION, &[]) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("settings could not be loaded ({e}); using defaults");
            Settings::default_for(pictures_dir())
        }
    }
}

/// Registers the enabled workflows' hotkeys and adjusts the tray tooltip when they're unsupported.
fn register_hotkeys(hotkeys: &dyn HotkeyRegistrar, settings: &Settings, on_fire: ActionCallback) -> bool {
    hotkeys.unregister_all();
    let mut unsupported = false;
    for (chord, result) in hotkeys.register(&settings.hotkey_bindings(), on_fire) {
        match result {
            Ok(()) => tracing::info!(%chord, "hotkey registered"),
            Err(PortError::Unsupported(why)) => {
                unsupported = true;
                tracing::warn!(%chord, "global hotkeys unsupported here ({why}); bind `laqta --action <name>` in your desktop's keyboard settings");
            }
            Err(e) => tracing::warn!(%chord, "hotkey registration failed: {e}"),
        }
    }
    unsupported
}

pub fn run(server: Option<laqta_ipc::Server>, args: crate::cli::Args) -> Result<(), String> {
    let mut startup: Vec<laqta_ipc::Request> = Vec::new();
    startup.extend(args.import_uploaders.iter().cloned().map(laqta_ipc::Request::Upload));
    startup.extend(args.import_effects.iter().cloned().map(laqta_ipc::Request::Upload));
    startup.extend(args.requests.iter().cloned());
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let spawner = AppSpawner::start(clock.clone()).map_err(|e| format!("cannot start the app worker: {e}"))?;
    let mut platform = platform()?;
    if args.sandbox {
        platform.secrets = Arc::new(stubs::MemorySecrets::default());
    }
    let data = data_dir(&args);
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
    let rng = stubs::system_rng();
    let uploaders = crate::uploads::registry(&data, Arc::new(ui.clone()), rng.clone());
    let host_deps = crate::uploads::host_deps(platform.secrets.clone(), platform.shell.clone());
    crate::uploads::apply_accounts(&uploaders, &settings.hosts, &host_deps);
    let cloud_options: crate::cloud::SharedCloudOptions = Arc::new(std::sync::RwLock::new(settings.cloud.clone()));
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
        media: Arc::new(crate::media::LazyMedia::new(
            data.clone(),
            {
                let s = settings.defaults.recording.ffmpeg_path.clone();
                Arc::new(move || s.clone())
            },
            Arc::new(ui.clone()),
        )),
        stream: platform.stream.clone(),
        ocr: Arc::new(crate::ocr::LazyOcr::new(data.clone(), Arc::new(ui.clone()), model_options.clone(), platform.os_ocr.clone())),
        window_ops: platform.window_ops.clone(),
        input: platform.input.clone(),
        highlighter: platform.highlighter.clone(),
        background: Arc::new(crate::cloud::Background::new(
            cloud_options.clone(),
            Arc::new(stubs::M7Pending),
            platform.secrets.clone(),
            spawner.clone(),
            laqta_imaging::Codecs::builtin(),
        )),
        analyzer: Arc::new(crate::cloud::Analyzer {
            options: cloud_options.clone(),
            secrets: platform.secrets.clone(),
            spawner: spawner.clone(),
        }),
        inpainter: Arc::new(stubs::M7Pending),
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

    // Task events: the log and the history writer.
    let mut events = app.events();
    spawner.spawn(Box::pin(async move {
        while let Ok(event) = events.recv().await {
            tracing::info!(?event, "task event");
        }
    }));
    if let Some(h) = &history {
        spawner.spawn(Box::pin(laqta_library::record_task_events(h.clone(), app.events(), clock.clone())));
    }

    // Every entry point goes through `dispatch` (ADR 015). A hotkey runs the first enabled workflow
    // bound to its action, so that workflow's settings apply.
    let dispatch_slot: Arc<Mutex<Option<ActionCallback>>> = Arc::new(Mutex::new(None));
    let dispatch: ActionCallback = {
        let app = app.clone();
        let spawner = spawner.clone();
        let quit_tx = quit_tx.clone();
        let hotkeys = hotkeys.clone();
        let slot = dispatch_slot.clone();
        Arc::new(move |action: Action| {
            let app = app.clone();
            let quit_tx = quit_tx.clone();
            let hotkeys = hotkeys.clone();
            let slot = slot.clone();
            spawner.spawn(Box::pin(async move {
                let workflow = app.settings().workflows.iter().find(|w| w.enabled && w.action == action).map(|w| w.id);
                match app.dispatch(action, workflow).await {
                    Ok(Dispatched::Exit) => {
                        let _ = quit_tx.send(());
                    }
                    Ok(Dispatched::HotkeysToggled(off)) => {
                        let cb = slot.lock().ok().and_then(|s| s.clone());
                        if let Some(cb) = cb {
                            register_hotkeys(hotkeys.as_ref(), &app.settings(), cb);
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
    if let Ok(mut s) = dispatch_slot.lock() {
        *s = Some(dispatch.clone());
    }

    // Sandbox mode writes no settings (M6 D8); `-NoHotkeys` registers none.
    let use_hotkeys = !args.no_hotkeys;
    let save_settings: crate::imports::SaveSettings = {
        let model_options = model_options.clone();
        let cloud_options = cloud_options.clone();
        let app = app.clone();
        let store = store.clone();
        let clock = clock.clone();
        let hotkeys = hotkeys.clone();
        let dispatch = dispatch.clone();
        let uploaders = uploaders.clone();
        let host_deps = host_deps.clone();
        let sandbox = args.sandbox;
        Arc::new(move |s: Settings| {
            if !sandbox {
                store.save(&s, SETTINGS_VERSION, clock.now()).map_err(|e| e.to_string())?;
            }
            crate::uploads::apply_accounts(&uploaders, &s.hosts, &host_deps);
            laqta_ui::i18n::set_language(s.language.as_deref());
            if let Ok(mut m) = model_options.write() {
                *m = s.models.clone();
            }
            if let Ok(mut c) = cloud_options.write() {
                *c = s.cloud.clone();
            }
            app.set_settings(s.clone());
            if use_hotkeys {
                register_hotkeys(hotkeys.as_ref(), &s, dispatch.clone());
            }
            app.start_watch_folders();
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
    if use_hotkeys && register_hotkeys(hotkeys.as_ref(), &settings, dispatch.clone()) {
        menu.tooltip = laqta_ui::presenters::tray::TOOLTIP_NO_HOTKEY.to_string();
    }
    match platform.tray.show(menu, dispatch.clone()) {
        Ok(()) => tracing::info!("tray icon shown"),
        Err(PortError::Unsupported(why)) => tracing::warn!("no tray ({why}); the CLI and hotkey still work"),
        Err(e) => tracing::warn!("tray failed: {e}; the CLI and hotkey still work"),
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
        spawner.spawn(Box::pin(async move {
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
                    Err(_) => break,
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
            fonts: crate::fonts::fonts(),
            spawner: spawner.clone(),
            codecs: laqta_imaging::Codecs::builtin(),
            import_sharex: {
                let importer = importer.clone();
                Arc::new(move |path: std::path::PathBuf| importer.sharex(&path))
            },
            models: Arc::new(crate::models::Models { data: data.clone(), options: model_options.clone() }),
            check_update: {
                let last = last_release.clone();
                Arc::new(move || {
                    let last = last.clone();
                    Box::pin(async move {
                        let found = laqta_http::run(crate::updates::check()).await?;
                        let msg = crate::updates::describe(&found);
                        let installable = cfg!(windows) && found.is_some();
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

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
}

#[cfg(target_os = "linux")]
fn platform() -> Result<Platform, String> {
    use laqta_platform_linux::Session;
    let session = Session::detect().ok_or("no display: neither WAYLAND_DISPLAY nor DISPLAY is set")?;
    tracing::info!("session: {session:?}");
    let p = laqta_platform_linux::ports(session);
    Ok(Platform { screen: p.screen, clipboard: p.clipboard, shell: p.shell, hover: p.hover, hotkeys: p.hotkeys, tray: p.tray, secrets: p.secrets, stream: p.stream })
}

#[cfg(windows)]
fn platform() -> Result<Platform, String> {
    laqta_platform_windows::set_dpi_awareness();
    let p = laqta_platform_windows::ports();
    Ok(Platform { screen: p.screen, clipboard: p.clipboard, shell: p.shell, hover: p.hover, hotkeys: p.hotkeys, tray: p.tray, secrets: p.secrets, stream: p.stream })
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform() -> Result<Platform, String> {
    Err("this platform is not supported (ADR 001)".into())
}

fn pictures_dir() -> std::path::PathBuf {
    dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))).unwrap_or_else(|| "Pictures".into())
}

fn data_dir() -> std::path::PathBuf {
    let exe_dir = std::env::current_exe().ok().and_then(|p| p.parent().map(|d| d.to_path_buf()));
    let default = dirs::data_local_dir().unwrap_or_else(|| ".".into()).join("Laqta");
    laqta_library::data_dir(exe_dir.as_deref(), default)
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
    let startup_actions = args.actions();
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let spawner = AppSpawner::start(clock.clone()).map_err(|e| format!("cannot start the app worker: {e}"))?;
    let platform = platform()?;
    let data = data_dir();
    let store = Arc::new(SettingsStore::new(data.clone()));
    let settings = load_settings(&store);
    let history = if settings.history.enabled {
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
        media: Arc::new(crate::media::LazyMedia::new(data.clone(), {
            let s = settings.defaults.recording.ffmpeg_path.clone();
            Arc::new(move || s.clone())
        }, Arc::new(ui.clone()))),
        stream: platform.stream.clone(),
        ocr: Arc::new(crate::ocr::LazyOcr::new(data.clone(), Arc::new(ui.clone()))),
        window_ops: Arc::new(stubs::NotYet),
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

    // IPC: replies after the dispatch returns.
    if let Some(server) = server {
        let app = app.clone();
        let quit_tx = quit_tx.clone();
        server.serve(move |action| match futures_lite::future::block_on(app.dispatch(action, None)) {
            Ok(Dispatched::Exit) => {
                let _ = quit_tx.send(());
                Ok(())
            }
            Ok(_) => Ok(()),
            Err(e) => Err(e.to_string()),
        });
    }

    let settings = app.settings();
    let mut menu: TrayMenu = laqta_ui::presenters::tray::tray_menu(&settings);
    if register_hotkeys(hotkeys.as_ref(), &settings, dispatch.clone()) {
        menu.tooltip = laqta_ui::presenters::tray::TOOLTIP_NO_HOTKEY.to_string();
    }
    match platform.tray.show(menu, dispatch.clone()) {
        Ok(()) => tracing::info!("tray icon shown"),
        Err(PortError::Unsupported(why)) => tracing::warn!("no tray ({why}); the CLI and hotkey still work"),
        Err(e) => tracing::warn!("tray failed: {e}; the CLI and hotkey still work"),
    }

    for action in startup_actions {
        dispatch(action);
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
        let save_settings: Arc<dyn Fn(Settings) -> Result<(), String> + Send + Sync> = {
            let app = app.clone();
            let store = store.clone();
            let clock = clock.clone();
            let hotkeys = hotkeys.clone();
            let dispatch = dispatch.clone();
            let uploaders = uploaders.clone();
            let host_deps = host_deps.clone();
            Arc::new(move |s: Settings| {
                store.save(&s, SETTINGS_VERSION, clock.now()).map_err(|e| e.to_string())?;
                crate::uploads::apply_accounts(&uploaders, &s.hosts, &host_deps);
                app.set_settings(s.clone());
                register_hotkeys(hotkeys.as_ref(), &s, dispatch.clone());
                Ok(())
            })
        };
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
            add_custom_uploader: {
                let uploaders = uploaders.clone();
                let dir = data.join("uploaders");
                Arc::new(move |json: String| crate::uploads::add_custom(&uploaders, &dir, &json))
            },
        };
        laqta_ui::run(services, ui_rx);
    }
    tracing::info!("Laqta exiting");
    #[cfg(windows)]
    laqta_platform_windows::shutdown(); // removes the tray icon, so Explorer leaves no ghost
    Ok(())
}

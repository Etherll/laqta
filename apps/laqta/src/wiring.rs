//! Builds the ports for this platform and session, starts every entry point, and runs the loop.

use std::sync::Arc;
use std::sync::mpsc;

use laqta_app::{App, Dispatched, Ports};
use laqta_domain::{Action, Settings};
use laqta_ports::{ActionCallback, Clock, HotkeyRegistrar, PortError, Spawner, Tray, TrayMenu};

use crate::stubs;

use crate::spawner::{AppSpawner, SystemClock};

struct Platform {
    ports: Ports,
    hotkeys: Arc<dyn HotkeyRegistrar>,
    tray: Arc<dyn Tray>,
}

#[cfg(target_os = "linux")]
fn platform(clock: Arc<dyn Clock>, spawner: Arc<dyn Spawner>) -> Result<Platform, String> {
    use laqta_platform_linux::Session;
    let session = Session::detect().ok_or("no display: neither WAYLAND_DISPLAY nor DISPLAY is set")?;
    tracing::info!("session: {session:?}");
    let p = laqta_platform_linux::ports(session);
    Ok(Platform {
        ports: stubs::ports(p.screen, p.clipboard, p.shell, clock, spawner),
        hotkeys: p.hotkeys,
        tray: p.tray,
    })
}

#[cfg(windows)]
fn platform(clock: Arc<dyn Clock>, spawner: Arc<dyn Spawner>) -> Result<Platform, String> {
    laqta_platform_windows::set_dpi_awareness();
    let p = laqta_platform_windows::ports();
    Ok(Platform {
        ports: stubs::ports(p.screen, p.clipboard, p.shell, clock, spawner),
        hotkeys: p.hotkeys,
        tray: p.tray,
    })
}

#[cfg(not(any(target_os = "linux", windows)))]
fn platform(_clock: Arc<dyn Clock>, _spawner: Arc<dyn Spawner>) -> Result<Platform, String> {
    Err("this platform is not supported (ADR 001)".into())
}

fn pictures_dir() -> std::path::PathBuf {
    dirs::picture_dir().or_else(|| dirs::home_dir().map(|h| h.join("Pictures"))).unwrap_or_else(|| "Pictures".into())
}

pub fn run(server: laqta_ipc::Server, startup_actions: Vec<Action>) -> Result<(), String> {
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let spawner = AppSpawner::start(clock.clone()).map_err(|e| format!("cannot start the app worker: {e}"))?;
    let platform = platform(clock, spawner.clone())?;
    let app = App::new(platform.ports, Settings::m0_default(pictures_dir()));
    let (quit_tx, quit_rx) = mpsc::channel::<()>();

    // Task-event logger.
    let mut events = app.events();
    spawner.spawn(Box::pin(async move {
        while let Ok(event) = events.recv().await {
            tracing::info!(?event, "task event");
        }
    }));

    // Every entry point goes through `dispatch` (ADR 015).
    let dispatch: ActionCallback = {
        let app = app.clone();
        let spawner = spawner.clone();
        let quit_tx = quit_tx.clone();
        Arc::new(move |action: Action| {
            let app = app.clone();
            let quit_tx = quit_tx.clone();
            spawner.spawn(Box::pin(async move {
                match app.dispatch(action, None).await {
                    Ok(Dispatched::Exit) => {
                        let _ = quit_tx.send(());
                    }
                    Ok(d) => tracing::debug!(?action, ?d, "dispatched"),
                    Err(e) => tracing::warn!(?action, "action failed: {e}"),
                }
            }));
        })
    };

    // IPC: replies after the dispatch returns.
    {
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

    // Hotkeys, then the tray (its tooltip depends on whether hotkeys work).
    let settings = app.settings();
    let mut menu: TrayMenu = laqta_ui::presenters::tray::tray_menu(&settings);
    for (chord, result) in platform.hotkeys.register(&settings.hotkey_bindings(), dispatch.clone()) {
        match result {
            Ok(()) => tracing::info!(?chord, "hotkey registered"),
            Err(PortError::Unsupported(why)) => {
                tracing::warn!(?chord, "global hotkeys unsupported here ({why}); bind `laqta --action print-screen` in your desktop's keyboard settings");
                menu.tooltip = laqta_ui::presenters::tray::TOOLTIP_NO_HOTKEY.to_string();
            }
            Err(e) => tracing::warn!(?chord, "hotkey registration failed: {e}"),
        }
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
        let _ = quit_rx.recv();
    } else {
        run_gpui(quit_rx);
    }
    tracing::info!("Laqta exiting");
    #[cfg(windows)]
    laqta_platform_windows::shutdown(); // removes the tray icon, so Explorer leaves no ghost
    Ok(())
}

/// GPUI on the main thread with no windows. A future waits on the quit channel and quits the app.
fn run_gpui(quit_rx: mpsc::Receiver<()>) {
    gpui_platform::application().run(move |cx: &mut gpui::App| {
        cx.spawn(async move |cx: &mut gpui::AsyncApp| {
            let _ = crate::blocking_pool::unblock(move || quit_rx.recv()).await;
            cx.update(|cx| cx.quit());
        })
        .detach();
    });
}

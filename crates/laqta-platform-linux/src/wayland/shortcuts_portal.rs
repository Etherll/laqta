use std::sync::Mutex;
use std::time::Duration;

use ashpd::desktop::CreateSessionOptions;
use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
use futures_lite::StreamExt;
use laqta_domain::{Action, Key, KeyChord};
use laqta_ports::{ActionCallback, HotkeyRegistrar, PortError};

/// GlobalShortcuts portal: one session, shortcut ids derived from the action
/// (`print-screen` → `Print`). Missing portal → `Unsupported`.
#[derive(Default)]
pub struct PortalShortcuts {
    started: Mutex<bool>,
}

fn shortcut_id(action: Action) -> &'static str {
    match action {
        Action::PrintScreen => "print-screen",
        Action::OpenScreenshotsFolder => "open-screenshots-folder",
        Action::ExitLaqta => "exit",
        #[allow(unreachable_patterns)]
        _ => "unknown",
    }
}

fn trigger(chord: KeyChord) -> &'static str {
    match chord.key {
        Key::PrintScreen => "Print",
        #[allow(unreachable_patterns)]
        _ => "",
    }
}

impl HotkeyRegistrar for PortalShortcuts {
    fn register(&self, bindings: &[(KeyChord, Action)], on_fire: ActionCallback) -> Vec<(KeyChord, Result<(), PortError>)> {
        let all = |r: Result<(), PortError>| bindings.iter().map(|(c, _)| (*c, r.clone())).collect::<Vec<_>>();
        if let Ok(mut s) = self.started.lock() {
            if *s {
                return all(Err(PortError::Failed("hotkeys already registered".into())));
            }
            *s = true;
        }
        let bindings: Vec<(KeyChord, Action)> = bindings.to_vec();
        let (tx, rx) = std::sync::mpsc::channel::<Result<(), PortError>>();
        let spawned = std::thread::Builder::new().name("laqta-shortcuts".into()).spawn(move || {
            async_io::block_on(async move {
                let portal = match GlobalShortcuts::new().await {
                    Ok(p) => p,
                    Err(e) => {
                        let _ = tx.send(Err(PortError::Unsupported(format!("GlobalShortcuts portal: {e}"))));
                        return;
                    }
                };
                let session = match portal.create_session(CreateSessionOptions::default()).await {
                    Ok(s) => s,
                    Err(e) => {
                        let _ = tx.send(Err(PortError::Unsupported(format!("GlobalShortcuts session: {e}"))));
                        return;
                    }
                };
                // The portal is present: report success now; binding may show a dialog.
                let _ = tx.send(Ok(()));
                let shortcuts: Vec<NewShortcut> = bindings
                    .iter()
                    .map(|(c, a)| NewShortcut::new(shortcut_id(*a), "Capture fullscreen").preferred_trigger(Some(trigger(*c))))
                    .collect();
                match portal.bind_shortcuts(&session, &shortcuts, None, BindShortcutsOptions::default()).await.map(|r| r.response()) {
                    Ok(Ok(_)) => tracing::info!("GlobalShortcuts bound"),
                    Ok(Err(e)) | Err(e) => tracing::warn!("GlobalShortcuts bind failed: {e}"),
                }
                let Ok(mut stream) = portal.receive_activated().await else {
                    tracing::warn!("GlobalShortcuts: no activation stream");
                    return;
                };
                while let Some(activated) = stream.next().await {
                    if let Some((_, action)) = bindings.iter().find(|(_, a)| shortcut_id(*a) == activated.shortcut_id()) {
                        on_fire(*action);
                    }
                }
                drop(session);
            })
        });
        if let Err(e) = spawned {
            return all(Err(PortError::Failed(e.to_string())));
        }
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(r) => all(r),
            Err(_) => all(Err(PortError::Unsupported("GlobalShortcuts portal did not answer within 5 s".into()))),
        }
    }
}

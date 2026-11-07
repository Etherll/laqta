use std::sync::Mutex;
use std::time::Duration;

use laqta_ports::{ActionCallback, PortError, Tray, TrayMenu};

/// StatusNotifierItem tray (KDE, most wlroots bars, GNOME with the AppIndicator extension).
#[derive(Default)]
pub struct SniTray {
    handle: Mutex<Option<ksni::Handle<SniItem>>>,
}

struct SniItem {
    menu: TrayMenu,
    on_select: ActionCallback,
}

impl ksni::Tray for SniItem {
    fn id(&self) -> String {
        "laqta".into()
    }
    fn title(&self) -> String {
        "Laqta".into()
    }
    fn icon_name(&self) -> String {
        "camera-photo".into()
    }
    fn tool_tip(&self) -> ksni::ToolTip {
        ksni::ToolTip { title: self.menu.tooltip.clone(), ..Default::default() }
    }
    fn activate(&mut self, _x: i32, _y: i32) {
        (self.on_select)(self.menu.default_action);
    }
    fn menu(&self) -> Vec<ksni::MenuItem<Self>> {
        self.menu
            .items
            .iter()
            .map(|item| {
                let action = item.action;
                ksni::menu::StandardItem {
                    label: item.label.clone(),
                    activate: Box::new(move |me: &mut SniItem| (me.on_select)(action)),
                    ..Default::default()
                }
                .into()
            })
            .collect()
    }
}

impl Tray for SniTray {
    fn show(&self, menu: TrayMenu, on_select: ActionCallback) -> Result<(), PortError> {
        let mut guard = self.handle.lock().map_err(|_| PortError::Failed("tray lock poisoned".into()))?;
        if let Some(h) = guard.as_ref() {
            let menu2 = menu.clone();
            async_io::block_on(h.update(move |t: &mut SniItem| {
                t.menu = menu2;
                t.on_select = on_select;
            }));
            return Ok(());
        }
        let (tx, rx) = std::sync::mpsc::channel();
        let item = SniItem { menu, on_select };
        std::thread::Builder::new()
            .name("laqta-tray".into())
            .spawn(move || {
                async_io::block_on(async move {
                    use ksni::TrayMethods;
                    match item.spawn().await {
                        Ok(handle) => {
                            let _ = tx.send(Ok(handle));
                            futures_lite::future::pending::<()>().await;
                        }
                        Err(e) => {
                            let _ = tx.send(Err(e));
                        }
                    }
                })
            })
            .map_err(|e| PortError::Failed(e.to_string()))?;
        match rx.recv_timeout(Duration::from_secs(5)) {
            Ok(Ok(handle)) => {
                *guard = Some(handle);
                Ok(())
            }
            Ok(Err(ksni::Error::Watcher(e))) => Err(PortError::Unsupported(format!("no StatusNotifier host: {e}"))),
            Ok(Err(e)) => Err(PortError::Failed(e.to_string())),
            Err(_) => Err(PortError::Failed("tray did not start within 5 s".into())),
        }
    }
}

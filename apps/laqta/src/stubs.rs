//! Adapters not built yet in this wave, and the plain values `main` supplies.

use std::sync::Arc;

use laqta_app::Ports;
use laqta_ports::{BoxFuture, Clipboard, Clock, HostInfo, HoverSource, InputCallback, Notifier, Overlay, PortError, RegionView, Rng, ScreenSource, Shell, Spawner, Toast};

#[derive(Debug)]
pub struct NoOverlay;
impl Overlay for NoOverlay {
    fn show(&self, _: Vec<laqta_domain::Frame>, _: RegionView, _: InputCallback) -> Result<(), PortError> {
        Err(PortError::Unsupported("region overlay".into()))
    }
    fn update(&self, _: RegionView) {}
    fn close(&self) {}
}

/// Logs toasts until the GPUI toast adapter replaces it.
#[derive(Debug)]
pub struct LogNotifier;
impl Notifier for LogNotifier {
    fn toast(&self, toast: Toast) -> Result<(), PortError> {
        tracing::info!(title = %toast.title, body = %toast.body, "toast");
        Ok(())
    }
}

#[derive(Debug)]
pub struct SystemRng(std::sync::Mutex<fastrand::Rng>);
impl Rng for SystemRng {
    fn next_u64(&self) -> u64 {
        match self.0.lock() {
            Ok(mut r) => r.u64(..),
            Err(p) => p.into_inner().u64(..),
        }
    }
}

pub fn host_info() -> HostInfo {
    let env = |k: &str| std::env::var(k).unwrap_or_default();
    let user = if cfg!(windows) { env("USERNAME") } else { env("USER") };
    let computer = if cfg!(windows) {
        env("COMPUTERNAME")
    } else {
        std::fs::read_to_string("/etc/hostname").map(|s| s.trim().to_string()).unwrap_or_else(|_| env("HOSTNAME"))
    };
    HostInfo { user_name: user.clone(), user_login: user, computer_name: computer }
}

pub fn ports(
    screen: Arc<dyn ScreenSource>,
    clipboard: Arc<dyn Clipboard>,
    shell: Arc<dyn Shell>,
    hover: Arc<dyn HoverSource>,
    clock: Arc<dyn Clock>,
    spawner: Arc<dyn Spawner>,
) -> Ports {
    Ports {
        screen,
        clipboard,
        shell,
        clock,
        spawner,
        overlay: Arc::new(NoOverlay),
        hover,
        notifier: Arc::new(LogNotifier),
        rng: Arc::new(SystemRng(std::sync::Mutex::new(fastrand::Rng::new()))),
        host: host_info(),
        codecs: laqta_imaging::Codecs::builtin(),
    }
}

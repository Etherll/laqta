//! Plain values and small adapters `main` builds itself.

use std::sync::Arc;

use laqta_ports::{HostInfo, Rng};

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

pub fn system_rng() -> Arc<dyn Rng> {
    Arc::new(SystemRng(std::sync::Mutex::new(fastrand::Rng::new())))
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



/// Until the OCR model runtime and the platform window tools are wired (M5).
#[derive(Debug)]
pub struct NotYet;

impl laqta_ports::Ocr for NotYet {
    fn recognize(&self, _image: laqta_domain::Bitmap, _language: String) -> laqta_ports::BoxFuture<'_, Result<laqta_ports::OcrResult, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("text recognition".into())) })
    }
    fn languages(&self) -> Vec<String> {
        Vec::new()
    }
}

impl laqta_ports::WindowOps for NotYet {
    fn set_borderless(&self, _w: &laqta_domain::region::HoverTarget, _b: bool) -> Result<(), laqta_ports::PortError> {
        Err(laqta_ports::PortError::Unsupported("window tools".into()))
    }
    fn set_topmost(&self, _w: &laqta_domain::region::HoverTarget, _t: bool) -> Result<(), laqta_ports::PortError> {
        Err(laqta_ports::PortError::Unsupported("window tools".into()))
    }
    fn inspect(&self, _w: &laqta_domain::region::HoverTarget) -> Result<Vec<(String, String)>, laqta_ports::PortError> {
        Err(laqta_ports::PortError::Unsupported("window tools".into()))
    }
}

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

/// No destinations until the upload registry is wired (M2).
#[derive(Debug)]
pub struct NoUploaders;

impl laqta_ports::Uploaders for NoUploaders {
    fn destination(&self, _id: &str) -> Option<Arc<dyn laqta_ports::Destination>> {
        None
    }
    fn shortener(&self, _id: &str) -> Option<Arc<dyn laqta_ports::Shortener>> {
        None
    }
    fn sharer(&self, _id: &str) -> Option<Arc<dyn laqta_ports::Sharer>> {
        None
    }
    fn destination_list(&self) -> Vec<(String, String)> {
        Vec::new()
    }
    fn download(&self, url: &str) -> laqta_ports::BoxFuture<'_, Result<laqta_ports::upload::Downloaded, laqta_ports::UploadError>> {
        let e = laqta_ports::UploadError::Unsupported(format!("download {url}"));
        Box::pin(async move { Err(e) })
    }
}

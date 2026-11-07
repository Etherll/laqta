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

/// Sandbox mode's secret store (M6 D8): kept in memory, gone on exit.
#[derive(Debug, Default)]
pub struct MemorySecrets(std::sync::Mutex<std::collections::HashMap<String, String>>);

impl laqta_ports::SecretStore for MemorySecrets {
    fn get(&self, key: &str) -> Result<Option<String>, laqta_ports::PortError> {
        Ok(self.0.lock().ok().and_then(|m| m.get(key).cloned()))
    }
    fn set(&self, key: &str, secret: &str) -> Result<(), laqta_ports::PortError> {
        if let Ok(mut m) = self.0.lock() {
            m.insert(key.to_owned(), secret.to_owned());
        }
        Ok(())
    }
    fn delete(&self, key: &str) -> Result<(), laqta_ports::PortError> {
        if let Ok(mut m) = self.0.lock() {
            m.remove(key);
        }
        Ok(())
    }
}

/// Until the M7 adapters land.
#[derive(Debug)]
pub struct M7Pending;

impl laqta_ports::BackgroundRemover for M7Pending {
    fn remove(&self, _image: laqta_domain::Bitmap) -> laqta_ports::BoxFuture<'_, Result<laqta_domain::Bitmap, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("background removal".into())) })
    }
}

impl laqta_ports::ImageAnalyzer for M7Pending {
    fn analyze(&self, _image: laqta_domain::Bitmap, _prompt: String) -> laqta_ports::BoxFuture<'_, Result<String, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("image analysis".into())) })
    }
}

impl laqta_ports::Inpainter for M7Pending {
    fn inpaint(
        &self,
        _image: laqta_domain::Bitmap,
        _mask: Vec<u8>,
    ) -> laqta_ports::BoxFuture<'_, Result<laqta_domain::Bitmap, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("inpainting".into())) })
    }
}

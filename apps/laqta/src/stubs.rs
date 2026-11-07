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


/// Until the platform's recording stream and the FFmpeg adapter are wired (M4).
#[derive(Debug)]
pub struct NoRecording;

impl laqta_ports::ScreenStream for NoRecording {
    fn open(
        &self,
        _area: laqta_domain::geometry::PhysRect,
        _show_cursor: bool,
        _fps: u32,
        _audio: &[laqta_domain::record::AudioDevice],
    ) -> laqta_ports::BoxFuture<'_, Result<laqta_ports::RecordingInput, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("screen recording".into())) })
    }
    fn audio_devices(&self) -> laqta_ports::BoxFuture<'_, Result<Vec<laqta_domain::record::AudioDevice>, laqta_ports::PortError>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}

impl laqta_ports::Media for NoRecording {
    fn start(
        &self,
        _input: laqta_ports::RecordingInput,
        _spec: laqta_domain::record::RecordSpec,
        _output: std::path::PathBuf,
    ) -> laqta_ports::BoxFuture<'_, Result<Box<dyn laqta_ports::RecordingHandle>, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("FFmpeg".into())) })
    }
    fn transcode(&self, _i: &std::path::Path, _o: &std::path::Path, _s: laqta_ports::TranscodeSpec) -> laqta_ports::BoxFuture<'_, Result<(), laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("FFmpeg".into())) })
    }
    fn probe(&self, _i: &std::path::Path) -> laqta_ports::BoxFuture<'_, Result<laqta_ports::MediaInfo, laqta_ports::PortError>> {
        Box::pin(async { Err(laqta_ports::PortError::Unsupported("FFmpeg".into())) })
    }
}

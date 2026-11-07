//! Toasts and randomness.

use std::path::PathBuf;

use laqta_domain::Bitmap;
use laqta_domain::workflow::ToastOptions;

use crate::PortError;

#[derive(Clone, Debug)]
pub struct Toast {
    pub title: String,
    pub body: String,
    pub image: Option<Bitmap>,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub options: ToastOptions,
}

/// Sounds Laqta plays after events (ShareX's per-event sounds).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Sound {
    Capture,
    TaskCompleted,
    ActionCompleted,
    Error,
}

pub trait Notifier: Send + Sync {
    fn toast(&self, toast: Toast) -> Result<(), PortError>;
    /// Plays a sound; `custom` is a user-chosen file that overrides the built-in one.
    fn sound(&self, sound: Sound, custom: Option<&std::path::Path>) -> Result<(), PortError> {
        let _ = (sound, custom);
        Ok(())
    }
}

pub trait Rng: Send + Sync {
    fn next_u64(&self) -> u64;
}

/// Facts about the machine that name patterns use (`%un`, `%uln`, `%cn`). A value, built by `main`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct HostInfo {
    pub user_name: String,
    pub user_login: String,
    pub computer_name: String,
}

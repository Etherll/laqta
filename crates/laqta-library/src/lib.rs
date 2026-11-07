//! Settings storage and history (ADR 006): versioned JSON settings with backups, and SQLite
//! history fed by task events. Outer crate: it may touch the file system and use threads, but
//! not tokio (ARCHITECTURE §2.4).
#![forbid(unsafe_code)]

mod error;
mod history;
mod paths;
mod settings_store;

pub use error::LibraryError;
pub use history::{History, record_task_events};
pub use paths::data_dir;
pub use settings_store::{Migration, SettingsStore};

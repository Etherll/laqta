//! Settings storage and history (ADR 006): versioned JSON settings with backups, and SQLite
//! history fed by task events. Outer crate: it may touch the file system and use threads, but
//! not tokio (ARCHITECTURE §2.4).
#![forbid(unsafe_code)]

mod archive;
mod error;
mod history;
mod legacy;
mod paths;
mod settings_store;
mod watch;

pub use archive::{apply_staged_import, export_archive, has_staged_import, import_archive, stage_archive};
pub use error::LibraryError;
pub use history::{History, record_task_events};
pub use legacy::{ImportReport, import_sharex_history, import_sharex_history_file};
pub use paths::data_dir;
pub use settings_store::{Migration, SettingsStore};
pub use watch::NotifyWatcher;

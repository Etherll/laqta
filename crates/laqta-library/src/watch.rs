//! Folder watching: the `FileWatcher` port over `notify` and `notify-debouncer-full`.
//! The debouncer runs its own thread; this adapter does not use tokio.

use std::collections::HashSet;
use std::io;
use std::path::Path;
use std::time::Duration;

use laqta_ports::{FileWatcher, PortError, WatchCallback, WatchHandle};
use notify::RecursiveMode;
use notify::event::{CreateKind, EventKind, ModifyKind, RenameMode};
use notify_debouncer_full::{DebounceEventResult, DebouncedEvent, new_debouncer};

/// Reports a file once writes to it have stopped for `settle`.
#[derive(Clone, Copy, Debug)]
pub struct NotifyWatcher {
    settle: Duration,
}

impl Default for NotifyWatcher {
    fn default() -> Self {
        Self::with_settle(Duration::from_secs(1))
    }
}

impl NotifyWatcher {
    pub fn with_settle(settle: Duration) -> Self {
        Self { settle }
    }
}

impl FileWatcher for NotifyWatcher {
    fn watch(&self, folder: &Path, recursive: bool, on_file: WatchCallback) -> Result<WatchHandle, PortError> {
        if !folder.is_dir() {
            return Err(PortError::Failed(format!("watch folder is missing: {}", folder.display())));
        }
        let mut debouncer = new_debouncer(self.settle, None, move |result: DebounceEventResult| report(result, &on_file))
            .map_err(|error| PortError::Failed(format!("file watcher did not start: {error}")))?;
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        debouncer.watch(folder, mode).map_err(|error| watch_error(folder, error))?;
        // Dropping the handle drops the debouncer, which stops its thread and the OS watch.
        Ok(WatchHandle(Box::new(debouncer)))
    }
}

fn report(result: DebounceEventResult, on_file: &WatchCallback) {
    match result {
        Ok(events) => report_events(&events, on_file),
        Err(errors) => {
            for error in errors {
                tracing::warn!(%error, "file watch error");
            }
        }
    }
}

fn report_events(events: &[DebouncedEvent], on_file: &WatchCallback) {
    let mut seen = HashSet::new();
    for event in events {
        if !is_arrival(&event.kind) {
            continue;
        }
        for path in &event.paths {
            if seen.insert(path) && path.is_file() {
                on_file(path.clone());
            }
        }
    }
}

/// A file was created or renamed into the folder. A data write to a file that was already there is
/// not an arrival. Writes to a file created in the same settle window are merged into its create event.
fn is_arrival(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::File | CreateKind::Any) | EventKind::Modify(ModifyKind::Any | ModifyKind::Name(RenameMode::To))
    )
}

fn watch_error(folder: &Path, error: notify::Error) -> PortError {
    match error.kind {
        notify::ErrorKind::Io(ref io) if io.kind() == io::ErrorKind::PermissionDenied => PortError::PermissionDenied,
        _ => PortError::Failed(format!("cannot watch {}: {error}", folder.display())),
    }
}

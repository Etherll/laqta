//! Folder watching: the `FileWatcher` port over `notify` and `notify-debouncer-full`.
//! The debouncer runs its own thread; this adapter does not use tokio.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::Duration;

use laqta_ports::{FileWatcher, PortError, WatchCallback, WatchHandle};
use notify::RecursiveMode;
use notify::event::{CreateKind, EventKind, ModifyKind, RemoveKind, RenameMode};
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
        // The files already there are not arrivals, whatever event a later write to them raises.
        let known = Mutex::new(files_in(folder, recursive));
        let mut debouncer = new_debouncer(self.settle, None, move |result: DebounceEventResult| report(result, &known, &on_file))
            .map_err(|error| PortError::Failed(format!("file watcher did not start: {error}")))?;
        let mode = if recursive { RecursiveMode::Recursive } else { RecursiveMode::NonRecursive };
        debouncer.watch(folder, mode).map_err(|error| watch_error(folder, error))?;
        // Dropping the handle drops the debouncer, which stops its thread and the OS watch.
        Ok(WatchHandle(Box::new(debouncer)))
    }
}

fn report(result: DebounceEventResult, known: &Mutex<HashSet<PathBuf>>, on_file: &WatchCallback) {
    match result {
        Ok(events) => report_events(&events, &mut known.lock().unwrap_or_else(PoisonError::into_inner), on_file),
        Err(errors) => {
            for error in errors {
                tracing::warn!(%error, "file watch error");
            }
        }
    }
}

/// Reports each path that arrived and was not in `known`, and keeps `known` up to date: a removed or
/// renamed-away file can arrive again.
fn report_events(events: &[DebouncedEvent], known: &mut HashSet<PathBuf>, on_file: &WatchCallback) {
    for event in events {
        if is_departure(&event.kind) {
            for path in &event.paths {
                forget(known, path);
            }
            continue;
        }
        // macOS reports both ends of a rename as `Name(Any)`: the end that is gone left.
        if matches!(event.kind, EventKind::Modify(ModifyKind::Name(RenameMode::Any))) {
            for path in event.paths.iter().filter(|path| !path.exists()) {
                forget(known, path);
            }
        }
        if !is_arrival(&event.kind) {
            continue;
        }
        for path in &event.paths {
            if path.is_file() && known.insert(key(path)) {
                on_file(path.clone());
            }
        }
    }
}

/// Forgets a path that left, and the files under it when it was a folder: the event names only the
/// folder.
fn forget(known: &mut HashSet<PathBuf>, path: &Path) {
    let gone = key(path);
    known.retain(|file| !file.starts_with(&gone));
}

/// An event that may mean a file appeared. Platforms differ (Windows reports a write as `Modify(Any)`,
/// macOS a rename as `Name(Any)` and can repeat a file's old "created" flag), so `known` decides
/// whether the file is new.
fn is_arrival(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::Create(CreateKind::File | CreateKind::Any)
            | EventKind::Modify(ModifyKind::Any | ModifyKind::Data(_) | ModifyKind::Name(RenameMode::To | RenameMode::Any))
    )
}

/// A file left: removed, or renamed away.
fn is_departure(kind: &EventKind) -> bool {
    matches!(kind, EventKind::Remove(RemoveKind::File | RemoveKind::Any) | EventKind::Modify(ModifyKind::Name(RenameMode::From)))
}

/// One spelling for a file's path: its folder resolved (macOS reports `/private/var` for `/var`), so
/// paths from events and from the folder listing compare equal, even after the file is gone.
fn key(path: &Path) -> PathBuf {
    match (path.parent().and_then(|parent| parent.canonicalize().ok()), path.file_name()) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

/// The files in `folder` now, and in its subfolders when `recursive`.
fn files_in(folder: &Path, recursive: bool) -> HashSet<PathBuf> {
    let mut files = HashSet::new();
    let mut folders = vec![folder.to_path_buf()];
    while let Some(dir) = folders.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(kind) = entry.file_type() else { continue };
            if kind.is_file() {
                files.insert(key(&entry.path()));
            } else if kind.is_dir() && recursive {
                folders.push(entry.path());
            }
        }
    }
    files
}

fn watch_error(folder: &Path, error: notify::Error) -> PortError {
    match error.kind {
        notify::ErrorKind::Io(ref io) if io.kind() == io::ErrorKind::PermissionDenied => PortError::PermissionDenied,
        _ => PortError::Failed(format!("cannot watch {}: {error}", folder.display())),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::sync::{Arc, Mutex};

    use notify::Event;

    use super::*;

    fn event(kind: EventKind, path: &Path) -> DebouncedEvent {
        DebouncedEvent::new(Event::new(kind).add_path(path.to_path_buf()), std::time::Instant::now())
    }

    /// A file renamed away the macOS way (`Name(Any)` on a path that is gone) can arrive again.
    #[test]
    fn a_name_renamed_away_on_macos_can_arrive_again() {
        let dir = tempfile::tempdir().unwrap();
        let shot = dir.path().join("shot.png");
        std::fs::write(&shot, b"1").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let on_file: WatchCallback = Arc::new(move |path| sink.lock().unwrap().push(path));
        let mut known = files_in(dir.path(), false);

        std::fs::remove_file(&shot).unwrap();
        report_events(&[event(EventKind::Modify(ModifyKind::Name(RenameMode::Any)), &shot)], &mut known, &on_file);
        std::fs::write(&shot, b"2").unwrap();
        report_events(&[event(EventKind::Create(CreateKind::File), &shot)], &mut known, &on_file);

        assert_eq!(seen.lock().unwrap().as_slice(), [shot]);
    }

    /// A subfolder renamed away takes its files with it: a folder of that name made again brings new files.
    #[test]
    fn the_files_of_a_folder_renamed_away_can_arrive_again() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("day");
        std::fs::create_dir(&sub).unwrap();
        let shot = sub.join("shot.png");
        std::fs::write(&shot, b"1").unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let sink = seen.clone();
        let on_file: WatchCallback = Arc::new(move |path| sink.lock().unwrap().push(path));
        let mut known = files_in(dir.path(), true);

        std::fs::rename(&sub, dir.path().join("day-old")).unwrap();
        report_events(&[event(EventKind::Modify(ModifyKind::Name(RenameMode::From)), &sub)], &mut known, &on_file);
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(&shot, b"2").unwrap();
        report_events(&[event(EventKind::Create(CreateKind::File), &shot)], &mut known, &on_file);

        assert_eq!(seen.lock().unwrap().as_slice(), [shot]);
    }
}

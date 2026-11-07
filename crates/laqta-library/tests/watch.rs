#![allow(clippy::unwrap_used)]

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::Duration;

use laqta_library::NotifyWatcher;
use laqta_ports::{FileWatcher, PortError, WatchCallback};

const SETTLE: Duration = Duration::from_millis(200);
/// Long enough for a write to settle, be debounced and be delivered, with room to spare.
const QUIET: Duration = Duration::from_millis(900);
const WAIT: Duration = Duration::from_secs(5);

fn watcher() -> NotifyWatcher {
    NotifyWatcher::with_settle(SETTLE)
}

fn channel_callback() -> (WatchCallback, mpsc::Receiver<PathBuf>) {
    let (tx, rx) = mpsc::channel();
    let callback: WatchCallback = Arc::new(move |path: PathBuf| {
        let _ = tx.send(path);
    });
    (callback, rx)
}

fn file_name(path: &Path) -> &OsStr {
    path.file_name().unwrap()
}

#[test]
fn new_file_is_reported_once() {
    let dir = tempfile::tempdir().unwrap();
    let (callback, rx) = channel_callback();
    let _handle = watcher().watch(dir.path(), false, callback).unwrap();

    fs::write(dir.path().join("shot.png"), b"png").unwrap();

    let reported = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(file_name(&reported), OsStr::new("shot.png"));
    assert!(rx.recv_timeout(QUIET).is_err(), "the same file was reported twice");
}

#[test]
fn subfolder_files_are_reported_only_when_recursive() {
    let flat = tempfile::tempdir().unwrap();
    fs::create_dir(flat.path().join("sub")).unwrap();
    let (callback, rx) = channel_callback();
    let handle = watcher().watch(flat.path(), false, callback).unwrap();

    fs::write(flat.path().join("sub").join("flat.png"), b"png").unwrap();
    assert!(rx.recv_timeout(QUIET).is_err(), "a non-recursive watch reported a subfolder file");
    drop(handle);

    let deep = tempfile::tempdir().unwrap();
    fs::create_dir(deep.path().join("sub")).unwrap();
    let (callback, rx) = channel_callback();
    let _handle = watcher().watch(deep.path(), true, callback).unwrap();

    fs::write(deep.path().join("sub").join("deep.png"), b"png").unwrap();
    let reported = rx.recv_timeout(WAIT).unwrap();
    assert_eq!(file_name(&reported), OsStr::new("deep.png"));
}

#[test]
fn dropping_the_handle_stops_reports() {
    let dir = tempfile::tempdir().unwrap();
    let (callback, rx) = channel_callback();
    let handle = watcher().watch(dir.path(), false, callback).unwrap();

    fs::write(dir.path().join("before.png"), b"png").unwrap();
    assert_eq!(file_name(&rx.recv_timeout(WAIT).unwrap()), OsStr::new("before.png"));

    drop(handle);
    fs::write(dir.path().join("after.png"), b"png").unwrap();
    assert!(rx.recv_timeout(QUIET).is_err(), "a file was reported after the handle was dropped");
}

#[test]
fn missing_folder_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let (callback, _rx) = channel_callback();

    let result = watcher().watch(&dir.path().join("missing"), true, callback);

    assert!(matches!(result, Err(PortError::Failed(_))), "got {result:?}");
}

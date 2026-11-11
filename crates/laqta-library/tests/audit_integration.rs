//! A watch folder reports only newly created (or moved-in) files: a data write to a file that was
//! already there is not an arrival, so editing a file in a watched folder does not upload it again.
#![allow(clippy::unwrap_used)]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, mpsc};
use std::time::Duration;

use laqta_library::NotifyWatcher;
use laqta_ports::{FileWatcher, WatchCallback};

#[test]
fn audit_bug_watch_reports_modified_existing_file() {
    let dir = tempfile::tempdir().unwrap();
    let existing = dir.path().join("notes.png");
    fs::write(&existing, b"old").unwrap();
    std::thread::sleep(Duration::from_millis(300));

    let (tx, rx) = mpsc::channel::<PathBuf>();
    let callback: WatchCallback = Arc::new(move |p| {
        let _ = tx.send(p);
    });
    let _handle = NotifyWatcher::with_settle(Duration::from_millis(200)).watch(dir.path(), false, callback).unwrap();
    std::thread::sleep(Duration::from_millis(200));

    let mut f = OpenOptions::new().append(true).open(&existing).unwrap();
    f.write_all(b" edited").unwrap();
    drop(f);

    let got = rx.recv_timeout(Duration::from_millis(1500));
    assert!(got.is_err(), "an edit to an existing file was reported as a new file: {got:?}");
}

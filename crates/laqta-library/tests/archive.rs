#![allow(clippy::unwrap_used)]

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{BufReader, Write};
use std::path::Path;

use jiff::civil::date;
use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery, NewHistoryItem};
use laqta_library::{History, LibraryError, export_archive, import_archive};
use tempfile::TempDir;
use zip::ZipArchive;
use zip::write::SimpleFileOptions;

const SETTINGS: &str = r#"{"version":3,"name":"work"}"#;

fn new_item(day: i8, file_name: &str) -> NewHistoryItem {
    NewHistoryItem {
        time: date(2026, 11, day).at(9, 0, 0, 0),
        kind: HistoryKind::Image,
        file_name: file_name.to_owned(),
        path: None,
        url: Some(format!("https://imgur.example/{day}")),
        thumbnail_url: None,
        deletion_url: None,
        short_url: None,
        host: Some("imgur.example".to_owned()),
        width: Some(1920),
        height: Some(1080),
        size_bytes: Some(4096),
        thumbnail_path: None,
        tags: vec!["work".to_owned()],
    }
}

/// A data folder with every part an export looks for, plus files it must leave out. Returns the open
/// history database, so its write-ahead log is still in use when the export runs.
fn populated(dir: &Path) -> History {
    fs::write(dir.join("settings.json"), SETTINGS).unwrap();
    fs::create_dir_all(dir.join("uploaders")).unwrap();
    fs::write(dir.join("uploaders/imgur.sxcu"), r#"{"Name":"imgur"}"#).unwrap();
    fs::write(dir.join("uploaders/notes.txt"), "not an uploader").unwrap();
    fs::create_dir_all(dir.join("backups/weekly")).unwrap();
    fs::write(dir.join("backups/settings-20261101-090000.json"), "{}").unwrap();
    fs::write(dir.join("backups/weekly/settings-2026-W44.json"), r#"{"old":true}"#).unwrap();

    let history = History::open(&dir.join("history.db")).unwrap();
    history.add(&new_item(2, "Laqta_2026-11-02.png")).unwrap();
    history.add(&new_item(3, "Laqta_2026-11-03.png")).unwrap();
    assert!(dir.join("history.db-wal").exists(), "the rows sit in the write-ahead log until a checkpoint");
    history
}

fn all(db: &History) -> Vec<HistoryItem> {
    db.query(&HistoryQuery::default()).unwrap()
}

fn entry_names(zip_path: &Path) -> BTreeSet<String> {
    let archive = ZipArchive::new(BufReader::new(File::open(zip_path).unwrap())).unwrap();
    archive.file_names().map(|name| name.unwrap().into_owned()).collect()
}

/// Writes a zip with exactly these entries. The zip writer does not check names, so this can make
/// hostile ones.
fn write_zip(path: &Path, entries: &[(&str, &[u8])]) {
    let mut zip = zip::ZipWriter::new(File::create(path).unwrap());
    for (name, body) in entries {
        zip.start_file(*name, SimpleFileOptions::default()).unwrap();
        zip.write_all(body).unwrap();
    }
    zip.finish().unwrap();
}

#[test]
fn export_then_import_restores_every_part_into_a_fresh_folder() {
    let source = TempDir::new().unwrap();
    let history = populated(source.path());
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("laqta.zip");

    export_archive(source.path(), &zip_path, Some(&history)).unwrap();
    assert!(zip_path.is_file());
    assert!(!out.path().join("laqta.zip.part").exists(), "the .part file is renamed away");
    assert_eq!(
        entry_names(&zip_path),
        BTreeSet::from([
            "backups/settings-20261101-090000.json".to_owned(),
            "backups/weekly/settings-2026-W44.json".to_owned(),
            "history.db".to_owned(),
            "settings.json".to_owned(),
            "uploaders/imgur.sxcu".to_owned(),
        ]),
        "a non-uploader file in uploaders/ is left out"
    );

    let fresh = TempDir::new().unwrap();
    let written = import_archive(&zip_path, fresh.path()).unwrap();

    assert_eq!(fs::read_to_string(fresh.path().join("settings.json")).unwrap(), SETTINGS);
    assert_eq!(fs::read_to_string(fresh.path().join("uploaders/imgur.sxcu")).unwrap(), r#"{"Name":"imgur"}"#);
    assert_eq!(fs::read_to_string(fresh.path().join("backups/weekly/settings-2026-W44.json")).unwrap(), r#"{"old":true}"#);
    assert!(!fresh.path().join("uploaders/notes.txt").exists());
    assert!(!fresh.path().join("backups/settings-before-import.json").exists(), "nothing existed to back up");
    assert_eq!(written.len(), 5);
    assert!(written.contains(&fresh.path().join("history.db")));

    let restored = History::open(&fresh.path().join("history.db")).unwrap();
    assert_eq!(all(&restored), all(&history), "the checkpointed database opens with the same rows");
    assert_eq!(all(&restored).len(), 2);
}

#[test]
fn export_without_a_history_zips_the_files_as_they_are() {
    let source = TempDir::new().unwrap();
    fs::write(source.path().join("settings.json"), SETTINGS).unwrap();
    // Dropping the connection checkpoints the log, so the file alone holds the row.
    History::open(&source.path().join("history.db")).unwrap().add(&new_item(4, "closed.png")).unwrap();
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("no-history.zip");

    export_archive(source.path(), &zip_path, None).unwrap();
    assert_eq!(entry_names(&zip_path), BTreeSet::from(["history.db".to_owned(), "settings.json".to_owned()]));

    let fresh = TempDir::new().unwrap();
    import_archive(&zip_path, fresh.path()).unwrap();
    let restored = History::open(&fresh.path().join("history.db")).unwrap();
    assert_eq!(all(&restored).len(), 1);
}

#[test]
fn an_archived_pre_import_backup_never_replaces_the_one_just_made() {
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("earlier-import.zip");
    write_zip(
        &zip_path,
        &[("settings.json", r#"{"from":"A"}"#.as_bytes()), ("backups/settings-before-import.json", r#"{"from":"X"}"#.as_bytes())],
    );

    let target = TempDir::new().unwrap();
    fs::write(target.path().join("settings.json"), r#"{"from":"Y"}"#).unwrap();
    let written = import_archive(&zip_path, target.path()).unwrap();

    assert_eq!(fs::read_to_string(target.path().join("settings.json")).unwrap(), r#"{"from":"A"}"#);
    assert_eq!(
        fs::read_to_string(target.path().join("backups/settings-before-import.json")).unwrap(),
        r#"{"from":"Y"}"#,
        "the target's original settings survive in the backup"
    );
    assert_eq!(written.len(), 2);
}

#[test]
fn import_keeps_the_settings_it_replaces() {
    let source = TempDir::new().unwrap();
    fs::write(source.path().join("settings.json"), r#"{"from":"archive"}"#).unwrap();
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("laqta.zip");
    export_archive(source.path(), &zip_path, None).unwrap();

    let target = TempDir::new().unwrap();
    fs::write(target.path().join("settings.json"), r#"{"from":"target"}"#).unwrap();
    let written = import_archive(&zip_path, target.path()).unwrap();

    assert_eq!(fs::read_to_string(target.path().join("settings.json")).unwrap(), r#"{"from":"archive"}"#);
    assert_eq!(fs::read_to_string(target.path().join("backups/settings-before-import.json")).unwrap(), r#"{"from":"target"}"#);
    assert_eq!(written[0], target.path().join("backups/settings-before-import.json"), "the backup is written first");
}

#[test]
fn import_drops_a_stale_history_log_that_would_corrupt_the_imported_database() {
    let source = TempDir::new().unwrap();
    let history = populated(source.path());
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("laqta.zip");
    export_archive(source.path(), &zip_path, Some(&history)).unwrap();

    let target = TempDir::new().unwrap();
    fs::write(target.path().join("history.db-wal"), "stale log from the old database").unwrap();
    fs::write(target.path().join("history.db-shm"), "stale index").unwrap();
    import_archive(&zip_path, target.path()).unwrap();

    assert!(!target.path().join("history.db-wal").exists());
    assert!(!target.path().join("history.db-shm").exists());
    let restored = History::open(&target.path().join("history.db")).unwrap();
    assert_eq!(all(&restored), all(&history));
}

#[test]
fn unknown_entries_are_skipped() {
    let out = TempDir::new().unwrap();
    let zip_path = out.path().join("mixed.zip");
    write_zip(&zip_path, &[("settings.json", SETTINGS.as_bytes()), ("random.bin", b"x"), ("uploaders/nested/a.sxcu", b"{}")]);

    let target = TempDir::new().unwrap();
    let written = import_archive(&zip_path, target.path()).unwrap();
    assert_eq!(written, vec![target.path().join("settings.json")]);
    assert!(!target.path().join("random.bin").exists());
    assert!(!target.path().join("uploaders").exists());
}

#[test]
fn an_entry_that_leaves_the_folder_fails_the_import_and_writes_nothing() {
    let hostile = ["../evil", "backups/../../evil", "backups/..\\evil", "/absolute-evil", "C:/evil", "backups/a\\..\\..\\evil"];
    for name in hostile {
        let out = TempDir::new().unwrap();
        let zip_path = out.path().join("hostile.zip");
        // A legitimate settings entry comes first, so a partial import would show up.
        write_zip(&zip_path, &[("settings.json", SETTINGS.as_bytes()), (name, b"pwned")]);

        let root = TempDir::new().unwrap();
        let target = root.path().join("data");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("settings.json"), "original").unwrap();

        let error = import_archive(&zip_path, &target).unwrap_err();
        assert!(matches!(error, LibraryError::Io(_)), "{name}: {error}");
        assert_eq!(fs::read_to_string(target.join("settings.json")).unwrap(), "original", "{name}: nothing was written");
        assert!(!target.join("backups").exists(), "{name}: no backup was written");
        assert!(!root.path().join("evil").exists(), "{name}: nothing escaped the data folder");
    }
}

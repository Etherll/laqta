#![allow(clippy::unwrap_used)]

use std::io::{Cursor, Write};
use std::path::PathBuf;

use laqta_domain::Settings;
use laqta_import::{ImportError, import_backup, import_dir};
use zip::write::SimpleFileOptions;

const APPLICATION: &str = include_str!("fixtures/ApplicationConfig.json");
const HOTKEYS: &str = include_str!("fixtures/HotkeysConfig.json");
const UPLOADERS: &str = include_str!("fixtures/UploadersConfig.json");
const HISTORY: &str = include_str!("fixtures/History.json");

fn base() -> Settings {
    Settings::default_for(PathBuf::from("/pictures"))
}

/// Writes a `.sxb` zip with the given entries into a temporary folder and returns its path.
fn backup_file(dir: &tempfile::TempDir, entries: &[(&str, &str)]) -> PathBuf {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, text) in entries {
        writer.start_file(*name, SimpleFileOptions::default()).unwrap();
        writer.write_all(text.as_bytes()).unwrap();
    }
    let bytes = writer.finish().unwrap().into_inner();
    let path = dir.path().join("backup.sxb");
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_backup_at_the_root_imports_like_the_folder() {
    let dir = tempfile::tempdir().unwrap();
    let sxb = backup_file(
        &dir,
        &[
            ("ApplicationConfig.json", APPLICATION),
            ("HotkeysConfig.json", HOTKEYS),
            ("UploadersConfig.json", UPLOADERS),
            ("History.json", HISTORY),
        ],
    );
    let folder = tempfile::tempdir().unwrap();
    for (name, text) in [
        ("ApplicationConfig.json", APPLICATION),
        ("HotkeysConfig.json", HOTKEYS),
        ("UploadersConfig.json", UPLOADERS),
        ("History.json", HISTORY),
    ] {
        std::fs::write(folder.path().join(name), text).unwrap();
    }

    let from_backup = import_backup(&sxb, base()).unwrap();
    let from_folder = import_dir(folder.path(), base()).unwrap();
    assert_eq!(from_backup, from_folder);
    assert_eq!(from_backup.settings.workflows.len(), 5);
}

#[test]
fn files_in_a_folder_inside_the_backup_are_found() {
    let dir = tempfile::tempdir().unwrap();
    let sxb = backup_file(
        &dir,
        &[("Backup/ApplicationConfig.json", APPLICATION), ("Backup/HotkeysConfig.json", HOTKEYS), ("Backup/History.json", HISTORY)],
    );
    let result = import_backup(&sxb, base()).unwrap();
    assert_eq!(result.settings.workflows.len(), 5);
    assert_eq!(result.history_json.as_deref(), Some(HISTORY));
    assert!(result.settings.hosts.is_empty(), "no uploaders file, so the base hosts stay");
}

#[test]
fn a_backup_without_configuration_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let sxb = backup_file(&dir, &[("readme.txt", "nothing")]);
    assert!(matches!(import_backup(&sxb, base()), Err(ImportError::Invalid(_))));
}

#[test]
fn a_file_that_is_not_a_zip_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("broken.sxb");
    std::fs::write(&path, b"plain text, not a zip").unwrap();
    assert!(matches!(import_backup(&path, base()), Err(ImportError::Zip { .. })));
}

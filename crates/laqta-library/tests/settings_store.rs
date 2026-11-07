#![allow(clippy::unwrap_used)]

use std::fs;
use std::path::PathBuf;

use jiff::civil::{DateTime, date};
use laqta_library::{LibraryError, Migration, SettingsStore};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
struct Prefs {
    name: String,
    count: u32,
}

fn prefs(name: &str, count: u32) -> Prefs {
    Prefs { name: name.to_owned(), count }
}

/// Times on 2026-11-07 at the given hour.
fn at(hour: i8) -> DateTime {
    date(2026, 11, 7).at(hour, 0, 0, 0)
}

fn store_in(dir: &tempfile::TempDir) -> SettingsStore {
    SettingsStore::new(dir.path().to_path_buf())
}

fn read_json(path: &PathBuf) -> Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn backup_names(dir: &tempfile::TempDir) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(dir.path().join("backups"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json") && name.starts_with("settings-"))
        .collect();
    names.sort();
    names
}

fn v1_to_v2(mut value: Value) -> Value {
    if let Some(label) = value.as_object_mut().and_then(|object| object.remove("label")) {
        value["name"] = label;
    }
    value
}

fn v2_to_v3(mut value: Value) -> Value {
    if let Some(object) = value.as_object_mut() {
        object.entry("count").or_insert(json!(0));
    }
    value
}

const MIGRATIONS: [Migration; 2] = [v1_to_v2, v2_to_v3];

#[test]
fn missing_file_loads_default() {
    let dir = tempfile::tempdir().unwrap();
    let loaded: Prefs = store_in(&dir).load(1, &[]).unwrap();
    assert_eq!(loaded, Prefs::default());
    assert!(!store_in(&dir).path().exists());
}

#[test]
fn save_then_load_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    store.save(&prefs("snap", 3), 1, at(10)).unwrap();

    let loaded: Prefs = store.load(1, &[]).unwrap();
    assert_eq!(loaded, prefs("snap", 3));
    assert_eq!(read_json(&store.path())["version"], 1);
    assert!(!dir.path().join("settings.json.part").exists());
}

#[test]
fn version_one_file_is_migrated_to_current() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    fs::write(store.path(), r#"{"version": 1, "label": "old"}"#).unwrap();

    let loaded: Prefs = store.load(3, &MIGRATIONS).unwrap();
    assert_eq!(loaded, prefs("old", 0));

    store.save(&loaded, 3, at(10)).unwrap();
    assert_eq!(read_json(&store.path())["version"], 3);
    assert_eq!(read_json(&store.path())["name"], "old");
}

#[test]
fn file_without_version_counts_as_version_one() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    fs::write(store.path(), r#"{"label": "bare"}"#).unwrap();

    let loaded: Prefs = store.load(3, &MIGRATIONS).unwrap();
    assert_eq!(loaded, prefs("bare", 0));
}

#[test]
fn newer_version_is_an_error_and_is_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    fs::write(store.path(), r#"{"version": 9, "name": "future", "count": 1}"#).unwrap();

    let error = store.load::<Prefs>(3, &MIGRATIONS).unwrap_err();
    assert!(matches!(error, LibraryError::TooNew { found: 9, supported: 3 }), "{error}");
    assert!(store.path().exists());
    assert!(!dir.path().join("settings.json.corrupt-1").exists());
}

#[test]
fn corrupt_file_falls_back_to_newest_backup() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    store.save(&prefs("first", 1), 1, at(10)).unwrap();
    store.save(&prefs("second", 2), 1, at(11)).unwrap();
    // The backup holds the file as it was before the second save.
    fs::write(store.path(), "{ this is not json").unwrap();

    let loaded: Prefs = store.load(1, &[]).unwrap();
    assert_eq!(loaded, prefs("first", 1));
    assert!(!dir.path().join("settings.json.corrupt-1").exists());
}

#[test]
fn corrupt_file_with_no_usable_backup_becomes_default() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    fs::write(store.path(), "not json at all").unwrap();

    let loaded: Prefs = store.load(1, &[]).unwrap();
    assert_eq!(loaded, Prefs::default());
    assert!(!store.path().exists());
    assert_eq!(fs::read_to_string(dir.path().join("settings.json.corrupt-1")).unwrap(), "not json at all");

    // The next free number is used when the same thing happens again.
    fs::write(store.path(), "broken again").unwrap();
    store.load::<Prefs>(1, &[]).unwrap();
    assert!(dir.path().join("settings.json.corrupt-2").exists());
}

#[test]
fn shape_mismatch_counts_as_corrupt() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    fs::write(store.path(), r#"{"version": 1, "count": "many"}"#).unwrap();

    let loaded: Prefs = store.load(1, &[]).unwrap();
    assert_eq!(loaded, Prefs::default());
    assert!(dir.path().join("settings.json.corrupt-1").exists());
}

#[test]
fn backups_rotate_to_the_newest_five() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    // Saves at hours 0 to 6. Hour 0 has no file to back up, so six backups are made.
    for hour in 0..7 {
        store.save(&prefs("rotating", hour as u32), 1, at(hour)).unwrap();
    }

    assert_eq!(
        backup_names(&dir),
        [
            "settings-20261107-020000.json",
            "settings-20261107-030000.json",
            "settings-20261107-040000.json",
            "settings-20261107-050000.json",
            "settings-20261107-060000.json",
        ]
    );
    // The backup taken at hour 6 holds what was saved at hour 5.
    let newest = read_json(&dir.path().join("backups/settings-20261107-060000.json"));
    assert_eq!(newest["count"], 5);
}

#[test]
fn weekly_backup_is_written_once_per_iso_week() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    let weekly = dir.path().join("backups/weekly");

    // 2026-11-02 is a Monday and 2026-11-08 a Sunday, so both are in ISO week 45.
    store.save(&prefs("v0", 0), 1, date(2026, 11, 2).at(10, 0, 0, 0)).unwrap();
    store.save(&prefs("v1", 1), 1, date(2026, 11, 2).at(11, 0, 0, 0)).unwrap();
    store.save(&prefs("v2", 2), 1, date(2026, 11, 8).at(9, 0, 0, 0)).unwrap();
    assert_eq!(fs::read_dir(&weekly).unwrap().count(), 1);
    assert_eq!(read_json(&weekly.join("settings-2026-W45.json"))["count"], 0);

    store.save(&prefs("v3", 3), 1, date(2026, 11, 9).at(9, 0, 0, 0)).unwrap();
    assert_eq!(read_json(&weekly.join("settings-2026-W46.json"))["count"], 2);
    assert_eq!(fs::read_dir(&weekly).unwrap().count(), 2);
}

#[test]
fn weekly_name_uses_the_iso_week_year() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    // 2027-01-01 is a Friday in ISO week 53 of 2026.
    store.save(&prefs("old", 1), 1, date(2026, 12, 31).at(9, 0, 0, 0)).unwrap();
    store.save(&prefs("new", 2), 1, date(2027, 1, 1).at(9, 0, 0, 0)).unwrap();
    assert!(dir.path().join("backups/weekly/settings-2026-W53.json").exists());
    assert!(!dir.path().join("backups/weekly/settings-2027-W01.json").exists());
}

#[test]
fn saving_a_non_object_is_an_error_and_writes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let store = store_in(&dir);
    assert!(store.save(&5_u32, 1, at(10)).is_err());
    assert!(!store.path().exists());
}

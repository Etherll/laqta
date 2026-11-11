//! Checks for ShareX settings the importer drops or maps wrongly. Field names are the ones ShareX writes
//! in `TaskSettings`, `UploadersConfig` and `AmazonS3Settings`.
#![allow(clippy::unwrap_used)]

use std::path::PathBuf;

use laqta_domain::Settings;
use laqta_domain::hosts::HostKind;
use laqta_import::import_dir;

fn import(files: &[(&str, &str)]) -> laqta_import::ImportResult {
    let dir = tempfile::tempdir().unwrap();
    for (name, text) in files {
        std::fs::write(dir.path().join(name), text).unwrap();
    }
    import_dir(dir.path(), Settings::default_for(PathBuf::from("/pictures"))).unwrap()
}

/// ShareX keeps watch folders in `DefaultTaskSettings.WatchFolderList` (and per hotkey). They are
/// neither imported nor reported.
#[test]
fn audit_bug_watch_folders_dropped_silently() {
    let app = r#"{"DefaultTaskSettings": {"WatchFolderEnabled": true,
        "WatchFolderList": [{"FolderPath": "/home/u/drop", "Filter": "*.png", "IncludeSubdirectories": true, "MoveFilesToScreenshotsFolder": false}]}}"#;
    let r = import(&[("ApplicationConfig.json", app)]);
    let reported = r.warnings.iter().any(|w| w.to_ascii_lowercase().contains("watch"));
    assert!(!r.settings.watch_folders.is_empty() || reported, "watch folder neither imported nor warned: {:?}", r.warnings);
}

/// ShareX uses `FTPSelectedImage/Text/File` to pick the account. The importer always takes the first.
#[test]
fn audit_bug_ftp_selected_account_ignored() {
    let up = r#"{"FTPSelectedImage": 1, "FTPSelectedFile": 1, "FTPSelectedText": 1, "FTPAccountList": [
        {"Name": "old", "Protocol": "FTP", "Host": "old.example", "Port": 21},
        {"Name": "used", "Protocol": "FTP", "Host": "used.example", "Port": 21}]}"#;
    let r = import(&[("UploadersConfig.json", up)]);
    let ftp = r.settings.hosts.iter().find(|h| h.kind == HostKind::Ftp).unwrap();
    assert_eq!(ftp.values.get("host").map(String::as_str), Some("used.example"), "warnings: {:?}", r.warnings);
}

/// `CustomDomain` only applies when `UseCustomCNAME` is on.
#[test]
fn audit_bug_s3_custom_domain_ignores_switch() {
    let up = r#"{"AmazonS3Settings": {"AccessKeyID": "AK", "Bucket": "b", "Region": "us-east-1",
        "UseCustomCNAME": false, "CustomDomain": "cdn.stale.example"}}"#;
    let r = import(&[("UploadersConfig.json", up)]);
    let s3 = r.settings.hosts.iter().find(|h| h.kind == HostKind::S3).unwrap();
    assert_eq!(s3.values.get("public_url_base"), None, "links would use a custom domain that is switched off");
}

/// ShareX's `OverrideScreenshotsFolder` on a hotkey changes where that task saves. Dropped, unreported.
#[test]
fn audit_bug_override_screenshots_folder_dropped_silently() {
    let hk = r#"{"Hotkeys": [{"HotkeyInfo": {"Hotkey": "F9", "Win": false}, "TaskSettings": {"Job": "PrintScreen",
        "Description": "Shot to D", "OverrideScreenshotsFolder": true, "ScreenshotsFolder": "/data/shots"}}]}"#;
    let r = import(&[("HotkeysConfig.json", hk)]);
    let reported = r.warnings.iter().any(|w| w.contains("ScreenshotsFolder") || w.to_ascii_lowercase().contains("folder"));
    assert!(reported, "per-task screenshots folder dropped without a warning: {:?}", r.warnings);
}

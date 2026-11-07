#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};

use laqta_domain::action::Action;
use laqta_domain::hosts::HostKind;
use laqta_domain::image::ImageFormat;
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks, FileExistsAction};
use laqta_domain::{Key, KeyChord, Modifiers, Settings};
use laqta_import::{ImportError, ImportResult, import_dir};

fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn base() -> Settings {
    Settings::default_for(PathBuf::from("/pictures"))
}

fn imported() -> ImportResult {
    import_dir(&fixtures(), base()).unwrap()
}

#[test]
fn hotkeys_become_workflows_with_actions_and_chords() {
    let result = imported();
    let workflows = &result.settings.workflows;
    assert_eq!(workflows.len(), 5);

    let actions: Vec<Action> = workflows.iter().map(|workflow| workflow.action).collect();
    assert_eq!(actions, vec![Action::PrintScreen, Action::RectangleRegion, Action::ActiveWindow, Action::ScreenRecorderGif, Action::Ocr]);

    let chords: Vec<Option<KeyChord>> = workflows.iter().map(|workflow| workflow.hotkey).collect();
    assert_eq!(
        chords,
        vec![
            Some(KeyChord::new(Modifiers::empty(), Key::PrintScreen)),
            Some(KeyChord::new(Modifiers::CTRL, Key::PrintScreen)),
            Some(KeyChord::new(Modifiers::ALT, Key::PrintScreen)),
            Some(KeyChord::new(Modifiers::CTRL | Modifiers::SHIFT, Key::Digit5)),
            Some(KeyChord::new(Modifiers::SUPER | Modifiers::CTRL | Modifiers::SHIFT, Key::S)),
        ]
    );

    let names: Vec<&str> = workflows.iter().map(|workflow| workflow.name.as_str()).collect();
    assert_eq!(names[0], "Capture entire screen");
    assert!(workflows.iter().all(|workflow| workflow.enabled));
    let ids: Vec<u32> = workflows.iter().map(|workflow| workflow.id.0).collect();
    assert_eq!(ids, vec![1, 2, 3, 4, 5]);
}

#[test]
fn sections_without_an_override_have_no_patch() {
    let result = imported();
    assert!(result.settings.workflows[0].patch.is_empty());
    assert!(result.settings.workflows[1].patch.is_empty());
    assert!(result.settings.workflows[4].patch.is_empty());
}

#[test]
fn use_default_false_becomes_a_patch_section() {
    let result = imported();
    let patch = &result.settings.workflows[2].patch;
    assert_eq!(patch.overridden_sections(), vec!["after_capture", "upload", "image", "naming"]);

    assert_eq!(
        patch.after_capture,
        Some(
            AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD
                | AfterCaptureTasks::SHOW_BEFORE_UPLOAD_WINDOW
                | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST
        )
    );

    let image = patch.image.as_ref().unwrap();
    assert_eq!(image.format, ImageFormat::Jpeg);
    assert_eq!(image.jpeg_quality, 75);
    assert_eq!(image.auto_jpeg_above_bytes, None);
    assert_eq!(image.file_exists, FileExistsAction::Overwrite);

    let upload = patch.upload.as_ref().unwrap();
    assert_eq!(upload.image_destination, "custom:My host");
    assert_eq!(upload.text_destination, "sxcu:pastebin");
    assert_eq!(upload.file_destination, "ftp");
    assert_eq!(upload.url_shortener, "sxcu:is-gd");

    let naming = patch.naming.as_ref().unwrap();
    assert_eq!(naming.pattern, "%y%mo%d_%h%mi%s");
    assert_eq!(naming.window_pattern, "%pn_%y%mo%d_%h%mi%s");
}

#[test]
fn override_patch_resolves_against_the_effective_settings() {
    let result = imported();
    let effective = result.settings.task_settings(Some(result.settings.workflows[2].id));
    assert_eq!(effective.image.format, ImageFormat::Jpeg);
    assert_eq!(effective.upload.image_destination, "custom:My host");
    assert_eq!(effective.after_upload, AfterUploadTasks::COPY_URL_TO_CLIPBOARD | AfterUploadTasks::SHOW_QR_CODE);
}

#[test]
fn default_task_settings_are_mapped() {
    let result = imported();
    let defaults = &result.settings.defaults;
    assert_eq!(defaults.after_capture, AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD);
    assert_eq!(defaults.after_upload, AfterUploadTasks::COPY_URL_TO_CLIPBOARD | AfterUploadTasks::SHOW_QR_CODE);

    assert_eq!(defaults.upload.image_destination, "imgur");
    assert_eq!(defaults.upload.text_destination, "sxcu:pastebin");
    assert_eq!(defaults.upload.file_destination, "dropbox");
    assert_eq!(defaults.upload.url_shortener, "bitly");

    assert_eq!(defaults.image.format, ImageFormat::Png);
    assert_eq!(defaults.image.jpeg_quality, 90);
    assert_eq!(defaults.image.auto_jpeg_above_bytes, Some(2048 * 1024));
    assert_eq!(defaults.image.file_exists, FileExistsAction::AppendNumber);

    assert_eq!(defaults.capture.delay_ms, 500);
    assert!(defaults.capture.show_cursor);
    assert!(!defaults.capture.transparent_window);

    assert!(defaults.general.play_sound_after_capture);
    assert!(!defaults.general.play_sound_after_upload);

    assert_eq!(defaults.naming.pattern, "%y-%mo-%d_%h-%mi-%s");
    assert_eq!(defaults.naming.window_pattern, "%pn_%y-%mo-%d_%h-%mi-%s");
}

#[test]
fn application_settings_are_mapped() {
    let result = imported();
    assert_eq!(result.settings.screenshots_dir, PathBuf::from("D:\\Screenshots"));
    assert!(!result.settings.history.enabled);
}

#[test]
fn effect_presets_are_read_and_known_effects_kept() {
    let result = imported();
    assert_eq!(result.presets.len(), 1);
    let preset = &result.presets[0];
    assert_eq!(preset.name, "Warm");
    assert_eq!(preset.effects.len(), 2);
    assert_eq!(preset.effects[0].effect.name(), "Brightness");
    assert!(preset.effects[0].enabled);
    assert_eq!(preset.effects[1].effect.name(), "Sepia");
    assert!(!preset.effects[1].enabled);
    assert_eq!(result.settings.effect_presets, result.presets);
}

#[test]
fn custom_uploaders_hosts_and_secrets_are_mapped() {
    let result = imported();
    assert_eq!(result.custom_uploaders.len(), 1);
    let uploader: serde_json::Value = serde_json::from_str(&result.custom_uploaders[0]).unwrap();
    assert_eq!(uploader["Name"], "My host");
    assert_eq!(uploader["RequestURL"], "https://uploads.example.test/api/upload");

    let hosts = &result.settings.hosts;
    assert_eq!(hosts.iter().map(|host| host.kind).collect::<Vec<_>>(), vec![HostKind::Ftp, HostKind::S3, HostKind::Bitly]);

    let ftp = &hosts[0];
    assert_eq!(ftp.get("host"), "ftp.example.test");
    assert_eq!(ftp.get("port"), "21");
    assert_eq!(ftp.get("username"), "alice");
    assert_eq!(ftp.get("directory"), "shots");
    assert_eq!(ftp.get("url_base"), "https://files.example.test/shots");
    assert_eq!(ftp.get("tls"), "none");

    let s3 = &hosts[1];
    assert_eq!(s3.get("bucket"), "shots");
    assert_eq!(s3.get("region"), "eu-west-1");
    assert_eq!(s3.get("endpoint"), "s3.example.test");
    assert_eq!(s3.get("path_prefix"), "laqta/");
    assert_eq!(s3.get("access_key_id"), "AKIAEXAMPLE");
    assert_eq!(s3.get("public_url_base"), "https://cdn.example.test");
    assert!(!s3.flag("use_path_style"));
    assert!(s3.flag("acl_public_read"));

    assert!(result.secrets.contains(&("laqta/ftp/password".to_owned(), "ftp-pass-plain".to_owned())));
    assert!(result.secrets.contains(&("laqta/bitly/access_token".to_owned(), "bitly-token-plain".to_owned())));
    assert_eq!(result.secrets.len(), 2, "the DPAPI-encrypted S3 secret must not be returned: {:?}", result.secrets);
}

#[test]
fn history_text_is_returned_verbatim() {
    let result = imported();
    let expected = std::fs::read_to_string(fixtures().join("History.json")).unwrap();
    assert_eq!(result.history_json.as_deref(), Some(expected.as_str()));
}

#[test]
fn unmappable_items_are_reported() {
    let result = imported();
    let warnings = result.warnings.join("\n");
    assert!(warnings.contains("SomeFutureEffect"), "{warnings}");
    assert!(warnings.contains("FTP account `Backup` skipped"), "{warnings}");
    assert!(warnings.contains("DPAPI"), "{warnings}");
    assert!(warnings.contains("the S3 secret access key"), "{warnings}");
}

#[test]
fn an_empty_folder_is_an_error() {
    let dir = tempfile::tempdir().unwrap();
    let error = import_dir(dir.path(), base()).unwrap_err();
    assert!(matches!(error, ImportError::Invalid(_)), "{error}");
}

#[test]
fn bad_entries_are_skipped_and_reported() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("HotkeysConfig.json"),
        r#"{"Hotkeys": [
            {"HotkeyInfo": {"Hotkey": "Frob, Control"}, "TaskSettings": {"Job": "Ruler"}},
            {"HotkeyInfo": {"Hotkey": "F9"}, "TaskSettings": {"Job": "NoSuchTask"}},
            {"HotkeyInfo": {"Hotkey": "F10"}, "TaskSettings": {"Job": "Ruler"}},
            {"HotkeyInfo": {"Hotkey": "F10"}, "TaskSettings": {"Job": "Ruler"}}
        ]}"#,
    )
    .unwrap();
    let result = import_dir(dir.path(), base()).unwrap();
    assert_eq!(result.settings.workflows.len(), 3);
    assert_eq!(result.settings.workflows[0].hotkey, None);
    let warnings = result.warnings.join("\n");
    assert!(warnings.contains("Frob"), "{warnings}");
    assert!(warnings.contains("NoSuchTask"), "{warnings}");
    assert!(warnings.contains("F10") && warnings.contains("several tasks"), "{warnings}");
}

#[test]
fn a_byte_order_mark_is_ignored() {
    let dir = tempfile::tempdir().unwrap();
    let text = std::fs::read_to_string(fixtures().join("HotkeysConfig.json")).unwrap();
    std::fs::write(dir.path().join("HotkeysConfig.json"), format!("\u{feff}{text}")).unwrap();
    let result = import_dir(dir.path(), base()).unwrap();
    assert_eq!(result.settings.workflows.len(), 5);
}

#[test]
fn a_malformed_file_names_itself() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("ApplicationConfig.json"), "{ not json").unwrap();
    let error = import_dir(dir.path(), base()).unwrap_err();
    assert!(error.to_string().contains("ApplicationConfig.json"), "{error}");
}

#[test]
fn ftps_tls_mode_and_sftp_key_path_come_from_the_account() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("UploadersConfig.json"),
        r#"{"FTPAccountList": [
            {"Name": "Secure", "Protocol": "FTPS", "Host": "ftps.example.test", "Port": 990, "Username": "carol",
             "Password": "", "FTPSEncryption": "Implicit", "SubFolderPath": "", "HttpHomePath": "", "Keypath": ""},
            {"Name": "Keys", "Protocol": "SFTP", "Host": "sftp.example.test", "Port": 22, "Username": "dave",
             "Password": "", "SubFolderPath": "up", "HttpHomePath": "", "Keypath": "/keys/id_ed25519"}
        ]}"#,
    )
    .unwrap();
    let result = import_dir(dir.path(), base()).unwrap();

    let ftp = result.settings.hosts.iter().find(|host| host.kind == HostKind::Ftp).unwrap();
    assert_eq!(ftp.get("host"), "ftps.example.test");
    assert_eq!(ftp.get("tls"), "implicit");

    let sftp = result.settings.hosts.iter().find(|host| host.kind == HostKind::Sftp).unwrap();
    assert_eq!(sftp.get("key_path"), "/keys/id_ed25519");
    assert_eq!(sftp.get("directory"), "up");

    assert!(result.secrets.is_empty(), "empty passwords are not secrets: {:?}", result.secrets);
}

#[test]
fn a_custom_destination_with_an_out_of_range_selection_keeps_the_default() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("UploadersConfig.json"),
        r#"{"CustomUploadersList": [{"Name": "Only", "RequestURL": "https://example.test"}], "CustomFileUploaderSelected": 4}"#,
    )
    .unwrap();
    std::fs::write(dir.path().join("ApplicationConfig.json"), r#"{"DefaultTaskSettings": {"FileDestination": "CustomFileUploader"}}"#)
        .unwrap();
    let result = import_dir(dir.path(), base()).unwrap();
    assert_eq!(result.custom_uploaders.len(), 1);
    assert_eq!(result.settings.defaults.upload.file_destination, "sxcu:uguu");
    assert!(result.warnings.iter().any(|warning| warning.contains("no custom uploader is selected")), "{:?}", result.warnings);
}

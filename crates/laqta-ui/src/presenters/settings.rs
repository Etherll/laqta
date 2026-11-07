//! Settings shown as differences from defaults, option labels, and the name-pattern preview.

use laqta_domain::hotkey::{KeyChord, find_conflicts};
use laqta_domain::naming::{self, NameContext};
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks, TaskSettingsPatch};
use laqta_domain::{ImageFormat, Settings};
use laqta_ports::HostInfo;

/// One after-capture option for the settings page. `available` is false for steps whose feature
/// lands in a later milestone; the UI shows them disabled.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TaskOption {
    pub flag: AfterCaptureTasks,
    pub label: &'static str,
    pub available: bool,
}

pub fn after_capture_options() -> Vec<TaskOption> {
    let o = |flag, label, available| TaskOption { flag, label, available };
    vec![
        o(AfterCaptureTasks::SAVE_IMAGE_TO_FILE, "Save image to file", true),
        o(AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD, "Copy image to clipboard", true),
        o(AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE, "Save thumbnail image to file", true),
        o(AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD, "Copy file to clipboard", true),
        o(AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD, "Copy file path to clipboard", true),
        o(AfterCaptureTasks::SHOW_IN_EXPLORER, "Show file in file manager", true),
        o(AfterCaptureTasks::DELETE_FILE, "Delete file locally", true),
        o(AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG, "Save image to file as…", false),
        o(AfterCaptureTasks::SHOW_QUICK_TASK_MENU, "Show quick task menu", false),
        o(AfterCaptureTasks::SHOW_AFTER_CAPTURE_WINDOW, "Show after capture window", false),
        o(AfterCaptureTasks::ANNOTATE_IMAGE, "Open in image editor", false),
        o(AfterCaptureTasks::BEAUTIFY_IMAGE, "Beautify image", false),
        o(AfterCaptureTasks::ADD_IMAGE_EFFECTS, "Add image effects", false),
        o(AfterCaptureTasks::OPEN_IN_PHOTO_EDITOR, "Open in photo editor", false),
        o(AfterCaptureTasks::PIN_TO_SCREEN, "Pin to screen", false),
        o(AfterCaptureTasks::SEND_IMAGE_TO_PRINTER, "Print image", false),
        o(AfterCaptureTasks::PERFORM_ACTIONS, "Perform actions", false),
        o(AfterCaptureTasks::SCAN_QR_CODE, "Scan QR code", false),
        o(AfterCaptureTasks::DO_OCR, "Recognize text (OCR)", false),
        o(AfterCaptureTasks::SHOW_BEFORE_UPLOAD_WINDOW, "Show before upload window", false),
        o(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST, "Upload image to host", false),
        o(AfterCaptureTasks::ANALYZE_IMAGE, "Analyze image", false),
        o(AfterCaptureTasks::REMOVE_BACKGROUND, "Remove background", false),
    ]
}

pub fn after_upload_options() -> Vec<(AfterUploadTasks, &'static str)> {
    vec![
        (AfterUploadTasks::COPY_URL_TO_CLIPBOARD, "Copy URL to clipboard"),
        (AfterUploadTasks::USE_URL_SHORTENER, "Shorten URL"),
        (AfterUploadTasks::SHARE_URL, "Share URL"),
        (AfterUploadTasks::OPEN_URL, "Open URL"),
        (AfterUploadTasks::SHOW_QR_CODE, "Show QR code"),
        (AfterUploadTasks::SHOW_AFTER_UPLOAD_WINDOW, "Show after upload window"),
    ]
}

/// Copied-text templates (upload-info tokens) offered as presets.
pub const CLIPBOARD_FORMATS: [(&str, &str); 6] = [
    ("URL", "$url"),
    ("Short URL", "$shorturl"),
    ("Markdown", "![$filenamenoext]($url)"),
    ("HTML", "<a href=\"$url\"><img src=\"$thumbnailurl\" alt=\"$filename\"/></a>"),
    ("Forum (BBCode)", "[url=$url][img]$thumbnailurl[/img][/url]"),
    ("Wiki", "[$url $filename]"),
];

/// The URL sharing services (`laqta-upload` sharer ids).
pub const SHARERS: [(&str, &str); 10] = [
    ("email", "Email"),
    ("pushbullet", "Pushbullet"),
    ("facebook", "Facebook"),
    ("reddit", "Reddit"),
    ("pinterest", "Pinterest"),
    ("tumblr", "Tumblr"),
    ("linkedin", "LinkedIn"),
    ("vk", "VK"),
    ("google-lens", "Google Lens"),
    ("bing-visual-search", "Bing Visual Search"),
];

/// Shorteners: the catalog's shortener entries plus Bitly when an account exists.
pub fn shortener_choices(destinations: &[(String, String)], hosts: &[laqta_domain::hosts::HostAccount]) -> Vec<(String, String)> {
    let _ = destinations;
    let mut out: Vec<(String, String)> = SHORTENERS.iter().map(|(id, n)| (id.to_string(), n.to_string())).collect();
    if hosts.iter().any(|h| h.kind == laqta_domain::hosts::HostKind::Bitly) {
        out.insert(0, ("bitly".into(), "Bitly".into()));
    }
    out
}

/// The catalog's shorteners (`laqta-upload/catalog`).
pub const SHORTENERS: [(&str, &str); 12] = [
    ("sxcu:is-gd", "is.gd"),
    ("sxcu:v-gd", "v.gd"),
    ("sxcu:tinyurl", "TinyURL"),
    ("sxcu:yourls", "YOURLS"),
    ("sxcu:polr", "Polr"),
    ("sxcu:kutt", "Kutt"),
    ("sxcu:zero-width", "Zero Width"),
    ("sxcu:turl", "TURL"),
    ("sxcu:2-gp", "2.gp"),
    ("sxcu:qr-net", "qr.net"),
    ("sxcu:vurl", "vurl"),
    ("sxcu:nlcm", "nlcm"),
];

pub fn format_label(format: ImageFormat) -> &'static str {
    match format {
        ImageFormat::Png => "PNG",
        ImageFormat::Jpeg => "JPEG",
        ImageFormat::Gif => "GIF",
        ImageFormat::Bmp => "BMP",
        ImageFormat::Tiff => "TIFF",
        ImageFormat::WebP => "WebP (lossless)",
        #[allow(unreachable_patterns)]
        _ => "Other",
    }
}

fn section_label(name: &str) -> &'static str {
    match name {
        "after_capture" => "After capture",
        "after_upload" => "After upload",
        "upload" => "Destinations",
        "general" => "General",
        "image" => "Image",
        "capture" => "Capture",
        "naming" => "File naming",
        "screenshots_dir" => "Screenshots folder",
        _ => "Other",
    }
}

/// Labels of the sections a workflow overrides, for "differences from defaults".
pub fn overridden_labels(patch: &TaskSettingsPatch) -> Vec<&'static str> {
    patch.overridden_sections().into_iter().map(section_label).collect()
}

/// A preview of a name pattern with fixed sample values.
pub fn name_preview(pattern: &str, now: jiff::civil::DateTime, host: &HostInfo) -> String {
    let mut n = 7u64;
    let mut rng = || {
        n = n.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        n >> 11
    };
    let mut ctx = NameContext {
        now,
        unix_seconds: 0,
        window_title: "Untitled - Editor",
        process_name: "editor",
        width: Some(1920),
        height: Some(1080),
        counter: 1,
        user_name: &host.user_name,
        user_login: &host.user_login,
        computer_name: &host.computer_name,
        random_file_lines: &[],
        rng: &mut rng,
    };
    naming::file_name(pattern, &mut ctx)
}

/// Text for each hotkey bound more than once, e.g. "Ctrl+PrintScreen is used by 2 workflows".
pub fn hotkey_conflicts(settings: &Settings) -> Vec<String> {
    let bindings: Vec<(KeyChord, usize)> =
        settings.workflows.iter().enumerate().filter(|(_, w)| w.enabled).filter_map(|(i, w)| w.hotkey.map(|h| (h, i))).collect();
    find_conflicts(&bindings).into_iter().map(|(chord, idx)| format!("{chord} is used by {} workflows", idx.len())).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_cover_all_flags() {
        let all = after_capture_options().iter().fold(AfterCaptureTasks::empty(), |a, o| a | o.flag);
        assert_eq!(all, AfterCaptureTasks::all());
    }

    #[test]
    fn preview_uses_pattern() {
        let now = jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0);
        assert_eq!(name_preview("Laqta_%y-%mo-%d", now, &HostInfo::default()), "Laqta_2026-11-07");
    }

    #[test]
    fn conflicts_detected() {
        let mut s = Settings::default_for("/p".into());
        assert!(hotkey_conflicts(&s).is_empty());
        s.workflows[1].hotkey = s.workflows[0].hotkey;
        assert_eq!(hotkey_conflicts(&s), vec!["PrintScreen is used by 2 workflows".to_string()]);
    }

    #[test]
    fn overridden_sections_labelled() {
        let patch = TaskSettingsPatch { image: Some(Default::default()), ..Default::default() };
        assert_eq!(overridden_labels(&patch), vec!["Image"]);
    }
}

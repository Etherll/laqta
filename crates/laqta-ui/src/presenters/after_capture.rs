//! A one-line summary of what happens to a capture: where it is saved and what runs after.

use std::path::Path;

use laqta_domain::Settings;
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks};

/// For example "Save to ~/Pictures/Laqta · copy to clipboard". `home` shortens the folder to `~`.
pub fn summary(settings: &Settings, home: Option<&Path>) -> String {
    let d = &settings.defaults;
    let flags = d.after_capture;
    let dir = d.screenshots_dir.as_deref().unwrap_or(&settings.screenshots_dir);
    let dir = match home.and_then(|h| dir.strip_prefix(h).ok()) {
        Some(rest) if rest.as_os_str().is_empty() => "~".to_string(),
        Some(rest) => format!("~/{}", rest.display()),
        None => dir.display().to_string(),
    };
    let mut steps = Vec::new();
    if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE) {
        steps.push(format!("save to {dir}"));
    } else if flags.contains(AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG) {
        steps.push("ask where to save".to_string());
    }
    let named = [
        (AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD, "copy to clipboard"),
        (AfterCaptureTasks::ANNOTATE_IMAGE, "open in the editor"),
        (AfterCaptureTasks::PIN_TO_SCREEN, "pin to screen"),
        (AfterCaptureTasks::DO_OCR, "read text"),
    ];
    steps.extend(named.iter().filter(|(f, _)| flags.contains(*f)).map(|(_, s)| s.to_string()));
    if flags.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST) {
        steps.push(if d.after_upload.contains(AfterUploadTasks::COPY_URL_TO_CLIPBOARD) {
            "upload and copy the link".to_string()
        } else {
            "upload".to_string()
        });
    }
    if steps.is_empty() {
        return "Nothing runs after a capture".to_string();
    }
    let mut text = steps.join(" · ");
    text[..1].make_ascii_uppercase();
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    // The expected text uses Unix paths.
    #[cfg(unix)]
    #[test]
    fn defaults_save_and_copy_with_home_shortened() {
        let s = Settings::default_for("/home/u/Pictures".into());
        assert_eq!(summary(&s, Some(Path::new("/home/u"))), "Save to ~/Pictures/Laqta · copy to clipboard");
    }

    #[cfg(unix)]
    #[test]
    fn upload_mentions_the_link_and_other_folders_stay_absolute() {
        let mut s = Settings::default_for("/data/shots".into());
        s.defaults.after_capture = AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST;
        assert_eq!(summary(&s, Some(Path::new("/home/u"))), "Save to /data/shots/Laqta · upload and copy the link");
    }

    #[test]
    fn no_tasks_says_so() {
        let mut s = Settings::default_for("/p".into());
        s.defaults.after_capture = AfterCaptureTasks::empty();
        assert_eq!(summary(&s, None), "Nothing runs after a capture");
    }
}

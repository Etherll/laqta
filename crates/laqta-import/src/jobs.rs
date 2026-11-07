//! ShareX's names for tasks (`HotkeyType`), after-task flags and destinations, as Laqta's values.
//! A name with no entry here is reported and skipped by the caller.

use std::ops::BitOr;

use laqta_domain::action::Action;
use laqta_domain::workflow::{AfterCaptureTasks, AfterUploadTasks};

/// `HotkeyType` names whose Laqta variant has another name. Other names match `Action` variants.
const ACTION_ALIASES: &[(&str, Action)] = &[
    ("ExitShareX", Action::ExitLaqta),
    ("ScreenRecorderGIF", Action::ScreenRecorderGif),
    ("ScreenRecorderGIFActiveWindow", Action::ScreenRecorderGifActiveWindow),
    ("ScreenRecorderGIFCustomRegion", Action::ScreenRecorderGifCustomRegion),
    ("StartScreenRecorderGIF", Action::StartScreenRecorderGif),
    ("GIFMaker", Action::GifMaker),
    ("GIFTrimmer", Action::GifTrimmer),
    ("QRCode", Action::QrCode),
    ("QRCodeDecodeFromScreen", Action::QrCodeDecodeFromScreen),
    ("QRCodeScanRegion", Action::QrCodeScanRegion),
    ("OCR", Action::Ocr),
    ("UploadURL", Action::UploadUrl),
    ("ShortenURL", Action::ShortenUrl),
    ("RemoveImageBackground", Action::RemoveBackground),
];

const AFTER_CAPTURE: &[(&str, AfterCaptureTasks)] = &[
    ("ShowQuickTaskMenu", AfterCaptureTasks::SHOW_QUICK_TASK_MENU),
    ("ShowAfterCaptureWindow", AfterCaptureTasks::SHOW_AFTER_CAPTURE_WINDOW),
    ("BeautifyImage", AfterCaptureTasks::BEAUTIFY_IMAGE),
    ("AddImageEffects", AfterCaptureTasks::ADD_IMAGE_EFFECTS),
    ("AnnotateImage", AfterCaptureTasks::ANNOTATE_IMAGE),
    ("OpenInPhotoEditor", AfterCaptureTasks::OPEN_IN_PHOTO_EDITOR),
    ("CopyImageToClipboard", AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD),
    ("PinToScreen", AfterCaptureTasks::PIN_TO_SCREEN),
    ("SendImageToPrinter", AfterCaptureTasks::SEND_IMAGE_TO_PRINTER),
    ("SaveImageToFile", AfterCaptureTasks::SAVE_IMAGE_TO_FILE),
    ("SaveImageToFileWithDialog", AfterCaptureTasks::SAVE_IMAGE_TO_FILE_WITH_DIALOG),
    ("SaveThumbnailImageToFile", AfterCaptureTasks::SAVE_THUMBNAIL_IMAGE_TO_FILE),
    ("PerformActions", AfterCaptureTasks::PERFORM_ACTIONS),
    ("CopyFileToClipboard", AfterCaptureTasks::COPY_FILE_TO_CLIPBOARD),
    ("CopyFilePathToClipboard", AfterCaptureTasks::COPY_FILE_PATH_TO_CLIPBOARD),
    ("ShowInExplorer", AfterCaptureTasks::SHOW_IN_EXPLORER),
    ("ScanQRCode", AfterCaptureTasks::SCAN_QR_CODE),
    ("DoOCR", AfterCaptureTasks::DO_OCR),
    ("ShowBeforeUploadWindow", AfterCaptureTasks::SHOW_BEFORE_UPLOAD_WINDOW),
    ("UploadImageToHost", AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST),
    ("DeleteFile", AfterCaptureTasks::DELETE_FILE),
    ("AnalyzeImage", AfterCaptureTasks::ANALYZE_IMAGE),
    ("RemoveBackground", AfterCaptureTasks::REMOVE_BACKGROUND),
];

const AFTER_UPLOAD: &[(&str, AfterUploadTasks)] = &[
    ("ShowAfterUploadWindow", AfterUploadTasks::SHOW_AFTER_UPLOAD_WINDOW),
    ("UseURLShortener", AfterUploadTasks::USE_URL_SHORTENER),
    ("ShareURL", AfterUploadTasks::SHARE_URL),
    ("CopyURLToClipboard", AfterUploadTasks::COPY_URL_TO_CLIPBOARD),
    ("OpenURL", AfterUploadTasks::OPEN_URL),
    ("ShowQRCode", AfterUploadTasks::SHOW_QR_CODE),
];

/// The custom uploader a custom destination name refers to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CustomKind {
    Image,
    Text,
    File,
    UrlShortener,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Destination {
    /// A destination id in the upload registry.
    Id(&'static str),
    /// The custom uploader ShareX selected for this slot. Its id is `custom:<name>`.
    Custom(CustomKind),
    /// ShareX's `FileUploader`: whatever the file destination is.
    SameAsFile,
}

const DESTINATIONS: &[(&str, Destination)] = &[
    ("Imgur", Destination::Id("imgur")),
    ("Dropbox", Destination::Id("dropbox")),
    ("GoogleDrive", Destination::Id("google-drive")),
    ("OneDrive", Destination::Id("onedrive")),
    ("AmazonS3", Destination::Id("s3")),
    ("FTP", Destination::Id("ftp")),
    ("SharedFolder", Destination::Id("shared-folder")),
    ("BITLY", Destination::Id("bitly")),
    ("CustomImageUploader", Destination::Custom(CustomKind::Image)),
    ("CustomTextUploader", Destination::Custom(CustomKind::Text)),
    ("CustomFileUploader", Destination::Custom(CustomKind::File)),
    ("CustomURLShortener", Destination::Custom(CustomKind::UrlShortener)),
    ("FileUploader", Destination::SameAsFile),
    ("Pastebin", Destination::Id("sxcu:pastebin")),
    ("Paste_ee", Destination::Id("sxcu:paste-ee")),
    ("Hastebin", Destination::Id("sxcu:hastebin")),
    ("Pomf", Destination::Id("sxcu:pomf")),
    ("Uguu", Destination::Id("sxcu:uguu")),
    ("Transfersh", Destination::Id("sxcu:transfer-sh")),
    ("ISGD", Destination::Id("sxcu:is-gd")),
    ("VGD", Destination::Id("sxcu:v-gd")),
    ("TINYURL", Destination::Id("sxcu:tinyurl")),
    ("YOURLS", Destination::Id("sxcu:yourls")),
    ("Polr", Destination::Id("sxcu:polr")),
    ("Kutt", Destination::Id("sxcu:kutt")),
];

/// The task a hotkey runs. The name is `HotkeyType`'s, which is also the Laqta variant name except for
/// the aliases above.
pub(crate) fn action(name: &str) -> Option<Action> {
    if let Some((_, action)) = ACTION_ALIASES.iter().find(|(alias, _)| alias.eq_ignore_ascii_case(name)) {
        return Some(*action);
    }
    Action::ALL.iter().copied().find(|action| format!("{action:?}").eq_ignore_ascii_case(name))
}

pub(crate) fn after_capture(text: &str, what: &str, warnings: &mut Vec<String>) -> AfterCaptureTasks {
    flags(text, AFTER_CAPTURE, AfterCaptureTasks::empty(), what, "after-capture task", warnings)
}

pub(crate) fn after_upload(text: &str, what: &str, warnings: &mut Vec<String>) -> AfterUploadTasks {
    flags(text, AFTER_UPLOAD, AfterUploadTasks::empty(), what, "after-upload task", warnings)
}

pub(crate) fn destination(name: &str) -> Option<Destination> {
    DESTINATIONS.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)).map(|(_, destination)| *destination)
}

/// A comma-separated list of ShareX flag names. Unknown names are reported and skipped.
fn flags<T: Copy + BitOr<Output = T>>(text: &str, table: &[(&str, T)], empty: T, what: &str, kind: &str, warnings: &mut Vec<String>) -> T {
    let mut out = empty;
    for name in text.split(',').map(str::trim).filter(|name| !name.is_empty() && !name.eq_ignore_ascii_case("none")) {
        match table.iter().find(|(known, _)| known.eq_ignore_ascii_case(name)) {
            Some((_, flag)) => out = out | *flag,
            None => warnings.push(format!("{what}: {kind} `{name}` has no Laqta equivalent, skipped")),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hotkey_types_map_to_actions() {
        assert_eq!(action("RectangleRegion"), Some(Action::RectangleRegion));
        assert_eq!(action("PrintScreen"), Some(Action::PrintScreen));
        assert_eq!(action("ExitShareX"), Some(Action::ExitLaqta));
        assert_eq!(action("ScreenRecorderGIF"), Some(Action::ScreenRecorderGif));
        assert_eq!(action("QRCode"), Some(Action::QrCode));
        assert_eq!(action("OCR"), Some(Action::Ocr));
        assert_eq!(action("rectangleregion"), Some(Action::RectangleRegion));
        assert_eq!(action("NoSuchTask"), None);
    }

    #[test]
    fn after_task_flags_combine_and_report_unknown_names() {
        let mut warnings = Vec::new();
        let flags = after_capture("CopyImageToClipboard, SaveImageToFile, Bogus", "test", &mut warnings);
        assert_eq!(flags, AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD | AfterCaptureTasks::SAVE_IMAGE_TO_FILE);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("Bogus"));
    }

    #[test]
    fn destinations_map_to_registry_ids() {
        assert_eq!(destination("Imgur"), Some(Destination::Id("imgur")));
        assert_eq!(destination("Paste_ee"), Some(Destination::Id("sxcu:paste-ee")));
        assert_eq!(destination("CustomImageUploader"), Some(Destination::Custom(CustomKind::Image)));
        assert_eq!(destination("nothing"), None);
    }
}

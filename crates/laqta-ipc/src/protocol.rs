//! The wire format and the text names of actions. One UTF-8 line per request and per reply.

use laqta_domain::Action;
use std::io::{self, BufRead};

/// Longest request or reply line, in bytes, without its line break.
const MAX_LINE_BYTES: usize = 4096;

const REQUEST_PREFIX: &str = "action ";

/// The text name of an action, as used by `--action` and the protocol: the variant name in
/// kebab-case, except `ExitLaqta`, which is `exit`.
pub fn action_name(action: Action) -> &'static str {
    match action {
        Action::FileUpload => "file-upload",
        Action::FolderUpload => "folder-upload",
        Action::ClipboardUpload => "clipboard-upload",
        Action::ClipboardUploadWithContentViewer => "clipboard-upload-with-content-viewer",
        Action::UploadText => "upload-text",
        Action::UploadUrl => "upload-url",
        Action::DragDropUpload => "drag-drop-upload",
        Action::ShortenUrl => "shorten-url",
        Action::StopUploads => "stop-uploads",
        Action::PrintScreen => "print-screen",
        Action::ActiveWindow => "active-window",
        Action::CustomWindow => "custom-window",
        Action::ActiveMonitor => "active-monitor",
        Action::RectangleRegion => "rectangle-region",
        Action::RectangleLight => "rectangle-light",
        Action::RectangleTransparent => "rectangle-transparent",
        Action::CustomRegion => "custom-region",
        Action::LastRegion => "last-region",
        Action::ScrollingCapture => "scrolling-capture",
        Action::AutoCapture => "auto-capture",
        Action::StartAutoCapture => "start-auto-capture",
        Action::StopAutoCapture => "stop-auto-capture",
        Action::ScreenRecorder => "screen-recorder",
        Action::ScreenRecorderActiveWindow => "screen-recorder-active-window",
        Action::ScreenRecorderCustomRegion => "screen-recorder-custom-region",
        Action::StartScreenRecorder => "start-screen-recorder",
        Action::ScreenRecorderGif => "screen-recorder-gif",
        Action::ScreenRecorderGifActiveWindow => "screen-recorder-gif-active-window",
        Action::ScreenRecorderGifCustomRegion => "screen-recorder-gif-custom-region",
        Action::StartScreenRecorderGif => "start-screen-recorder-gif",
        Action::StopScreenRecording => "stop-screen-recording",
        Action::PauseScreenRecording => "pause-screen-recording",
        Action::AbortScreenRecording => "abort-screen-recording",
        Action::ColorPicker => "color-picker",
        Action::ScreenColorPicker => "screen-color-picker",
        Action::Ruler => "ruler",
        Action::PinToScreen => "pin-to-screen",
        Action::PinToScreenFromScreen => "pin-to-screen-from-screen",
        Action::PinToScreenFromClipboard => "pin-to-screen-from-clipboard",
        Action::PinToScreenFromFile => "pin-to-screen-from-file",
        Action::PinToScreenCloseAll => "pin-to-screen-close-all",
        Action::ImageEditor => "image-editor",
        Action::ImageBeautifier => "image-beautifier",
        Action::ImageEffects => "image-effects",
        Action::ImageViewer => "image-viewer",
        Action::ImageCombiner => "image-combiner",
        Action::ImageSplitter => "image-splitter",
        Action::ImageThumbnailer => "image-thumbnailer",
        Action::ImageResizer => "image-resizer",
        Action::ImageConverter => "image-converter",
        Action::ImageWatermark => "image-watermark",
        Action::ImageComparer => "image-comparer",
        Action::IconConverter => "icon-converter",
        Action::VideoConverter => "video-converter",
        Action::VideoTrimmer => "video-trimmer",
        Action::VideoThumbnailer => "video-thumbnailer",
        Action::GifMaker => "gif-maker",
        Action::GifTrimmer => "gif-trimmer",
        Action::AnalyzeImage => "analyze-image",
        Action::RemoveBackground => "remove-background",
        Action::Ocr => "ocr",
        Action::QrCode => "qr-code",
        Action::QrCodeDecodeFromScreen => "qr-code-decode-from-screen",
        Action::QrCodeScanRegion => "qr-code-scan-region",
        Action::HashCheck => "hash-check",
        Action::Metadata => "metadata",
        Action::StripMetadata => "strip-metadata",
        Action::IndexFolder => "index-folder",
        Action::ClipboardViewer => "clipboard-viewer",
        Action::BorderlessWindow => "borderless-window",
        Action::ActiveWindowBorderless => "active-window-borderless",
        Action::ActiveWindowTopMost => "active-window-top-most",
        Action::InspectWindow => "inspect-window",
        Action::MonitorTest => "monitor-test",
        Action::NetworkMonitor => "network-monitor",
        Action::MouseHighlighter => "mouse-highlighter",
        Action::PhotoEditor => "photo-editor",
        Action::DisableHotkeys => "disable-hotkeys",
        Action::OpenMainWindow => "open-main-window",
        Action::OpenScreenshotsFolder => "open-screenshots-folder",
        Action::OpenHistory => "open-history",
        Action::OpenImageHistory => "open-image-history",
        Action::OpenSettings => "open-settings",
        Action::ToggleActionsToolbar => "toggle-actions-toolbar",
        Action::ToggleTrayMenu => "toggle-tray-menu",
        Action::ExitLaqta => "exit",
        _ => "unknown",
    }
}

/// The action with this name, ignoring ASCII case. Only the names `action_name` returns match.
pub fn parse_action(name: &str) -> Option<Action> {
    Action::ALL.iter().copied().find(|&action| action_name(action).eq_ignore_ascii_case(name))
}

/// Reads one line without its line break. `Ok(None)` means the stream ended. A line that is too
/// long, or not UTF-8, is an `InvalidData` error.
pub(crate) fn read_line(reader: &mut impl BufRead) -> io::Result<Option<String>> {
    let mut line = String::new();
    let mut limited = io::Read::take(&mut *reader, MAX_LINE_BYTES as u64);
    let read = limited.read_line(&mut line)?;
    if read == 0 {
        return Ok(None);
    }
    if !line.ends_with('\n') && read >= MAX_LINE_BYTES {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "line too long"));
    }
    Ok(Some(line.trim_end_matches(['\r', '\n']).to_owned()))
}

pub(crate) fn encode_request(action: Action) -> String {
    format!("{REQUEST_PREFIX}{}\n", action_name(action))
}

/// Parses a request line (without its line break). The error text is the reply's message.
pub(crate) fn parse_request(line: &str) -> Result<Action, String> {
    let name = line.strip_prefix(REQUEST_PREFIX).filter(|name| !name.is_empty()).ok_or_else(|| "malformed request".to_owned())?;
    parse_action(name).ok_or_else(|| format!("unknown action {name}"))
}

/// Longest error message sent. Keeps a reply within the line limit the reader enforces.
const MAX_MESSAGE_BYTES: usize = 1024;

/// Encodes a reply as one line. Line breaks in an error message become spaces, and a long message
/// is cut at a character boundary.
pub(crate) fn encode_reply(result: &Result<(), String>) -> String {
    match result {
        Ok(()) => "ok\n".to_owned(),
        Err(message) => {
            let mut flat = message.replace(['\r', '\n'], " ");
            let mut end = flat.len().min(MAX_MESSAGE_BYTES);
            while !flat.is_char_boundary(end) {
                end -= 1;
            }
            flat.truncate(end);
            format!("err {flat}\n")
        }
    }
}

/// Parses a reply line (without its line break). An `err` reply becomes `Err` with its message.
pub(crate) fn parse_reply(line: &str) -> Result<(), String> {
    if line == "ok" {
        Ok(())
    } else if let Some(message) = line.strip_prefix("err ") {
        Err(message.to_owned())
    } else {
        Err(format!("malformed reply: {line}"))
    }
}

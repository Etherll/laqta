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

/// One request from a secondary instance or the native-messaging host.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Request {
    Action(Action),
    /// Upload a file (bare paths on the command line, Send To, shell menus).
    Upload(std::path::PathBuf),
    /// Run a workflow by name (`-workflow`).
    Workflow(String),
    UploadText(String),
    /// Download a URL and upload it again.
    UploadUrl(String),
    ShortenUrl(String),
}

/// Escapes a payload so it stays on one line: `\` → `\\`, line breaks → `\n` / `\r`.
fn escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            c => out.push(c),
        }
    }
    out
}

fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('r') => out.push('\r'),
            Some(other) => out.push(other),
            None => out.push('\\'),
        }
    }
    out
}

pub(crate) fn encode_request(action: Action) -> String {
    format!("{REQUEST_PREFIX}{}\n", action_name(action))
}

pub(crate) fn encode(request: &Request) -> String {
    match request {
        Request::Action(a) => encode_request(*a),
        Request::Upload(p) => format!("upload {}\n", escape(&p.to_string_lossy())),
        Request::Workflow(n) => format!("workflow {}\n", escape(n)),
        Request::UploadText(t) => format!("text {}\n", escape(t)),
        Request::UploadUrl(u) => format!("url {}\n", escape(u)),
        Request::ShortenUrl(u) => format!("shorten {}\n", escape(u)),
    }
}

pub(crate) fn parse(line: &str) -> Result<Request, String> {
    let (verb, rest) = line.split_once(' ').ok_or_else(|| "malformed request".to_owned())?;
    if rest.is_empty() {
        return Err("malformed request".to_owned());
    }
    let payload = unescape(rest);
    Ok(match verb {
        "action" => Request::Action(parse_action(rest).ok_or_else(|| format!("unknown action {rest}"))?),
        "upload" => Request::Upload(payload.into()),
        "workflow" => Request::Workflow(payload),
        "text" => Request::UploadText(payload),
        "url" => Request::UploadUrl(payload),
        "shorten" => Request::ShortenUrl(payload),
        _ => return Err("malformed request".to_owned()),
    })
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

#[cfg(test)]
mod request_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn requests_round_trip() {
        for r in [
            Request::Action(Action::PrintScreen),
            Request::Upload("/tmp/a b\\c.png".into()),
            Request::Workflow("Region to Imgur".into()),
            Request::UploadText("line 1\nline 2\\".into()),
            Request::UploadUrl("https://x/y".into()),
            Request::ShortenUrl("https://x/z".into()),
        ] {
            let line = encode(&r);
            assert_eq!(line.matches('\n').count(), 1, "{line:?}");
            assert_eq!(parse(line.trim_end_matches('\n')).unwrap(), r);
        }
        assert!(parse("bogus x").is_err());
        assert!(parse("upload ").is_err());
    }
}

//! One variant per hotkey action. CLI and IPC names live in `laqta-ipc`.

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[non_exhaustive]
pub enum Action {
    // Upload
    FileUpload,
    FolderUpload,
    ClipboardUpload,
    ClipboardUploadWithContentViewer,
    UploadText,
    UploadUrl,
    DragDropUpload,
    ShortenUrl,
    StopUploads,
    // Screen capture
    PrintScreen,
    ActiveWindow,
    CustomWindow,
    ActiveMonitor,
    RectangleRegion,
    RectangleLight,
    RectangleTransparent,
    CustomRegion,
    LastRegion,
    ScrollingCapture,
    AutoCapture,
    StartAutoCapture,
    StopAutoCapture,
    // Screen recording
    ScreenRecorder,
    ScreenRecorderActiveWindow,
    ScreenRecorderCustomRegion,
    StartScreenRecorder,
    ScreenRecorderGif,
    ScreenRecorderGifActiveWindow,
    ScreenRecorderGifCustomRegion,
    StartScreenRecorderGif,
    StopScreenRecording,
    PauseScreenRecording,
    AbortScreenRecording,
    // Tools
    ColorPicker,
    ScreenColorPicker,
    Ruler,
    PinToScreen,
    PinToScreenFromScreen,
    PinToScreenFromClipboard,
    PinToScreenFromFile,
    PinToScreenCloseAll,
    ImageEditor,
    ImageBeautifier,
    ImageEffects,
    ImageViewer,
    ImageCombiner,
    ImageSplitter,
    ImageThumbnailer,
    ImageResizer,
    ImageConverter,
    ImageWatermark,
    ImageComparer,
    IconConverter,
    VideoConverter,
    VideoTrimmer,
    VideoThumbnailer,
    GifMaker,
    GifTrimmer,
    AnalyzeImage,
    RemoveBackground,
    Ocr,
    QrCode,
    QrCodeDecodeFromScreen,
    QrCodeScanRegion,
    HashCheck,
    Metadata,
    StripMetadata,
    IndexFolder,
    ClipboardViewer,
    BorderlessWindow,
    ActiveWindowBorderless,
    ActiveWindowTopMost,
    InspectWindow,
    MonitorTest,
    NetworkMonitor,
    MouseHighlighter,
    PhotoEditor,
    // Other
    DisableHotkeys,
    OpenMainWindow,
    OpenScreenshotsFolder,
    OpenHistory,
    OpenImageHistory,
    OpenSettings,
    ToggleActionsToolbar,
    ToggleTrayMenu,
    ExitLaqta,
}

/// The group a hotkey action is listed under.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ActionCategory {
    Upload,
    ScreenCapture,
    ScreenRecord,
    Tools,
    Other,
}

impl Action {
    /// English UI label.
    pub fn label(self) -> &'static str {
        match self {
            Action::FileUpload => "Upload file",
            Action::FolderUpload => "Upload folder",
            Action::ClipboardUpload => "Upload from clipboard",
            Action::ClipboardUploadWithContentViewer => "Upload from clipboard with content viewer",
            Action::UploadText => "Upload text",
            Action::UploadUrl => "Upload from URL",
            Action::DragDropUpload => "Drag and drop upload",
            Action::ShortenUrl => "Shorten URL",
            Action::StopUploads => "Stop all uploads",
            Action::PrintScreen => "Capture entire screen",
            Action::ActiveWindow => "Capture active window",
            Action::CustomWindow => "Capture window by title",
            Action::ActiveMonitor => "Capture active monitor",
            Action::RectangleRegion => "Capture region",
            Action::RectangleLight => "Capture region (light)",
            Action::RectangleTransparent => "Capture region (transparent)",
            Action::CustomRegion => "Capture custom region",
            Action::LastRegion => "Capture last region",
            Action::ScrollingCapture => "Scrolling capture",
            Action::AutoCapture => "Auto capture",
            Action::StartAutoCapture => "Start auto capture",
            Action::StopAutoCapture => "Stop auto capture",
            Action::ScreenRecorder => "Screen recording",
            Action::ScreenRecorderActiveWindow => "Screen recording (active window)",
            Action::ScreenRecorderCustomRegion => "Screen recording (custom region)",
            Action::StartScreenRecorder => "Start screen recording using last region",
            Action::ScreenRecorderGif => "Screen recording (GIF)",
            Action::ScreenRecorderGifActiveWindow => "Screen recording (GIF, active window)",
            Action::ScreenRecorderGifCustomRegion => "Screen recording (GIF, custom region)",
            Action::StartScreenRecorderGif => "Start GIF recording using last region",
            Action::StopScreenRecording => "Stop screen recording",
            Action::PauseScreenRecording => "Pause screen recording",
            Action::AbortScreenRecording => "Abort screen recording",
            Action::ColorPicker => "Color picker",
            Action::ScreenColorPicker => "Screen color picker",
            Action::Ruler => "Ruler",
            Action::PinToScreen => "Pin to screen",
            Action::PinToScreenFromScreen => "Pin to screen (from screen)",
            Action::PinToScreenFromClipboard => "Pin to screen (from clipboard)",
            Action::PinToScreenFromFile => "Pin to screen (from file)",
            Action::PinToScreenCloseAll => "Close all pinned images",
            Action::ImageEditor => "Image editor",
            Action::ImageBeautifier => "Image beautifier",
            Action::ImageEffects => "Image effects",
            Action::ImageViewer => "Image viewer",
            Action::ImageCombiner => "Image combiner",
            Action::ImageSplitter => "Image splitter",
            Action::ImageThumbnailer => "Image thumbnailer",
            Action::ImageResizer => "Image resizer",
            Action::ImageConverter => "Image converter",
            Action::ImageWatermark => "Image watermark",
            Action::ImageComparer => "Image comparer",
            Action::IconConverter => "Icon converter",
            Action::VideoConverter => "Video converter",
            Action::VideoTrimmer => "Video trimmer",
            Action::VideoThumbnailer => "Video thumbnailer",
            Action::GifMaker => "GIF maker",
            Action::GifTrimmer => "GIF trimmer",
            Action::AnalyzeImage => "Analyze image",
            Action::RemoveBackground => "Remove background",
            Action::Ocr => "Text capture (OCR)",
            Action::QrCode => "QR code",
            Action::QrCodeDecodeFromScreen => "QR code (scan screen)",
            Action::QrCodeScanRegion => "QR code (scan region)",
            Action::HashCheck => "Hash checker",
            Action::Metadata => "Metadata viewer",
            Action::StripMetadata => "Strip metadata",
            Action::IndexFolder => "Directory indexer",
            Action::ClipboardViewer => "Clipboard viewer",
            Action::BorderlessWindow => "Borderless window",
            Action::ActiveWindowBorderless => "Make active window borderless",
            Action::ActiveWindowTopMost => "Make active window top most",
            Action::InspectWindow => "Inspect window",
            Action::MonitorTest => "Monitor test",
            Action::NetworkMonitor => "Network monitor",
            Action::MouseHighlighter => "Mouse highlighter",
            Action::PhotoEditor => "Photo editor",
            Action::DisableHotkeys => "Disable/enable hotkeys",
            Action::OpenMainWindow => "Open main window",
            Action::OpenScreenshotsFolder => "Open screenshots folder",
            Action::OpenHistory => "Open history",
            Action::OpenImageHistory => "Open image history",
            Action::OpenSettings => "Open settings",
            Action::ToggleActionsToolbar => "Toggle actions toolbar",
            Action::ToggleTrayMenu => "Toggle tray menu",
            Action::ExitLaqta => "Exit Laqta",
        }
    }

    /// Every action, in declaration order.
    pub const ALL: &'static [Action] = &[
        Self::FileUpload,
        Self::FolderUpload,
        Self::ClipboardUpload,
        Self::ClipboardUploadWithContentViewer,
        Self::UploadText,
        Self::UploadUrl,
        Self::DragDropUpload,
        Self::ShortenUrl,
        Self::StopUploads,
        Self::PrintScreen,
        Self::ActiveWindow,
        Self::CustomWindow,
        Self::ActiveMonitor,
        Self::RectangleRegion,
        Self::RectangleLight,
        Self::RectangleTransparent,
        Self::CustomRegion,
        Self::LastRegion,
        Self::ScrollingCapture,
        Self::AutoCapture,
        Self::StartAutoCapture,
        Self::StopAutoCapture,
        Self::ScreenRecorder,
        Self::ScreenRecorderActiveWindow,
        Self::ScreenRecorderCustomRegion,
        Self::StartScreenRecorder,
        Self::ScreenRecorderGif,
        Self::ScreenRecorderGifActiveWindow,
        Self::ScreenRecorderGifCustomRegion,
        Self::StartScreenRecorderGif,
        Self::StopScreenRecording,
        Self::PauseScreenRecording,
        Self::AbortScreenRecording,
        Self::ColorPicker,
        Self::ScreenColorPicker,
        Self::Ruler,
        Self::PinToScreen,
        Self::PinToScreenFromScreen,
        Self::PinToScreenFromClipboard,
        Self::PinToScreenFromFile,
        Self::PinToScreenCloseAll,
        Self::ImageEditor,
        Self::ImageBeautifier,
        Self::ImageEffects,
        Self::ImageViewer,
        Self::ImageCombiner,
        Self::ImageSplitter,
        Self::ImageThumbnailer,
        Self::ImageResizer,
        Self::ImageConverter,
        Self::ImageWatermark,
        Self::ImageComparer,
        Self::IconConverter,
        Self::VideoConverter,
        Self::VideoTrimmer,
        Self::VideoThumbnailer,
        Self::GifMaker,
        Self::GifTrimmer,
        Self::AnalyzeImage,
        Self::RemoveBackground,
        Self::Ocr,
        Self::QrCode,
        Self::QrCodeDecodeFromScreen,
        Self::QrCodeScanRegion,
        Self::HashCheck,
        Self::Metadata,
        Self::StripMetadata,
        Self::IndexFolder,
        Self::ClipboardViewer,
        Self::BorderlessWindow,
        Self::ActiveWindowBorderless,
        Self::ActiveWindowTopMost,
        Self::InspectWindow,
        Self::MonitorTest,
        Self::NetworkMonitor,
        Self::MouseHighlighter,
        Self::PhotoEditor,
        Self::DisableHotkeys,
        Self::OpenMainWindow,
        Self::OpenScreenshotsFolder,
        Self::OpenHistory,
        Self::OpenImageHistory,
        Self::OpenSettings,
        Self::ToggleActionsToolbar,
        Self::ToggleTrayMenu,
        Self::ExitLaqta,
    ];

    /// The group this action is listed under.
    pub fn category(self) -> ActionCategory {
        match self {
            Self::FileUpload
            | Self::FolderUpload
            | Self::ClipboardUpload
            | Self::ClipboardUploadWithContentViewer
            | Self::UploadText
            | Self::UploadUrl
            | Self::DragDropUpload
            | Self::ShortenUrl
            | Self::StopUploads => ActionCategory::Upload,
            Self::PrintScreen
            | Self::ActiveWindow
            | Self::CustomWindow
            | Self::ActiveMonitor
            | Self::RectangleRegion
            | Self::RectangleLight
            | Self::RectangleTransparent
            | Self::CustomRegion
            | Self::LastRegion
            | Self::ScrollingCapture
            | Self::AutoCapture
            | Self::StartAutoCapture
            | Self::StopAutoCapture => ActionCategory::ScreenCapture,
            Self::ScreenRecorder
            | Self::ScreenRecorderActiveWindow
            | Self::ScreenRecorderCustomRegion
            | Self::StartScreenRecorder
            | Self::ScreenRecorderGif
            | Self::ScreenRecorderGifActiveWindow
            | Self::ScreenRecorderGifCustomRegion
            | Self::StartScreenRecorderGif
            | Self::StopScreenRecording
            | Self::PauseScreenRecording
            | Self::AbortScreenRecording => ActionCategory::ScreenRecord,
            Self::ColorPicker
            | Self::ScreenColorPicker
            | Self::Ruler
            | Self::PinToScreen
            | Self::PinToScreenFromScreen
            | Self::PinToScreenFromClipboard
            | Self::PinToScreenFromFile
            | Self::PinToScreenCloseAll
            | Self::ImageEditor
            | Self::ImageBeautifier
            | Self::ImageEffects
            | Self::ImageViewer
            | Self::ImageCombiner
            | Self::ImageSplitter
            | Self::ImageThumbnailer
            | Self::ImageResizer
            | Self::ImageConverter
            | Self::ImageWatermark
            | Self::ImageComparer
            | Self::IconConverter
            | Self::VideoConverter
            | Self::VideoTrimmer
            | Self::VideoThumbnailer
            | Self::GifMaker
            | Self::GifTrimmer
            | Self::AnalyzeImage
            | Self::RemoveBackground
            | Self::Ocr
            | Self::QrCode
            | Self::QrCodeDecodeFromScreen
            | Self::QrCodeScanRegion
            | Self::HashCheck
            | Self::Metadata
            | Self::StripMetadata
            | Self::IndexFolder
            | Self::ClipboardViewer
            | Self::BorderlessWindow
            | Self::ActiveWindowBorderless
            | Self::ActiveWindowTopMost
            | Self::InspectWindow
            | Self::MonitorTest
            | Self::NetworkMonitor
            | Self::MouseHighlighter
            | Self::PhotoEditor => ActionCategory::Tools,
            Self::DisableHotkeys
            | Self::OpenMainWindow
            | Self::OpenScreenshotsFolder
            | Self::OpenHistory
            | Self::OpenImageHistory
            | Self::OpenSettings
            | Self::ToggleActionsToolbar
            | Self::ToggleTrayMenu
            | Self::ExitLaqta => ActionCategory::Other,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// The number of variants in the enum above. Update it together with the enum and `ALL`.
    const VARIANT_COUNT: usize = 86;

    #[test]
    fn all_lists_every_variant_once() {
        assert_eq!(Action::ALL.len(), VARIANT_COUNT);
        let unique: HashSet<Action> = Action::ALL.iter().copied().collect();
        assert_eq!(unique.len(), VARIANT_COUNT);
    }

    #[test]
    fn categories_are_non_empty_and_listed_in_one_run_each() {
        let order = [
            ActionCategory::Upload,
            ActionCategory::ScreenCapture,
            ActionCategory::ScreenRecord,
            ActionCategory::Tools,
            ActionCategory::Other,
        ];
        for category in order {
            assert!(Action::ALL.iter().any(|action| action.category() == category), "{category:?} has no actions");
        }
        let mut runs: Vec<ActionCategory> = Vec::new();
        for action in Action::ALL {
            let category = action.category();
            if runs.last() != Some(&category) {
                runs.push(category);
            }
        }
        assert_eq!(runs, order);
    }
}

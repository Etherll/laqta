//! ShareX's `TaskSettings` as Laqta's sections. The ShareX defaults overlay Laqta's defaults. A hotkey's
//! overrides are the sections whose `UseDefault*` flag is false, and they become `Some` patches (ADR 008).

use laqta_domain::capture::CaptureOptions;
use laqta_domain::image::ImageFormat;
use laqta_domain::workflow::{
    FileExistsAction, GeneralSettings, ImageSettings, NamingSettings, TaskSettings, TaskSettingsPatch, UploadSettings,
};

use crate::application;
use crate::jobs::{self, Destination};
use crate::json::{self, Object};
use crate::uploaders::Selections;

/// Overlays the sections a ShareX task object sets on `base`. `what` names the object in warnings.
pub(crate) fn overlay(
    base: &TaskSettings,
    object: &Object,
    selections: &Selections,
    what: &str,
    warnings: &mut Vec<String>,
) -> TaskSettings {
    let mut task = base.clone();
    if let Some(text) = json::text(object, "AfterCaptureJob") {
        task.after_capture = jobs::after_capture(text, what, warnings);
    }
    if let Some(text) = json::text(object, "AfterUploadJob") {
        task.after_upload = jobs::after_upload(text, what, warnings);
    }
    task.upload = destinations(&base.upload, object, selections, what, warnings);
    if let Some(section) = json::sub(object, "GeneralSettings") {
        task.general = general(&base.general, section);
    }
    if let Some(section) = json::sub(object, "ImageSettings") {
        task.image = image(&base.image, section, what, warnings);
    }
    if let Some(section) = json::sub(object, "CaptureSettings") {
        task.capture = capture(&base.capture, section);
    }
    if let Some(section) = json::sub(object, "UploadSettings") {
        task.naming = naming(&base.naming, section);
    }
    task
}

/// The sections a hotkey's task object overrides. ShareX treats a missing `UseDefault*` flag as true.
pub(crate) fn patch(
    object: &Object,
    defaults: &TaskSettings,
    selections: &Selections,
    what: &str,
    warnings: &mut Vec<String>,
) -> TaskSettingsPatch {
    let overrides = |key: &str| json::flag(object, key) == Some(false);
    let mut patch = TaskSettingsPatch::default();
    if overrides("UseDefaultAfterCaptureJob") {
        patch.after_capture =
            Some(json::text(object, "AfterCaptureJob").map_or(defaults.after_capture, |text| jobs::after_capture(text, what, warnings)));
    }
    if overrides("UseDefaultAfterUploadJob") {
        patch.after_upload =
            Some(json::text(object, "AfterUploadJob").map_or(defaults.after_upload, |text| jobs::after_upload(text, what, warnings)));
    }
    if overrides("UseDefaultDestinations") {
        patch.upload = Some(destinations(&defaults.upload, object, selections, what, warnings));
    }
    if overrides("UseDefaultGeneralSettings") {
        patch.general = Some(
            json::sub(object, "GeneralSettings").map_or_else(|| defaults.general.clone(), |section| general(&defaults.general, section)),
        );
    }
    if overrides("UseDefaultImageSettings") {
        patch.image = Some(
            json::sub(object, "ImageSettings")
                .map_or_else(|| defaults.image.clone(), |section| image(&defaults.image, section, what, warnings)),
        );
    }
    if overrides("UseDefaultCaptureSettings") {
        patch.capture = Some(
            json::sub(object, "CaptureSettings").map_or_else(|| defaults.capture.clone(), |section| capture(&defaults.capture, section)),
        );
    }
    if overrides("UseDefaultUploadSettings") {
        patch.naming =
            Some(json::sub(object, "UploadSettings").map_or_else(|| defaults.naming.clone(), |section| naming(&defaults.naming, section)));
    }
    if json::flag(object, "OverrideScreenshotsFolder") == Some(true)
        && let Some(folder) = json::text(object, "ScreenshotsFolder")
    {
        patch.screenshots_dir = application::folder_of(folder, what, warnings);
        if patch.screenshots_dir.is_some() {
            warnings.push(format!("{what}: this task saves to the screenshots folder `{folder}`; check that it exists on this machine"));
        }
    }
    if !json::list(object, "WatchFolderList").is_empty() {
        warnings.push(format!("{what}: this hotkey's watch folders are not imported; set them in Laqta"));
    }
    patch
}

/// The destinations are flat keys on the task object, not a section.
fn destinations(base: &UploadSettings, object: &Object, selections: &Selections, what: &str, warnings: &mut Vec<String>) -> UploadSettings {
    let file = slot(object, "FileDestination", &base.file_destination, &base.file_destination, selections, what, warnings);
    UploadSettings {
        image_destination: slot(object, "ImageDestination", &base.image_destination, &file, selections, what, warnings),
        text_destination: slot(object, "TextDestination", &base.text_destination, &file, selections, what, warnings),
        url_shortener: slot(object, "URLShortenerDestination", &base.url_shortener, &file, selections, what, warnings),
        file_destination: file,
        ..base.clone()
    }
}

/// One destination key. `current` is kept when the key is absent or cannot be mapped.
fn slot(object: &Object, key: &str, current: &str, file: &str, selections: &Selections, what: &str, warnings: &mut Vec<String>) -> String {
    let Some(name) = json::text(object, key) else {
        return current.to_owned();
    };
    match jobs::destination(name) {
        Some(Destination::Id(id)) => id.to_owned(),
        Some(Destination::SameAsFile) => file.to_owned(),
        Some(Destination::Custom(kind)) => match selections.get(kind) {
            Some(uploader) => format!("custom:{}", uploader.trim()),
            None => {
                warnings.push(format!("{what}: {key} is `{name}` but no custom uploader is selected; kept `{current}`"));
                current.to_owned()
            }
        },
        None => {
            warnings.push(format!("{what}: {key} `{name}` has no Laqta equivalent; kept `{current}`"));
            current.to_owned()
        }
    }
}

fn general(base: &GeneralSettings, object: &Object) -> GeneralSettings {
    GeneralSettings {
        play_sound_after_capture: json::flag(object, "PlaySoundAfterCapture").unwrap_or(base.play_sound_after_capture),
        play_sound_after_upload: json::flag(object, "PlaySoundAfterUpload").unwrap_or(base.play_sound_after_upload),
        ..base.clone()
    }
}

fn image(base: &ImageSettings, object: &Object, what: &str, warnings: &mut Vec<String>) -> ImageSettings {
    let mut image = base.clone();
    if let Some(name) = json::text(object, "ImageFormat") {
        match format_named(name) {
            Some(format) => image.format = format,
            None => warnings.push(format!("{what}: image format `{name}` has no Laqta equivalent; kept {:?}", base.format)),
        }
    }
    if let Some(quality) = json::number(object, "ImageJPEGQuality") {
        image.jpeg_quality = quality.round().clamp(1.0, 100.0) as u8;
    }
    if let Some(auto) = json::flag(object, "ImageAutoUseJPEG") {
        image.auto_jpeg_above_bytes = if auto {
            json::number(object, "ImageAutoUseJPEGSize")
                .map(|kilobytes| (kilobytes.max(0.0) * 1024.0) as u64)
                .or(base.auto_jpeg_above_bytes)
        } else {
            None
        };
    }
    if let Some(name) = json::text(object, "FileExistAction") {
        match name.to_ascii_lowercase().as_str() {
            "uniquename" => image.file_exists = FileExistsAction::AppendNumber,
            "overwrite" => image.file_exists = FileExistsAction::Overwrite,
            // Laqta never asks: a taken name gets a number, which is what `Ask` does here, so say so.
            "ask" => {
                image.file_exists = FileExistsAction::AppendNumber;
                warnings.push(format!("{what}: file exists action `ask` is not supported; Laqta adds a number to the name"));
            }
            "skip" => image.file_exists = FileExistsAction::Skip,
            _ => warnings.push(format!("{what}: file exists action `{name}` has no Laqta equivalent; kept `{:?}`", base.file_exists)),
        }
    }
    image
}

fn format_named(name: &str) -> Option<ImageFormat> {
    match name.to_ascii_uppercase().as_str() {
        "PNG" => Some(ImageFormat::Png),
        "JPG" | "JPEG" => Some(ImageFormat::Jpeg),
        "GIF" => Some(ImageFormat::Gif),
        "BMP" => Some(ImageFormat::Bmp),
        "TIFF" | "TIF" => Some(ImageFormat::Tiff),
        "WEBP" => Some(ImageFormat::WebP),
        _ => None,
    }
}

fn capture(base: &CaptureOptions, object: &Object) -> CaptureOptions {
    CaptureOptions {
        show_cursor: json::flag(object, "ShowCursor").unwrap_or(base.show_cursor),
        transparent_window: json::flag(object, "CaptureTransparent").unwrap_or(base.transparent_window),
        delay_ms: json::number(object, "ScreenshotDelay")
            .map_or(base.delay_ms, |seconds| (seconds.max(0.0) * 1000.0).round().min(f64::from(u32::MAX)) as u32),
        ..base.clone()
    }
}

/// ShareX's upload section holds the file-name patterns, which Laqta keeps as its naming settings.
fn naming(base: &NamingSettings, object: &Object) -> NamingSettings {
    NamingSettings {
        pattern: json::text(object, "NameFormatPattern").map_or_else(|| base.pattern.clone(), str::to_owned),
        window_pattern: json::text(object, "NameFormatPatternActiveWindow").map_or_else(|| base.window_pattern.clone(), str::to_owned),
        ..base.clone()
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn image_format_names() {
        assert_eq!(format_named("jpg"), Some(ImageFormat::Jpeg));
        assert_eq!(format_named("PNG"), Some(ImageFormat::Png));
        assert_eq!(format_named("heic"), None);
    }

    #[test]
    fn missing_flags_mean_use_default() {
        let object: Object = serde_json::from_str(r#"{"AfterCaptureJob": "SaveImageToFile"}"#).unwrap();
        let defaults = TaskSettings::default();
        let patch = patch(&object, &defaults, &Selections::default(), "test", &mut Vec::new());
        assert!(patch.is_empty());
    }
}

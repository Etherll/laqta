//! Workflows: a named Action plus the task settings it overrides (ADR 008).

use crate::action::Action;
use crate::capture::CaptureOptions;
use crate::hotkey::KeyChord;
use crate::image::ImageFormat;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct WorkflowId(pub u32);

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(transparent)]
    pub struct AfterCaptureTasks: u32 {
        const SHOW_QUICK_TASK_MENU = 1 << 0;
        const SHOW_AFTER_CAPTURE_WINDOW = 1 << 1;
        const BEAUTIFY_IMAGE = 1 << 2;
        const ADD_IMAGE_EFFECTS = 1 << 3;
        const ANNOTATE_IMAGE = 1 << 4;
        const OPEN_IN_PHOTO_EDITOR = 1 << 5;
        const COPY_IMAGE_TO_CLIPBOARD = 1 << 6;
        const PIN_TO_SCREEN = 1 << 7;
        const SEND_IMAGE_TO_PRINTER = 1 << 8;
        const SAVE_IMAGE_TO_FILE = 1 << 9;
        const SAVE_IMAGE_TO_FILE_WITH_DIALOG = 1 << 10;
        const SAVE_THUMBNAIL_IMAGE_TO_FILE = 1 << 11;
        const PERFORM_ACTIONS = 1 << 12;
        const COPY_FILE_TO_CLIPBOARD = 1 << 13;
        const COPY_FILE_PATH_TO_CLIPBOARD = 1 << 14;
        const SHOW_IN_EXPLORER = 1 << 15;
        const SCAN_QR_CODE = 1 << 16;
        const DO_OCR = 1 << 17;
        const SHOW_BEFORE_UPLOAD_WINDOW = 1 << 18;
        const UPLOAD_IMAGE_TO_HOST = 1 << 19;
        const DELETE_FILE = 1 << 20;
        const ANALYZE_IMAGE = 1 << 21;
        const REMOVE_BACKGROUND = 1 << 22;
        const COPY_FOLDER_PATH_TO_CLIPBOARD = 1 << 23;
    }
}

bitflags::bitflags! {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
    #[serde(transparent)]
    pub struct AfterUploadTasks: u32 {
        const SHOW_AFTER_UPLOAD_WINDOW = 1 << 0;
        const USE_URL_SHORTENER = 1 << 1;
        const SHARE_URL = 1 << 2;
        const COPY_URL_TO_CLIPBOARD = 1 << 3;
        const OPEN_URL = 1 << 4;
        const SHOW_QR_CODE = 1 << 5;
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileExistsAction {
    Overwrite,
    #[default]
    AppendNumber,
    Ask,
    Skip,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    pub play_sound_after_capture: bool,
    pub play_sound_after_upload: bool,
    /// Sound when a task finishes, and an error sound when it fails.
    pub play_sound_after_task: bool,
    pub show_toast: bool,
    pub toast: ToastOptions,
    /// Custom sound files overriding the built-in sounds.
    pub custom_capture_sound: Option<std::path::PathBuf>,
    pub custom_task_sound: Option<std::path::PathBuf>,
    pub custom_error_sound: Option<std::path::PathBuf>,
}

impl Default for GeneralSettings {
    fn default() -> Self {
        GeneralSettings {
            play_sound_after_capture: false,
            play_sound_after_upload: false,
            play_sound_after_task: false,
            show_toast: true,
            toast: ToastOptions::default(),
            custom_capture_sound: None,
            custom_task_sound: None,
            custom_error_sound: None,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToastPlacement {
    TopLeft,
    TopRight,
    BottomLeft,
    #[default]
    BottomRight,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToastClickAction {
    None,
    #[default]
    OpenFile,
    OpenFolder,
    OpenUrl,
    CopyUrl,
    Close,
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ToastOptions {
    pub duration_ms: u32,
    pub fade_ms: u32,
    pub placement: ToastPlacement,
    pub width: u32,
    pub height: u32,
    pub left_click: ToastClickAction,
    pub right_click: ToastClickAction,
    pub middle_click: ToastClickAction,
}

impl Default for ToastOptions {
    fn default() -> Self {
        ToastOptions {
            duration_ms: 3000,
            fade_ms: 1000,
            placement: ToastPlacement::default(),
            width: 400,
            height: 300,
            left_click: ToastClickAction::OpenFile,
            right_click: ToastClickAction::Close,
            middle_click: ToastClickAction::OpenFolder,
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ImageSettings {
    pub format: ImageFormat,
    pub jpeg_quality: u8,
    pub auto_jpeg_above_bytes: Option<u64>,
    pub file_exists: FileExistsAction,
    pub thumbnail_width: u32,
    /// 0 keeps the aspect ratio.
    pub thumbnail_height: u32,
    pub thumbnail_suffix: String,
}

impl Default for ImageSettings {
    fn default() -> Self {
        ImageSettings {
            format: ImageFormat::Png,
            jpeg_quality: 90,
            auto_jpeg_above_bytes: None,
            file_exists: FileExistsAction::default(),
            thumbnail_width: 200,
            thumbnail_height: 0,
            thumbnail_suffix: "-thumbnail".to_string(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct NamingSettings {
    pub pattern: String,
    pub window_pattern: String,
    pub subfolder: String,
}

impl Default for NamingSettings {
    fn default() -> Self {
        NamingSettings {
            pattern: "Laqta_%y-%mo-%d_%h-%mi-%s".to_string(),
            window_pattern: "%pn_%y-%mo-%d_%h-%mi-%s".to_string(),
            subfolder: "%y-%mo".to_string(),
        }
    }
}

/// The task settings a job runs with. Sections are complete values here; `TaskSettingsPatch` holds the overrides.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct TaskSettings {
    pub after_capture: AfterCaptureTasks,
    pub beautify: BeautifySettings,
    /// The effect preset `ADD_IMAGE_EFFECTS` applies (by name, from `Settings.effect_presets`);
    /// empty = the first preset.
    pub effect_preset: String,
    pub upload: UploadSettings,
    pub recording: crate::record::RecordingSettings,
    pub after_upload: AfterUploadTasks,
    pub general: GeneralSettings,
    pub image: ImageSettings,
    pub capture: CaptureOptions,
    pub naming: NamingSettings,
    /// `None` means `Settings.screenshots_dir`.
    pub screenshots_dir: Option<std::path::PathBuf>,
}

impl Default for TaskSettings {
    fn default() -> Self {
        TaskSettings {
            after_capture: AfterCaptureTasks::SAVE_IMAGE_TO_FILE | AfterCaptureTasks::COPY_IMAGE_TO_CLIPBOARD,
            after_upload: AfterUploadTasks::COPY_URL_TO_CLIPBOARD,
            upload: UploadSettings::default(),
            beautify: BeautifySettings::default(),
            effect_preset: String::new(),
            recording: crate::record::RecordingSettings::default(),
            general: GeneralSettings::default(),
            image: ImageSettings::default(),
            capture: CaptureOptions::default(),
            naming: NamingSettings::default(),
            screenshots_dir: None,
        }
    }
}

/// An external program run after capture. Imported ShareX `PerformActions` tasks map to
/// `AfterCaptureTasks::PERFORM_ACTIONS`.
/// In `args`, `%input` is the saved file and `%output` the output file (input with `output_extension`).
#[derive(Clone, Debug, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ExternalAction {
    pub name: String,
    pub program: std::path::PathBuf,
    pub args: String,
    /// Empty: the program changes the input in place (or produces nothing).
    pub output_extension: String,
    /// Run only for these extensions (empty = all).
    pub extensions: Vec<String>,
    pub enabled: bool,
    /// Hide the program's console window (Windows).
    pub hidden: bool,
}

impl ExternalAction {
    /// The argument list for `input`, split like a shell would (quotes group words) after
    /// substituting `%input` and `%output`.
    pub fn argv(&self, input: &std::path::Path) -> (Vec<String>, Option<std::path::PathBuf>) {
        let output = (!self.output_extension.is_empty()).then(|| input.with_extension(self.output_extension.trim_start_matches('.')));
        let mut out = Vec::new();
        let mut cur = String::new();
        let mut quoted = false;
        let mut started = false;
        for ch in self.args.chars() {
            match ch {
                '"' => {
                    quoted = !quoted;
                    started = true;
                }
                c if c.is_whitespace() && !quoted => {
                    if started {
                        out.push(std::mem::take(&mut cur));
                        started = false;
                    }
                }
                c => {
                    cur.push(c);
                    started = true;
                }
            }
        }
        if started {
            out.push(cur);
        }
        let inp = input.display().to_string();
        let outp = output.as_ref().map(|o| o.display().to_string()).unwrap_or_default();
        let argv = out.into_iter().map(|a| a.replace("%input", &inp).replace("%output", &outp)).collect();
        (argv, output)
    }

    pub fn applies_to(&self, path: &std::path::Path) -> bool {
        self.enabled
            && (self.extensions.is_empty()
                || path
                    .extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|e| self.extensions.iter().any(|x| x.trim_start_matches('.').eq_ignore_ascii_case(e))))
    }
}

#[cfg(test)]
mod action_tests {
    use super::*;

    #[test]
    fn argv_substitutes_and_quotes() {
        let a = ExternalAction {
            args: r#"-i "%input" -o %output --flag"#.into(),
            output_extension: "webp".into(),
            enabled: true,
            ..Default::default()
        };
        let (argv, out) = a.argv(std::path::Path::new("/s/a b.png"));
        assert_eq!(argv, vec!["-i", "/s/a b.png", "-o", "/s/a b.webp", "--flag"]);
        assert_eq!(out, Some(std::path::PathBuf::from("/s/a b.webp")));
        assert!(a.applies_to(std::path::Path::new("x.png")));
    }
}

/// The beautifier (`AfterCaptureTasks::BEAUTIFY_IMAGE`): padding, background, rounded corners, shadow.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct BeautifySettings {
    pub padding: u32,
    pub background: crate::color::Rgba,
    /// A second colour makes a diagonal gradient.
    pub background_end: Option<crate::color::Rgba>,
    pub corner_radius: u32,
    pub shadow_size: u32,
    pub shadow_opacity: f32,
}

impl Default for BeautifySettings {
    fn default() -> Self {
        BeautifySettings {
            padding: 64,
            background: crate::color::Rgba::rgb(0x6a, 0x5a, 0xcd),
            background_end: Some(crate::color::Rgba::rgb(0xff, 0x8c, 0x69)),
            corner_radius: 12,
            shadow_size: 24,
            shadow_opacity: 0.5,
        }
    }
}

/// Where uploads go and how results are reported.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UploadSettings {
    /// Destination ids from the upload registry, e.g. `imgur`, `sxcu:uguu`, `custom:My host`.
    pub image_destination: String,
    pub text_destination: String,
    pub file_destination: String,
    pub url_shortener: String,
    pub url_sharer: String,
    /// What "copy URL" puts on the clipboard (upload-info tokens such as `$url`, `$shorturl`).
    pub clipboard_format: String,
    /// Retries after the first attempt for network errors, HTTP 5xx and 429.
    pub retries: u32,
    /// Ask before uploading files larger than this (bytes); 0 = never.
    pub large_file_warning_bytes: u64,
}

impl Default for UploadSettings {
    fn default() -> Self {
        UploadSettings {
            image_destination: "imgur".into(),
            text_destination: "sxcu:paste-ee".into(),
            file_destination: "sxcu:uguu".into(),
            url_shortener: "sxcu:is-gd".into(),
            url_sharer: "reddit".into(),
            clipboard_format: "$url".into(),
            retries: 2,
            large_file_warning_bytes: 0,
        }
    }
}

/// Per-workflow overrides. A `None` section keeps the default (ADR 008).
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TaskSettingsPatch {
    pub after_capture: Option<AfterCaptureTasks>,
    pub beautify: Option<BeautifySettings>,
    pub effect_preset: Option<String>,
    pub upload: Option<UploadSettings>,
    pub recording: Option<crate::record::RecordingSettings>,
    pub after_upload: Option<AfterUploadTasks>,
    pub general: Option<GeneralSettings>,
    pub image: Option<ImageSettings>,
    pub capture: Option<CaptureOptions>,
    pub naming: Option<NamingSettings>,
    pub screenshots_dir: Option<std::path::PathBuf>,
}

impl TaskSettings {
    /// Field by field: a `Some` section in `patch` replaces the one in `defaults`.
    pub fn effective(defaults: &TaskSettings, patch: &TaskSettingsPatch) -> TaskSettings {
        TaskSettings {
            after_capture: patch.after_capture.unwrap_or(defaults.after_capture),
            after_upload: patch.after_upload.unwrap_or(defaults.after_upload),
            upload: patch.upload.clone().unwrap_or_else(|| defaults.upload.clone()),
            beautify: patch.beautify.clone().unwrap_or_else(|| defaults.beautify.clone()),
            effect_preset: patch.effect_preset.clone().unwrap_or_else(|| defaults.effect_preset.clone()),
            recording: patch.recording.clone().unwrap_or_else(|| defaults.recording.clone()),
            general: patch.general.clone().unwrap_or_else(|| defaults.general.clone()),
            image: patch.image.clone().unwrap_or_else(|| defaults.image.clone()),
            capture: patch.capture.clone().unwrap_or_else(|| defaults.capture.clone()),
            naming: patch.naming.clone().unwrap_or_else(|| defaults.naming.clone()),
            screenshots_dir: patch.screenshots_dir.clone().or_else(|| defaults.screenshots_dir.clone()),
        }
    }
}

impl TaskSettingsPatch {
    pub fn is_empty(&self) -> bool {
        self.overridden_sections().is_empty()
    }

    /// The sections this patch sets (`Some`), by name, for the settings UI.
    pub fn overridden_sections(&self) -> Vec<&'static str> {
        let flags = [
            (self.after_capture.is_some(), "after_capture"),
            (self.after_upload.is_some(), "after_upload"),
            (self.upload.is_some(), "upload"),
            (self.beautify.is_some(), "beautify"),
            (self.effect_preset.is_some(), "effects"),
            (self.recording.is_some(), "recording"),
            (self.general.is_some(), "general"),
            (self.image.is_some(), "image"),
            (self.capture.is_some(), "capture"),
            (self.naming.is_some(), "naming"),
            (self.screenshots_dir.is_some(), "screenshots_dir"),
        ];
        flags.into_iter().filter(|(set, _)| *set).map(|(_, name)| name).collect()
    }
}

/// A named Action with its hotkey and task-settings overrides.
#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Workflow {
    pub id: WorkflowId,
    pub name: String,
    pub action: Action,
    pub hotkey: Option<KeyChord>,
    pub enabled: bool,
    pub patch: TaskSettingsPatch,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    /// One flag for each of the 23 after-capture tasks (the set ShareX workflows use), plus Remove background.
    fn after_capture_has_24_flags() {
        assert_eq!(AfterCaptureTasks::all().bits().count_ones(), 24);
        assert_eq!(AfterCaptureTasks::all().bits(), (1 << 24) - 1);
    }

    #[test]
    fn empty_patch_keeps_defaults() {
        let defaults = TaskSettings::default();
        let patch = TaskSettingsPatch::default();
        assert!(patch.is_empty());
        assert!(patch.overridden_sections().is_empty());
        assert_eq!(TaskSettings::effective(&defaults, &patch), defaults);
    }

    #[test]
    fn effective_patch_wins_per_section() {
        let defaults = TaskSettings::default();
        let image = ImageSettings { format: ImageFormat::Jpeg, jpeg_quality: 70, ..ImageSettings::default() };
        let patch =
            TaskSettingsPatch { image: Some(image.clone()), after_upload: Some(AfterUploadTasks::empty()), ..TaskSettingsPatch::default() };

        let eff = TaskSettings::effective(&defaults, &patch);

        assert_eq!(eff.image, image);
        assert_eq!(eff.after_upload, AfterUploadTasks::empty());
        // Sections the patch leaves as None keep the default.
        assert_eq!(eff.after_capture, defaults.after_capture);
        assert_eq!(eff.general, defaults.general);
        assert_eq!(eff.capture, defaults.capture);
        assert_eq!(eff.naming, defaults.naming);
        assert_eq!(eff.screenshots_dir, None);
    }

    #[test]
    fn patch_screenshots_dir_overrides_default() {
        let defaults = TaskSettings { screenshots_dir: Some(PathBuf::from("/default")), ..TaskSettings::default() };
        let patch = TaskSettingsPatch { screenshots_dir: Some(PathBuf::from("/work")), ..TaskSettingsPatch::default() };
        assert_eq!(TaskSettings::effective(&defaults, &patch).screenshots_dir, Some(PathBuf::from("/work")));
        let none = TaskSettings::effective(&defaults, &TaskSettingsPatch::default());
        assert_eq!(none.screenshots_dir, Some(PathBuf::from("/default")));
    }

    #[test]
    fn overridden_sections_names_set_sections() {
        let patch = TaskSettingsPatch {
            image: Some(ImageSettings::default()),
            after_capture: Some(AfterCaptureTasks::empty()),
            ..TaskSettingsPatch::default()
        };
        assert!(!patch.is_empty());
        assert_eq!(patch.overridden_sections(), vec!["after_capture", "image"]);
    }
}

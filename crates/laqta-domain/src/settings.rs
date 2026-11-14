//! The settings value the application receives from `main`.

use std::path::PathBuf;

use crate::action::Action;
use crate::hotkey::{Key, KeyChord, Modifiers};
use crate::workflow::{TaskSettings, TaskSettingsPatch, Workflow, WorkflowId};

pub const SETTINGS_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Native upload host accounts (secrets live in the OS secret store).
    pub hosts: Vec<crate::hosts::HostAccount>,
    /// External programs for `PERFORM_ACTIONS`.
    pub actions: Vec<crate::workflow::ExternalAction>,
    /// Image effect presets, with `.sxie` import and export.
    pub effect_presets: Vec<crate::effects::Preset>,
    /// Editor tools hidden from the toolbar (tool names, e.g. "Spotlight"); their shortcuts still work.
    pub editor_hidden_tools: Vec<String>,
    /// The colour and size picked for each drawing tool on the capture toolbar.
    pub capture_marks: Vec<crate::capture::MarkStyle>,
    pub version: u32,
    pub screenshots_dir: PathBuf,
    pub defaults: TaskSettings,
    pub workflows: Vec<Workflow>,
    pub disable_hotkeys: bool,
    pub history: HistoryOptions,
    pub tray_click: Action,
    pub theme: Theme,
    /// UI language tag (`de`, `pt-BR`, …); `None` follows the system.
    pub language: Option<String>,
    /// Watch folders.
    pub watch_folders: Vec<WatchFolder>,
    /// Auto capture.
    pub auto_capture: AutoCaptureOptions,
    /// Local models (ADR 023): the model per task, custom models, CPU or GPU.
    pub models: ModelOptions,
    /// Cloud services: background removal provider and the image analyzer.
    pub cloud: CloudOptions,
    /// Windows only: start Laqta with administrator rights (one UAC prompt per start), so its
    /// shortcuts and capture reach elevated windows. Other platforms ignore it.
    pub run_as_admin: bool,
    /// Windows only: catch the shortcuts with a keyboard hook instead of `RegisterHotKey`, so they
    /// still work in games that take the keyboard for themselves. The hook takes the keys it
    /// matches. Other platforms ignore it.
    pub hotkeys_via_hook: bool,
}

/// Bounds for numbers the settings window takes as text. The window shows an error for a value outside
/// them and never changes it silently; the code that reads a setting clamps again as a last guard.
pub mod limits {
    /// JPEG quality, in percent.
    pub const JPEG_QUALITY: (u32, u32) = (1, 100);
    /// Recording frame rate.
    pub const FPS: (u32, u32) = (1, 240);
    /// GIF frame rate.
    pub const GIF_FPS: (u32, u32) = (1, 60);
    /// Camera picture width, in percent of the recording.
    pub const CAMERA_SIZE_PERCENT: (u32, u32) = (5, 100);
    /// How long a notification stays, in milliseconds.
    pub const TOAST_DURATION_MS: (u32, u32) = (500, 600_000);
    /// Upload retries after the first attempt.
    pub const RETRIES: (u32, u32) = (0, 10);
    /// Constant rate factor of x264 and friends.
    pub const CRF: (u32, u32) = (0, 63);
    /// Scrolling capture: frames, wheel notches per step, and the wait between steps in milliseconds.
    pub const SCROLL_FRAMES: (u32, u32) = (2, 500);
    pub const SCROLL_AMOUNT: (u32, u32) = (1, 50);
    pub const SCROLL_DELAY_MS: (u32, u32) = (50, 60_000);
}

impl AutoCaptureOptions {
    /// The shortest wait between two automatic captures, in milliseconds.
    pub const MIN_INTERVAL_MS: u32 = 1_000;
    /// The longest: a day. Larger values would overflow when the window converts seconds to milliseconds.
    pub const MAX_INTERVAL_MS: u32 = 86_400_000;
}

/// A setting the application refuses to save.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SettingsIssue {
    /// A screenshots folder that is not an absolute path: it would mean a different folder for every
    /// process, depending on where it was started. `workflow` is the shortcut that overrides the folder.
    RelativeScreenshotsDir { workflow: Option<WorkflowId>, path: PathBuf },
    /// An automatic capture interval outside [`AutoCaptureOptions::MIN_INTERVAL_MS`, `MAX_INTERVAL_MS`].
    AutoCaptureInterval { interval_ms: u32 },
}

impl std::fmt::Display for SettingsIssue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SettingsIssue::RelativeScreenshotsDir { path, .. } if path.as_os_str().is_empty() => {
                write!(f, "The screenshots folder can't be empty")
            }
            SettingsIssue::RelativeScreenshotsDir { path, .. } => {
                write!(f, "The screenshots folder must be a full path, not \"{}\"", path.display())
            }
            SettingsIssue::AutoCaptureInterval { interval_ms } => write!(
                f,
                "The auto-capture interval must be between {} and {} seconds, not {}",
                AutoCaptureOptions::MIN_INTERVAL_MS / 1000,
                AutoCaptureOptions::MAX_INTERVAL_MS / 1000,
                interval_ms / 1000
            ),
        }
    }
}

/// What is wrong with `settings`, in the order a person would fix it; empty when it can be saved.
pub fn validate(settings: &Settings) -> Vec<SettingsIssue> {
    let mut issues = Vec::new();
    let mut folder = |workflow: Option<WorkflowId>, path: &std::path::Path| {
        if !path.is_absolute() {
            issues.push(SettingsIssue::RelativeScreenshotsDir { workflow, path: path.to_path_buf() });
        }
    };
    folder(None, &settings.screenshots_dir);
    if let Some(dir) = &settings.defaults.screenshots_dir {
        folder(None, dir);
    }
    for w in &settings.workflows {
        if let Some(dir) = &w.patch.screenshots_dir {
            folder(Some(w.id), dir);
        }
    }
    let interval = settings.auto_capture.interval_ms;
    if !(AutoCaptureOptions::MIN_INTERVAL_MS..=AutoCaptureOptions::MAX_INTERVAL_MS).contains(&interval) {
        issues.push(SettingsIssue::AutoCaptureInterval { interval_ms: interval });
    }
    issues
}

/// Which background remover runs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BackgroundProvider {
    /// A local model (`models.background`).
    #[default]
    Local,
    /// remove.bg (API key in the secret store under `cloud:remove.bg`).
    RemoveBg,
}

/// An OpenAI-compatible chat endpoint (OpenAI, Gemini's OpenAI API, OpenRouter, Ollama, LM Studio).
/// The API key is in the secret store under `cloud:analyzer`.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AnalyzerOptions {
    pub base_url: String,
    pub model: String,
    pub prompt: String,
}

impl Default for AnalyzerOptions {
    fn default() -> Self {
        AnalyzerOptions {
            base_url: "https://api.openai.com/v1".into(),
            model: "gpt-4.1-mini".into(),
            prompt: "Describe this screenshot in a few sentences. List any text it contains.".into(),
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct CloudOptions {
    pub background: BackgroundProvider,
    pub analyzer: AnalyzerOptions,
}

/// Where local models run.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ModelProvider {
    /// The GPU when one is usable, else the CPU. No GPU provider ships yet, so this runs on the CPU.
    #[default]
    Auto,
    Cpu,
    Gpu,
}

/// Which engine reads the text in OCR tasks.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum OcrEngine {
    /// Laqta's downloaded models (PP-OCR).
    #[default]
    Models,
    /// The operating system's OCR (Windows.Media.Ocr); ignored where there is none.
    System,
}

/// A model file the user added for a supported family.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CustomModel {
    /// `custom-…`; the file is `<models>/<id>.onnx`.
    pub id: String,
    pub name: String,
    /// The catalog task name (`ocr-det`, `ocr-rec`, `background`).
    pub task: String,
    pub family: String,
}

/// The model chosen per task (catalog or custom id; `None` = the catalog default).
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ModelOptions {
    pub provider: ModelProvider,
    pub ocr_det: Option<String>,
    pub ocr_rec: Option<String>,
    /// The engine for text recognition. `System` is used where the OS has one.
    pub ocr_engine: OcrEngine,
    pub background: Option<String>,
    pub custom: Vec<CustomModel>,
}

/// A folder whose new files run a workflow.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct WatchFolder {
    pub enabled: bool,
    pub folder: PathBuf,
    /// Glob on the file name (`*.png`, `*`); several patterns separated by `;` or `|`.
    pub filter: String,
    pub include_subfolders: bool,
    /// Move the file into the screenshots folder (under its naming subfolder) before the workflow runs.
    pub move_to_screenshots: bool,
    /// The workflow whose settings the upload uses; `None` = the defaults.
    pub workflow: Option<crate::workflow::WorkflowId>,
}

impl Default for WatchFolder {
    fn default() -> Self {
        WatchFolder {
            enabled: true,
            folder: PathBuf::new(),
            filter: "*".into(),
            include_subfolders: false,
            move_to_screenshots: false,
            workflow: None,
        }
    }
}

/// Auto capture: run `action` every `interval_ms`, `count` times (0 = until stopped).
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct AutoCaptureOptions {
    pub action: Action,
    pub interval_ms: u32,
    pub count: u32,
    pub workflow: Option<crate::workflow::WorkflowId>,
}

impl Default for AutoCaptureOptions {
    fn default() -> Self {
        AutoCaptureOptions { action: Action::PrintScreen, interval_ms: 60_000, count: 0, workflow: None }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct HistoryOptions {
    pub enabled: bool,
    /// 0 means unlimited.
    pub max_items: u32,
}

impl Default for HistoryOptions {
    fn default() -> Self {
        HistoryOptions { enabled: true, max_items: 0 }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}

impl Default for Settings {
    fn default() -> Self {
        Self::default_for(PathBuf::from("Pictures"))
    }
}

impl Settings {
    /// screenshots_dir = `<pictures>/Laqta`, with five default hotkey workflows.
    pub fn default_for(pictures_dir: PathBuf) -> Settings {
        Settings {
            hosts: Vec::new(),
            actions: Vec::new(),
            effect_presets: Vec::new(),
            editor_hidden_tools: Vec::new(),
            capture_marks: Vec::new(),
            version: SETTINGS_VERSION,
            screenshots_dir: pictures_dir.join("Laqta"),
            defaults: TaskSettings::default(),
            workflows: default_workflows(),
            disable_hotkeys: false,
            history: HistoryOptions::default(),
            tray_click: Action::OpenMainWindow,
            theme: Theme::System,
            language: None,
            watch_folders: Vec::new(),
            auto_capture: AutoCaptureOptions::default(),
            models: ModelOptions::default(),
            cloud: CloudOptions::default(),
            run_as_admin: false,
            hotkeys_via_hook: false,
        }
    }

    pub fn m0_default(pictures_dir: PathBuf) -> Settings {
        Self::default_for(pictures_dir)
    }

    /// Enabled workflows that have a hotkey, in workflow order. Empty when `disable_hotkeys` is set.
    pub fn hotkey_bindings(&self) -> Vec<(KeyChord, Action)> {
        if self.disable_hotkeys {
            return Vec::new();
        }
        self.workflows.iter().filter(|w| w.enabled).filter_map(|w| w.hotkey.map(|chord| (chord, w.action))).collect()
    }

    pub fn workflow(&self, id: WorkflowId) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.id == id)
    }

    /// The workflow an action runs under when nothing names one (the tray, a Laqta window, the CLI):
    /// the first enabled workflow for that action.
    pub fn workflow_for_action(&self, action: Action) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.enabled && w.action == action)
    }

    /// The first enabled workflow with that hotkey.
    pub fn workflow_for_hotkey(&self, chord: KeyChord) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.enabled && w.hotkey == Some(chord))
    }

    /// Defaults merged with the workflow's patch (defaults alone if `workflow` is `None` or unknown).
    /// `screenshots_dir` is always `Some` in the result.
    pub fn task_settings(&self, workflow: Option<WorkflowId>) -> TaskSettings {
        let patch = workflow.and_then(|id| self.workflow(id)).map(|w| w.patch.clone()).unwrap_or_default();
        let mut ts = TaskSettings::effective(&self.defaults, &patch);
        ts.screenshots_dir = Some(self.screenshots_dir_for(&ts));
        ts
    }

    pub fn screenshots_dir_for(&self, ts: &TaskSettings) -> PathBuf {
        ts.screenshots_dir.clone().unwrap_or_else(|| self.screenshots_dir.clone())
    }
}

impl Settings {
    /// `default_for`, with the hotkeys `os` can type. Mac keyboards have no Print Screen key, and
    /// macOS keeps Command+Shift+3, 4 and 5 for its own screenshots, so there the five workflows
    /// are Control+Option+1 to 5.
    pub fn default_for_os(pictures_dir: PathBuf, os: crate::record::Os) -> Settings {
        let mut settings = Settings::default_for(pictures_dir);
        if os == crate::record::Os::MacOs {
            let digits = [Key::Digit1, Key::Digit2, Key::Digit3, Key::Digit4, Key::Digit5];
            for (workflow, key) in settings.workflows.iter_mut().zip(digits) {
                workflow.hotkey = Some(KeyChord { modifiers: Modifiers::CTRL | Modifiers::ALT, key });
            }
        }
        settings
    }
}

/// The default hotkey workflows, ids 1..=5.
fn default_workflows() -> Vec<Workflow> {
    let entries = [
        (1, Action::PrintScreen, Modifiers::empty()),
        (2, Action::RectangleRegion, Modifiers::CTRL),
        (3, Action::ActiveWindow, Modifiers::ALT),
        (4, Action::ScreenRecorder, Modifiers::SHIFT),
        (5, Action::ScreenRecorderGif, Modifiers::CTRL | Modifiers::SHIFT),
    ];
    entries
        .into_iter()
        .map(|(id, action, modifiers)| Workflow {
            id: WorkflowId(id),
            name: action.label().to_string(),
            action,
            hotkey: Some(KeyChord { modifiers, key: Key::PrintScreen }),
            enabled: true,
            patch: TaskSettingsPatch::default(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::ImageFormat;
    use crate::workflow::ImageSettings;
    use serde::Deserialize;

    #[test]
    fn validate_accepts_the_defaults_and_names_a_relative_screenshots_folder() {
        assert!(validate(&settings()).is_empty());
        let mut s = settings();
        s.screenshots_dir = PathBuf::from("Pictures/Laqta");
        s.workflows[1].patch.screenshots_dir = Some(PathBuf::from("shots"));
        s.defaults.screenshots_dir = Some(PathBuf::from(""));
        let id = s.workflows[1].id;
        assert_eq!(
            validate(&s),
            vec![
                SettingsIssue::RelativeScreenshotsDir { workflow: None, path: PathBuf::from("Pictures/Laqta") },
                SettingsIssue::RelativeScreenshotsDir { workflow: None, path: PathBuf::new() },
                SettingsIssue::RelativeScreenshotsDir { workflow: Some(id), path: PathBuf::from("shots") },
            ]
        );
        assert!(validate(&s)[0].to_string().contains("full path"));
        assert!(validate(&s)[1].to_string().contains("can't be empty"));
    }

    #[test]
    fn validate_bounds_the_auto_capture_interval() {
        let mut s = settings();
        for (ms, ok) in [(0, false), (999, false), (1_000, true), (86_400_000, true), (86_400_001, false), (u32::MAX, false)] {
            s.auto_capture.interval_ms = ms;
            assert_eq!(validate(&s).is_empty(), ok, "{ms} ms");
        }
        s.auto_capture.interval_ms = 0;
        assert!(validate(&s)[0].to_string().contains("between 1 and 86400 seconds"));
    }

    #[test]
    fn a_tray_click_opens_the_main_window() {
        assert_eq!(Settings::default().tray_click, Action::OpenMainWindow);
    }

    fn settings() -> Settings {
        Settings::default_for(PathBuf::from("/pics"))
    }

    #[test]
    fn macos_defaults_avoid_print_screen_and_stay_distinct() {
        use crate::record::Os;
        let mac = Settings::default_for_os("/p".into(), Os::MacOs);
        let chords: Vec<KeyChord> = mac.hotkey_bindings().iter().map(|(chord, _)| *chord).collect();
        assert_eq!(chords.len(), 5);
        assert!(chords.iter().all(|c| c.key != Key::PrintScreen && c.modifiers == Modifiers::CTRL | Modifiers::ALT));
        assert!(crate::hotkey::find_conflicts(&mac.hotkey_bindings()).is_empty());
        assert_eq!(Settings::default_for_os("/p".into(), Os::Linux), Settings::default_for("/p".into()));
    }

    #[test]
    fn default_for_builds_five_hotkey_workflows() {
        let s = settings();
        assert_eq!(s.version, SETTINGS_VERSION);
        assert_eq!(s.screenshots_dir, PathBuf::from("/pics/Laqta"));
        let ids: Vec<u32> = s.workflows.iter().map(|w| w.id.0).collect();
        assert_eq!(ids, vec![1, 2, 3, 4, 5]);
        assert!(s.workflows.iter().all(|w| w.enabled && w.patch.is_empty() && w.hotkey.is_some()));
        assert_eq!(s.hotkey_bindings().len(), 5);
        assert_eq!(s.hotkey_bindings()[0].0, KeyChord { modifiers: Modifiers::empty(), key: Key::PrintScreen });
    }

    #[test]
    fn hotkey_bindings_skip_disabled_and_respect_disable_hotkeys() {
        let mut s = settings();
        s.workflows[1].enabled = false;
        assert_eq!(s.hotkey_bindings().len(), 4);

        s.disable_hotkeys = true;
        assert!(s.hotkey_bindings().is_empty());
    }

    #[test]
    fn workflow_without_hotkey_gets_no_binding() {
        let mut s = settings();
        s.workflows[0].hotkey = None;
        assert_eq!(s.hotkey_bindings().len(), 4);
    }

    #[test]
    fn workflow_for_hotkey_returns_first_enabled_match() {
        let mut s = settings();
        let chord = KeyChord { modifiers: Modifiers::CTRL, key: Key::PrintScreen };
        assert_eq!(s.workflow_for_hotkey(chord).map(|w| w.id), Some(WorkflowId(2)));

        s.workflows[1].enabled = false;
        assert_eq!(s.workflow_for_hotkey(chord), None);
    }

    #[test]
    fn same_action_on_two_chords_resolves_to_each_workflow() {
        let mut s = settings();
        s.workflows[1].action = s.workflows[0].action;
        let plain = KeyChord { modifiers: Modifiers::empty(), key: Key::PrintScreen };
        let ctrl = KeyChord { modifiers: Modifiers::CTRL, key: Key::PrintScreen };
        assert_eq!(s.workflow_for_hotkey(plain).map(|w| w.id), Some(WorkflowId(1)));
        assert_eq!(s.workflow_for_hotkey(ctrl).map(|w| w.id), Some(WorkflowId(2)));
    }

    #[test]
    fn task_settings_resolves_screenshots_dir() {
        let mut s = settings();

        let defaults = s.task_settings(None);
        assert_eq!(defaults.screenshots_dir, Some(PathBuf::from("/pics/Laqta")));
        assert_eq!(s.task_settings(Some(WorkflowId(999))).screenshots_dir, Some(PathBuf::from("/pics/Laqta")));

        s.workflows[0].patch.screenshots_dir = Some(PathBuf::from("/elsewhere"));
        let ts = s.task_settings(Some(WorkflowId(1)));
        assert_eq!(ts.screenshots_dir, Some(PathBuf::from("/elsewhere")));
        assert_eq!(s.screenshots_dir_for(&ts), PathBuf::from("/elsewhere"));
    }

    #[test]
    fn task_settings_applies_workflow_patch() {
        let mut s = settings();
        s.workflows[0].patch.image = Some(ImageSettings { format: ImageFormat::Jpeg, ..ImageSettings::default() });
        assert_eq!(s.task_settings(Some(WorkflowId(1))).image.format, ImageFormat::Jpeg);
        assert_eq!(s.task_settings(Some(WorkflowId(2))).image.format, ImageFormat::Png);
    }

    #[test]
    fn run_as_admin_is_off_by_default() {
        assert!(!settings().run_as_admin);
        assert!(!Settings::default().run_as_admin);
    }

    #[test]
    fn hotkeys_via_hook_is_off_by_default() {
        assert!(!settings().hotkeys_via_hook);
        assert!(!Settings::default().hotkeys_via_hook);
    }

    #[test]
    fn empty_object_loads_defaults() {
        let empty = serde::de::value::MapDeserializer::<_, serde::de::value::Error>::new(std::iter::empty::<(&str, &str)>());
        let loaded = Settings::deserialize(empty).expect("an empty object is valid settings");
        assert_eq!(loaded, Settings::default());
    }
}

#[cfg(test)]
mod json_tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn settings_json_round_trip() {
        let s = Settings::default_for(PathBuf::from("/p"));
        let json = serde_json::to_string_pretty(&s).unwrap();
        let back: Settings = serde_json::from_str(&json).unwrap();
        assert_eq!(back, s);
        assert_eq!(back.workflows[1].name, "Capture region");
        let empty: Settings = serde_json::from_str("{}").unwrap();
        assert_eq!(empty.version, SETTINGS_VERSION);
    }

    #[test]
    fn run_as_admin_round_trips_and_old_files_load_without_it() {
        let mut s = Settings::default_for(PathBuf::from("/p"));
        s.run_as_admin = true;
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.run_as_admin);
        let mut value = serde_json::to_value(Settings::default_for(PathBuf::from("/p"))).unwrap();
        value.as_object_mut().unwrap().remove("run_as_admin");
        let old: Settings = serde_json::from_value(value).unwrap();
        assert!(!old.run_as_admin);
    }

    #[test]
    fn hotkeys_via_hook_round_trips_and_old_files_load_without_it() {
        let mut s = Settings::default_for(PathBuf::from("/p"));
        s.hotkeys_via_hook = true;
        let back: Settings = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert!(back.hotkeys_via_hook);
        let mut value = serde_json::to_value(Settings::default_for(PathBuf::from("/p"))).unwrap();
        value.as_object_mut().unwrap().remove("hotkeys_via_hook");
        let old: Settings = serde_json::from_value(value).unwrap();
        assert!(!old.hotkeys_via_hook);
    }
}

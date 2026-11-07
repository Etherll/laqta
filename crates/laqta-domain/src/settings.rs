//! The settings value the application receives from `main`.

use std::path::PathBuf;

use crate::action::Action;
use crate::hotkey::{Key, KeyChord, Modifiers};
use crate::workflow::{TaskSettings, TaskSettingsPatch, Workflow, WorkflowId};

pub const SETTINGS_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Settings {
    pub version: u32,
    pub screenshots_dir: PathBuf,
    pub defaults: TaskSettings,
    pub workflows: Vec<Workflow>,
    pub disable_hotkeys: bool,
    pub history: HistoryOptions,
    pub tray_click: Action,
    pub theme: Theme,
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
    /// screenshots_dir = `<pictures>/Laqta`, with ShareX's five default hotkey workflows.
    pub fn default_for(pictures_dir: PathBuf) -> Settings {
        Settings {
            version: SETTINGS_VERSION,
            screenshots_dir: pictures_dir.join("Laqta"),
            defaults: TaskSettings::default(),
            workflows: default_workflows(),
            disable_hotkeys: false,
            history: HistoryOptions::default(),
            tray_click: Action::PrintScreen,
            theme: Theme::System,
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
        self.workflows
            .iter()
            .filter(|w| w.enabled)
            .filter_map(|w| w.hotkey.map(|chord| (chord, w.action)))
            .collect()
    }

    pub fn workflow(&self, id: WorkflowId) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.id == id)
    }

    /// The first enabled workflow with that hotkey.
    pub fn workflow_for_hotkey(&self, chord: KeyChord) -> Option<&Workflow> {
        self.workflows.iter().find(|w| w.enabled && w.hotkey == Some(chord))
    }

    /// Defaults merged with the workflow's patch (defaults alone if `workflow` is `None` or unknown).
    /// `screenshots_dir` is always `Some` in the result.
    pub fn task_settings(&self, workflow: Option<WorkflowId>) -> TaskSettings {
        let patch = workflow
            .and_then(|id| self.workflow(id))
            .map(|w| w.patch.clone())
            .unwrap_or_default();
        let mut ts = TaskSettings::effective(&self.defaults, &patch);
        ts.screenshots_dir = Some(self.screenshots_dir_for(&ts));
        ts
    }

    pub fn screenshots_dir_for(&self, ts: &TaskSettings) -> PathBuf {
        ts.screenshots_dir.clone().unwrap_or_else(|| self.screenshots_dir.clone())
    }
}

/// ShareX's default hotkey workflows, ids 1..=5.
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

    fn settings() -> Settings {
        Settings::default_for(PathBuf::from("/pics"))
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
    fn empty_object_loads_defaults() {
        let empty = serde::de::value::MapDeserializer::<_, serde::de::value::Error>::new(
            std::iter::empty::<(&str, &str)>(),
        );
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
}

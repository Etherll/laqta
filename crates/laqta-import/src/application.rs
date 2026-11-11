//! `ApplicationConfig.json`: the default task settings, the screenshots folder, history and effect presets.

use std::path::PathBuf;

use laqta_domain::effects::Preset;
use laqta_domain::settings::WatchFolder;
use laqta_domain::workflow::TaskSettings;

use crate::json::{self, Object};
use crate::sxie;
use crate::task;
use crate::uploaders::Selections;

#[derive(Debug, Default)]
pub(crate) struct Application {
    pub defaults: Option<TaskSettings>,
    pub screenshots_dir: Option<PathBuf>,
    pub history_enabled: Option<bool>,
    pub presets: Vec<Preset>,
    pub watch_folders: Vec<WatchFolder>,
}

pub(crate) fn read(root: &Object, base: &TaskSettings, selections: &Selections, warnings: &mut Vec<String>) -> Application {
    let defaults_object = json::sub(root, "DefaultTaskSettings");
    Application {
        defaults: defaults_object
            .map(|object| task::overlay(base, object, selections, "ApplicationConfig.json DefaultTaskSettings", warnings)),
        screenshots_dir: screenshots_dir(root, warnings),
        history_enabled: json::flag(root, "HistorySaveTasks"),
        presets: presets(root, defaults_object, warnings),
        watch_folders: watch_folders(defaults_object, warnings),
    }
}

/// The custom folder when its switch is on, else ShareX's `ScreenshotsFolder`.
fn screenshots_dir(root: &Object, warnings: &mut Vec<String>) -> Option<PathBuf> {
    let custom = if json::flag(root, "UseCustomScreenshotsPath") == Some(true) { json::text(root, "CustomScreenshotsPath") } else { None };
    let folder = custom.or_else(|| json::text(root, "ScreenshotsFolder"))?;
    folder_of(folder, "ApplicationConfig.json", warnings)
}

/// A screenshots folder as ShareX wrote it, for the global setting or a task. A value with a `%` variable
/// is not expanded here, so it is skipped with a warning that names `what`.
pub(crate) fn folder_of(folder: &str, what: &str, warnings: &mut Vec<String>) -> Option<PathBuf> {
    if folder.contains('%') {
        warnings.push(format!(
            "{what}: the screenshots folder `{folder}` contains a % variable, which is not expanded; set the folder in Laqta"
        ));
        return None;
    }
    Some(PathBuf::from(folder))
}

/// The watch folders of the default task settings. `WatchFolderEnabled` switches them all, and it is off
/// when the key is missing.
fn watch_folders(defaults: Option<&Object>, warnings: &mut Vec<String>) -> Vec<WatchFolder> {
    let Some(defaults) = defaults else {
        return Vec::new();
    };
    let enabled = json::flag(defaults, "WatchFolderEnabled").unwrap_or(false);
    let mut folders = Vec::new();
    for item in json::list(defaults, "WatchFolderList") {
        let Some(object) = item.as_object() else {
            warnings.push("a watch folder is not an object; skipped".to_owned());
            continue;
        };
        let Some(folder) = json::text(object, "FolderPath") else {
            warnings.push("a watch folder has no FolderPath; skipped".to_owned());
            continue;
        };
        folders.push(WatchFolder {
            enabled,
            folder: PathBuf::from(folder),
            filter: json::text(object, "Filter").unwrap_or("*").to_owned(),
            include_subfolders: json::flag(object, "IncludeSubdirectories").unwrap_or(false),
            move_to_screenshots: json::flag(object, "MoveFilesToScreenshotsFolder").unwrap_or(false),
            ..WatchFolder::default()
        });
    }
    folders
}

/// Presets are read from the root and from the default image settings. A preset name keeps the first
/// copy.
fn presets(root: &Object, defaults: Option<&Object>, warnings: &mut Vec<String>) -> Vec<Preset> {
    let image_settings = defaults.and_then(|defaults| json::sub(defaults, "ImageSettings"));
    let mut sources = vec![json::list(root, "ImageEffectPresets")];
    if let Some(image_settings) = image_settings {
        sources.push(json::list(image_settings, "ImageEffectPresets"));
    }

    let mut presets: Vec<Preset> = Vec::new();
    for item in sources.into_iter().flatten() {
        match sxie::preset_from_value(item, warnings) {
            Some(preset) if presets.iter().any(|known| known.name == preset.name) => {}
            Some(preset) => presets.push(preset),
            None => warnings.push("an image effect preset is not an object, skipped".to_owned()),
        }
    }
    presets
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn custom_screenshots_folder_needs_the_switch() {
        let mut warnings = Vec::new();
        let on: Object = serde_json::from_str(r#"{"UseCustomScreenshotsPath": true, "CustomScreenshotsPath": "D:\\Shots"}"#).unwrap();
        assert_eq!(screenshots_dir(&on, &mut warnings), Some(PathBuf::from("D:\\Shots")));
        let off: Object = serde_json::from_str(r#"{"UseCustomScreenshotsPath": false, "CustomScreenshotsPath": "D:\\Shots"}"#).unwrap();
        assert_eq!(screenshots_dir(&off, &mut warnings), None);
        assert!(warnings.is_empty());
    }

    #[test]
    fn sharex_folder_is_the_fallback_and_variables_are_refused() {
        let mut warnings = Vec::new();
        let fallback: Object = serde_json::from_str(r#"{"UseCustomScreenshotsPath": false, "ScreenshotsFolder": "E:\\Captures"}"#).unwrap();
        assert_eq!(screenshots_dir(&fallback, &mut warnings), Some(PathBuf::from("E:\\Captures")));
        let variable: Object = serde_json::from_str(r#"{"ScreenshotsFolder": "%USERPROFILE%\\Shots"}"#).unwrap();
        assert_eq!(screenshots_dir(&variable, &mut warnings), None);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("%USERPROFILE%"));
    }
}

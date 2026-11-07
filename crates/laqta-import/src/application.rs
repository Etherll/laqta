//! `ApplicationConfig.json`: the default task settings, the screenshots folder, history and effect presets.

use std::path::PathBuf;

use laqta_domain::effects::Preset;
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
}

pub(crate) fn read(root: &Object, base: &TaskSettings, selections: &Selections, warnings: &mut Vec<String>) -> Application {
    let defaults_object = json::sub(root, "DefaultTaskSettings");
    Application {
        defaults: defaults_object
            .map(|object| task::overlay(base, object, selections, "ApplicationConfig.json DefaultTaskSettings", warnings)),
        screenshots_dir: screenshots_dir(root, warnings),
        history_enabled: json::flag(root, "HistorySaveTasks"),
        presets: presets(root, defaults_object, warnings),
    }
}

/// The custom folder when its switch is on, else ShareX's `ScreenshotsFolder`. A value with a `%`
/// variable is not expanded here, so it is skipped with a warning.
fn screenshots_dir(root: &Object, warnings: &mut Vec<String>) -> Option<PathBuf> {
    let custom = if json::flag(root, "UseCustomScreenshotsPath") == Some(true) { json::text(root, "CustomScreenshotsPath") } else { None };
    let folder = custom.or_else(|| json::text(root, "ScreenshotsFolder"))?;
    if folder.contains('%') {
        warnings.push(format!("the screenshots folder `{folder}` contains a % variable, which is not expanded; set the folder in Laqta"));
        return None;
    }
    Some(PathBuf::from(folder))
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

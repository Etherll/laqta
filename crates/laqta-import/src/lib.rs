//! Reads ShareX configuration, history, `.sxcu`, `.sxie` and backup archives into Laqta values.
//!
//! The functions read files and return values; nothing is written or fetched. What ShareX has and
//! Laqta cannot represent is reported in `ImportResult::warnings` and left out.
#![deny(unsafe_code)]

mod application;
mod backup;
#[allow(unsafe_code)]
mod dpapi;
mod effects;
mod hotkeys;
mod jobs;
mod json;
mod keys;
mod sxie;
mod task;
mod uploaders;

use std::path::Path;

use laqta_domain::effects::Preset;
use laqta_domain::hotkey::find_conflicts;
use laqta_domain::workflow::Workflow;
use laqta_domain::{KeyChord, Settings};

pub use backup::import_backup;
pub use sxie::{parse_sxie, parse_sxie_with_warnings, write_sxie};

/// What an import produced. `settings` is the base with ShareX's values applied.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportResult {
    pub settings: Settings,
    /// Custom uploaders (`.sxcu`) as JSON text, in ShareX's format. The registry parses them.
    pub custom_uploaders: Vec<String>,
    /// Secret values, keyed by `HostAccount::secret_key`.
    pub secrets: Vec<(String, String)>,
    /// The text of `History.json`, when the folder has one.
    pub history_json: Option<String>,
    /// Image effect presets, also copied into `settings.effect_presets` when there are any.
    pub presets: Vec<Preset>,
    pub warnings: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ImportError {
    #[error("cannot read {what}: {source}")]
    Io {
        what: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{file} is not valid JSON: {source}")]
    Json {
        file: String,
        #[source]
        source: serde_json::Error,
    },
    #[error("{what} is not a readable zip archive: {source}")]
    Zip {
        what: String,
        #[source]
        source: zip::result::ZipError,
    },
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Unsupported(String),
}

/// The text of each ShareX file that was found.
#[derive(Debug, Default)]
struct Files {
    application: Option<String>,
    hotkeys: Option<String>,
    uploaders: Option<String>,
    history: Option<String>,
}

impl Files {
    fn is_empty(&self) -> bool {
        self.application.is_none() && self.hotkeys.is_none() && self.uploaders.is_none() && self.history.is_none()
    }
}

/// Reads the ShareX settings folder on top of `base`. Each file is optional, but at least one must
/// be present.
pub fn import_dir(sharex_dir: &Path, base: Settings) -> Result<ImportResult, ImportError> {
    let files = Files {
        application: read_optional(&sharex_dir.join("ApplicationConfig.json"))?,
        hotkeys: read_optional(&sharex_dir.join("HotkeysConfig.json"))?,
        uploaders: read_optional(&sharex_dir.join("UploadersConfig.json"))?,
        history: read_optional(&sharex_dir.join("History.json"))?,
    };
    if files.is_empty() {
        return Err(ImportError::Invalid(format!("no ShareX configuration files in {}", sharex_dir.display())));
    }
    import_files(files, base)
}

fn read_optional(path: &Path) -> Result<Option<String>, ImportError> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(source) => Err(ImportError::Io { what: path.display().to_string(), source }),
    }
}

/// Applies ShareX's files to `base`. Shared by the folder and backup readers.
fn import_files(files: Files, base: Settings) -> Result<ImportResult, ImportError> {
    let Files { application, hotkeys, uploaders, history } = files;
    let mut warnings = Vec::new();
    let mut settings = base;

    // Uploaders first: the selected custom uploaders are needed to map destinations.
    let uploaders = match uploaders {
        Some(text) => Some(uploaders::read(&json::root_object("UploadersConfig.json", &text)?, &mut warnings)),
        None => None,
    };
    let selections = uploaders.as_ref().map(|found| found.selections.clone()).unwrap_or_default();

    let mut presets = Vec::new();
    if let Some(text) = application {
        let root = json::root_object("ApplicationConfig.json", &text)?;
        let found = application::read(&root, &settings.defaults, &selections, &mut warnings);
        if let Some(defaults) = found.defaults {
            settings.defaults = defaults;
        }
        if let Some(dir) = found.screenshots_dir {
            settings.screenshots_dir = dir;
        }
        if let Some(enabled) = found.history_enabled {
            settings.history.enabled = enabled;
        }
        if !found.presets.is_empty() {
            settings.effect_presets = found.presets.clone();
        }
        presets = found.presets;
    }

    if let Some(text) = hotkeys {
        let root = json::root_object("HotkeysConfig.json", &text)?;
        let workflows = hotkeys::read(&root, &settings.defaults, &selections, &mut warnings);
        report_conflicts(&workflows, &mut warnings);
        settings.workflows = workflows;
    }

    let mut custom_uploaders = Vec::new();
    let mut secrets = Vec::new();
    if let Some(found) = uploaders {
        settings.hosts = found.hosts;
        custom_uploaders = found.custom;
        secrets = found.secrets;
    }

    Ok(ImportResult {
        settings,
        custom_uploaders,
        secrets,
        history_json: history.map(|text| json::strip_bom(&text).to_owned()),
        presets,
        warnings,
    })
}

fn report_conflicts(workflows: &[Workflow], warnings: &mut Vec<String>) {
    let bindings: Vec<(KeyChord, &str)> =
        workflows.iter().filter_map(|workflow| workflow.hotkey.map(|chord| (chord, workflow.name.as_str()))).collect();
    for (chord, indexes) in find_conflicts(&bindings) {
        let names: Vec<&str> = indexes.iter().map(|&index| bindings[index].1).collect();
        warnings.push(format!("hotkey {chord} is used by several tasks ({}); the first one wins", names.join(", ")));
    }
}

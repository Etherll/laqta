//! Imports from ShareX (M6 D7): a configuration folder or `.sxb` backup, a `.sxcu` custom uploader and
//! a `.sxie` effect preset. Settings go through the same save path as the settings window.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use laqta_app::App;
use laqta_domain::Settings;
use laqta_library::History;
use laqta_ports::SecretStore;
use laqta_upload::uploaders::SharedRegistry;

pub type SaveSettings = Arc<dyn Fn(Settings) -> Result<(), String> + Send + Sync>;

#[derive(Clone)]
pub struct Importer {
    pub app: Arc<App>,
    pub save: SaveSettings,
    pub uploaders: Arc<SharedRegistry>,
    pub uploaders_dir: PathBuf,
    pub secrets: Arc<dyn SecretStore>,
    pub history: Option<Arc<History>>,
}

impl Importer {
    pub fn uploader(&self, path: &Path) -> Result<String, String> {
        let json = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        crate::uploads::add_custom(&self.uploaders, &self.uploaders_dir, &json)
    }

    /// Adds the preset, replacing one with the same name.
    pub fn effect(&self, path: &Path) -> Result<String, String> {
        let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let preset = laqta_import::parse_sxie(&bytes).map_err(|e| e.to_string())?;
        let name = preset.name.clone();
        let mut s = self.app.settings();
        s.effect_presets.retain(|p| p.name != name);
        s.effect_presets.push(preset);
        (self.save)(s)?;
        Ok(name)
    }

    /// A ShareX folder (with `ApplicationConfig.json` and friends) or a `.sxb` backup.
    pub fn sharex(&self, path: &Path) -> Result<String, String> {
        let base = self.app.settings();
        let result = if path.is_dir() { laqta_import::import_dir(path, base) } else { laqta_import::import_backup(path, base) }
            .map_err(|e| e.to_string())?;
        let mut warnings = result.warnings.clone();
        let mut uploaders = 0;
        for json in &result.custom_uploaders {
            match crate::uploads::add_custom(&self.uploaders, &self.uploaders_dir, json) {
                Ok(_) => uploaders += 1,
                Err(e) => warnings.push(format!("custom uploader skipped: {e}")),
            }
        }
        for (key, secret) in &result.secrets {
            if let Err(e) = self.secrets.set(key, secret) {
                warnings.push(format!("secret {key} not saved: {e}"));
            }
        }
        let mut history = String::new();
        if let (Some(json), Some(h)) = (&result.history_json, &self.history) {
            match laqta_library::import_sharex_history(h, json) {
                Ok(rep) => history = format!(", {} history items", rep.imported),
                Err(e) => warnings.push(format!("history not imported: {e}")),
            }
        }
        let workflows = result.settings.workflows.len();
        (self.save)(result.settings)?;
        for w in &warnings {
            tracing::warn!("ShareX import: {w}");
        }
        Ok(format!(
            "Imported {workflows} workflows, {uploaders} custom uploaders, {} presets{history}.{}",
            result.presets.len(),
            if warnings.is_empty() { String::new() } else { format!(" {} items could not be brought over (see the log).", warnings.len()) }
        ))
    }
}

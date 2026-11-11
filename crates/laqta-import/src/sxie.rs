//! `.sxie` image-effect packages: a zip whose `Config.json` is ShareX's `ImageEffectPreset`, next to the
//! image files some effects use. Only `Config.json` is read and written. Image files are neither
//! extracted nor packed.

use std::io::{Cursor, Write};

use laqta_domain::effects::{EffectItem, Preset};
use serde_json::{Map, Value};
use zip::write::SimpleFileOptions;
use zip::{ZipArchive, ZipWriter};

use crate::backup::{basename, entry};
use crate::json::{self, Object};
use crate::{ImportError, effects};

const PACKAGE: &str = "the .sxie package";

/// Reads a `.sxie` package. Effects with no Laqta equivalent are reported with a `tracing` warning and
/// skipped; see `parse_sxie_with_warnings` for the list.
pub fn parse_sxie(bytes: &[u8]) -> Result<Preset, ImportError> {
    let (preset, warnings) = parse_sxie_with_warnings(bytes)?;
    for warning in &warnings {
        tracing::warn!("{warning}");
    }
    Ok(preset)
}

/// Reads a `.sxie` package and also returns what was skipped: effects with no Laqta equivalent, and
/// image files the effects use that sit inside the package (they are not extracted).
pub fn parse_sxie_with_warnings(bytes: &[u8]) -> Result<(Preset, Vec<String>), ImportError> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).map_err(|source| ImportError::Zip { what: PACKAGE.to_owned(), source })?;
    let config = entry(&mut archive, "Config.json", PACKAGE)?
        .ok_or_else(|| ImportError::Invalid("the .sxie package has no Config.json".to_owned()))?;
    let value = json::parse_value("Config.json", &config)?;
    let mut warnings = Vec::new();
    let preset = preset_from_value(&value, &mut warnings)
        .ok_or_else(|| ImportError::Invalid("Config.json is not an image effect preset".to_owned()))?;

    let packaged: Vec<String> = archive.file_names().filter_map(|name| name.ok().map(|name| name.into_owned())).collect();
    for item in &preset.effects {
        if let Some(path) = effects::asset_path(&item.effect) {
            let wanted = basename(path);
            if packaged.iter().any(|name| basename(name).eq_ignore_ascii_case(wanted)) {
                warnings.push(format!("image file `{path}` is inside the .sxie package and was not extracted"));
            }
        }
    }
    Ok((preset, warnings))
}

/// Writes a preset as a `.sxie` package. Effects that have no confirmed ShareX class are refused, not
/// written under a guessed name, because ShareX cannot load a package with an unknown class.
pub fn write_sxie(preset: &Preset) -> Result<Vec<u8>, ImportError> {
    let config = serde_json::to_string_pretty(&preset_to_value(preset)?)
        .map_err(|source| ImportError::Json { file: "Config.json".to_owned(), source })?;
    let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("Config.json", SimpleFileOptions::default())
        .map_err(|source| ImportError::Zip { what: PACKAGE.to_owned(), source })?;
    writer.write_all(config.as_bytes()).map_err(|source| ImportError::Io { what: PACKAGE.to_owned(), source })?;
    let cursor = writer.finish().map_err(|source| ImportError::Zip { what: PACKAGE.to_owned(), source })?;
    Ok(cursor.into_inner())
}

/// A preset from ShareX's `ImageEffectPreset` JSON. Unknown effects are pushed to `warnings`. `None` when
/// the value is not an object.
pub(crate) fn preset_from_value(value: &Value, warnings: &mut Vec<String>) -> Option<Preset> {
    let object = value.as_object()?;
    let name = object.get("Name").and_then(Value::as_str).unwrap_or_default().to_owned();
    let mut items = Vec::new();
    for item in json::list(object, "Effects") {
        let Some(fields) = item.as_object() else {
            warnings.push("an image effect is not an object; skipped".to_owned());
            continue;
        };
        let type_name = fields.get("$type").and_then(Value::as_str).unwrap_or_default();
        let class = class_name(type_name);
        match effects::parse(class, fields) {
            Some(effect) => items.push(EffectItem { effect, enabled: json::flag(fields, "Enabled").unwrap_or(true) }),
            None => warnings.push(format!("image effect `{class}` has no Laqta equivalent; skipped")),
        }
    }
    Some(Preset { name, effects: items })
}

/// `ShareX.ImageEffectsLib.Brightness, ShareX.ImageEffectsLib` → `Brightness`.
fn class_name(type_name: &str) -> &str {
    type_name.split(',').next().unwrap_or_default().trim().rsplit('.').next().unwrap_or_default()
}

fn preset_to_value(preset: &Preset) -> Result<Value, ImportError> {
    let mut items = Vec::with_capacity(preset.effects.len());
    for item in &preset.effects {
        let (class, mut fields): (&str, Object) = effects::write(&item.effect)
            .ok_or_else(|| ImportError::Unsupported(format!("the `{}` effect has no ShareX class to write", item.effect.name())))?;
        // ShareX's binder resolves only the bare class name (`KnownTypesSerializationBinder`).
        fields.insert("$type".to_owned(), Value::String(class.to_owned()));
        fields.insert("Enabled".to_owned(), Value::Bool(item.enabled));
        items.push(Value::Object(fields));
    }
    let mut root = Map::new();
    root.insert("Name".to_owned(), Value::String(preset.name.clone()));
    root.insert("Effects".to_owned(), Value::Array(items));
    Ok(Value::Object(root))
}

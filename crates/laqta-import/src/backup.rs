//! `.sxb` backups: a zip archive of ShareX's configuration files.

use std::fs::File;
use std::io::{BufReader, Read, Seek};
use std::path::Path;

use laqta_domain::Settings;
use zip::ZipArchive;

use crate::{Files, ImportError, ImportResult, import_files};

/// Reads the configuration files from a `.sxb` backup on top of `base`. The files may sit at the root of
/// the archive or in a folder.
pub fn import_backup(sxb: &Path, base: Settings) -> Result<ImportResult, ImportError> {
    let what = sxb.display().to_string();
    let file = File::open(sxb).map_err(|source| ImportError::Io { what: what.clone(), source })?;
    let mut archive = ZipArchive::new(BufReader::new(file)).map_err(|source| ImportError::Zip { what: what.clone(), source })?;
    let files = Files {
        application: entry(&mut archive, "ApplicationConfig.json", &what)?,
        hotkeys: entry(&mut archive, "HotkeysConfig.json", &what)?,
        uploaders: entry(&mut archive, "UploadersConfig.json", &what)?,
        history: entry(&mut archive, "History.json", &what)?,
    };
    if files.is_empty() {
        return Err(ImportError::Invalid(format!("{what} has no ShareX configuration files")));
    }
    import_files(files, base)
}

/// The text of the archive entry whose file name is `file_name`, preferring the entry nearest the root.
pub(crate) fn entry<R: Read + Seek>(archive: &mut ZipArchive<R>, file_name: &str, what: &str) -> Result<Option<String>, ImportError> {
    let names: Vec<String> = archive.file_names().filter_map(|name| name.ok().map(|name| name.into_owned())).collect();
    let Some(name) = names
        .iter()
        .filter(|name| basename(name).eq_ignore_ascii_case(file_name))
        .min_by_key(|name| name.matches(['/', '\\']).count())
        .cloned()
    else {
        return Ok(None);
    };
    let label = format!("{name} in {what}");
    let mut file = archive.by_name(&name).map_err(|source| ImportError::Zip { what: label.clone(), source })?;
    let mut text = String::new();
    file.read_to_string(&mut text).map_err(|source| ImportError::Io { what: label, source })?;
    Ok(Some(text))
}

/// The last part of a path that uses either separator.
pub(crate) fn basename(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

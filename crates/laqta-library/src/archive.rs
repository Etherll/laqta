//! The data-folder archive (docs/FEATURES.md, Application: "export / import archive"): a zip of the
//! settings, the history database, custom uploaders and backups, with paths relative to the data
//! folder.

use std::borrow::Cow;
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};

use zip::result::ZipError;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use crate::{History, LibraryError};

const SETTINGS: &str = "settings.json";
const HISTORY_DB: &str = "history.db";
const UPLOADERS: &str = "uploaders";
const BACKUPS: &str = "backups";
/// Where an import keeps the settings file it replaces.
const SETTINGS_BEFORE_IMPORT: &str = "backups/settings-before-import.json";

/// Writes a zip of `settings.json`, `history.db`, `uploaders/*.sxcu` and everything under `backups/`,
/// with entry names relative to `data_dir`. A part that does not exist is left out.
///
/// With a `history`, its write-ahead log is checkpointed first, so `history.db` holds every committed
/// entry. Without one, a write-ahead log that is still live is not included, so export when the app is
/// not writing. The zip is written to `<dest>.part` and renamed into place.
pub fn export_archive(data_dir: &Path, dest_zip: &Path, history: Option<&History>) -> Result<(), LibraryError> {
    if let Some(history) = history {
        history.checkpoint()?;
    }
    let mut entries = Vec::new();
    for name in [SETTINGS, HISTORY_DB] {
        let path = data_dir.join(name);
        if path.is_file() {
            entries.push((name.to_owned(), path));
        }
    }
    collect(&data_dir.join(UPLOADERS), UPLOADERS, false, is_sxcu, &mut entries)?;
    collect(&data_dir.join(BACKUPS), BACKUPS, true, |_| true, &mut entries)?;
    entries.sort();

    let part = part_path(dest_zip);
    if let Err(error) = write_zip(&part, &entries) {
        let _ = fs::remove_file(&part);
        return Err(error);
    }
    fs::rename(&part, dest_zip)?;
    Ok(())
}

/// Extracts the known parts of an archive made by [`export_archive`] into `data_dir` and returns the
/// files it wrote. Entries with other names are skipped.
///
/// An entry whose name is absolute, contains `..`, or contains a backslash or colon fails the whole
/// import before anything is written. An existing `settings.json` is copied to
/// `backups/settings-before-import.json` first, whatever the archive holds. The archive's own copy of
/// that file is skipped, so it never replaces the backup just made. An imported `history.db` replaces
/// the old database together with its `-wal` and `-shm` files. Run the import with no `History` open
/// on `data_dir`.
pub fn import_archive(zip_path: &Path, data_dir: &Path) -> Result<Vec<PathBuf>, LibraryError> {
    let mut archive = ZipArchive::new(BufReader::new(File::open(zip_path)?))?;
    let names = archive.file_names().map(|name| name.map(Cow::into_owned)).collect::<Result<Vec<String>, ZipError>>()?;

    // Check every name before writing anything, so a hostile archive changes nothing.
    if let Some(name) = names.iter().find(|name| !is_safe(name)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("archive entry {name:?} would leave the data folder")).into());
    }
    let plan: Vec<usize> =
        names.iter().enumerate().filter(|(_, name)| !name.ends_with('/') && is_known(name)).map(|(index, _)| index).collect();

    let mut written = Vec::new();
    if data_dir.join(SETTINGS).is_file() {
        fs::create_dir_all(data_dir.join(BACKUPS))?;
        let backup = data_dir.join(SETTINGS_BEFORE_IMPORT);
        fs::copy(data_dir.join(SETTINGS), &backup)?;
        written.push(backup);
    }
    for index in plan {
        let name = &names[index];
        if name == HISTORY_DB {
            remove_history_sidecars(data_dir)?;
        }
        let dest = data_dir.join(name);
        extract(&mut archive, index, &dest)?;
        written.push(dest);
    }
    Ok(written)
}

/// Adds the files in `dir` as `prefix/<name>` entries. Subfolders are followed only when `recurse`
/// is set, and only files that `keep` accepts are added. A missing folder adds nothing. Symbolic
/// links are skipped.
fn collect(dir: &Path, prefix: &str, recurse: bool, keep: fn(&str) -> bool, out: &mut Vec<(String, PathBuf)>) -> Result<(), LibraryError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            tracing::warn!(path = %entry.path().display(), "archive: skipping a file name that is not UTF-8");
            continue;
        };
        let file_type = entry.file_type()?;
        if file_type.is_dir() {
            if recurse {
                collect(&entry.path(), &format!("{prefix}/{name}"), true, keep, out)?;
            }
        } else if file_type.is_file() && keep(&name) {
            out.push((format!("{prefix}/{name}"), entry.path()));
        }
    }
    Ok(())
}

fn write_zip(path: &Path, entries: &[(String, PathBuf)]) -> Result<(), LibraryError> {
    let mut zip = ZipWriter::new(BufWriter::new(File::create(path)?));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, source) in entries {
        zip.start_file(name.as_str(), options)?;
        io::copy(&mut File::open(source)?, &mut zip)?;
    }
    let file = zip.finish()?.into_inner().map_err(io::Error::from)?;
    file.sync_all()?;
    Ok(())
}

/// Writes one entry to `dest` through a `.part` file, so a failed copy leaves no partial file behind.
fn extract(archive: &mut ZipArchive<BufReader<File>>, index: usize, dest: &Path) -> Result<(), LibraryError> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut entry = archive.by_index(index)?;
    let part = part_path(dest);
    let mut file = File::create(&part)?;
    io::copy(&mut entry, &mut file)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&part, dest)?;
    Ok(())
}

/// The entry names an export writes: `settings.json`, `history.db`, `uploaders/<name>.sxcu`, and any
/// file under `backups/`.
fn is_known(name: &str) -> bool {
    if name == SETTINGS || name == HISTORY_DB {
        return true;
    }
    if name == SETTINGS_BEFORE_IMPORT {
        // The importer writes this file itself. An archived copy must not replace the backup it just made.
        return false;
    }
    if let Some(file) = name.strip_prefix("uploaders/") {
        return !file.contains('/') && is_sxcu(file);
    }
    name.strip_prefix("backups/").is_some_and(|rest| !rest.is_empty())
}

/// A relative name with `/` separators only, no `.` or `..` part, and no backslash, colon or NUL.
/// Joined to a folder, such a name stays inside it on every platform.
fn is_safe(name: &str) -> bool {
    !name.starts_with('/') && !name.contains(['\\', ':', '\0']) && name.split('/').all(|part| part != ".." && part != ".")
}

fn is_sxcu(name: &str) -> bool {
    Path::new(name).extension().is_some_and(|ext| ext.eq_ignore_ascii_case("sxcu"))
}

/// Removes the write-ahead log and shared-memory files that belong to the database being replaced.
/// Left in place, they would be applied to the imported file.
fn remove_history_sidecars(data_dir: &Path) -> io::Result<()> {
    for suffix in ["-wal", "-shm"] {
        match fs::remove_file(data_dir.join(format!("{HISTORY_DB}{suffix}"))) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn part_path(dest: &Path) -> PathBuf {
    let mut part = dest.as_os_str().to_owned();
    part.push(".part");
    PathBuf::from(part)
}

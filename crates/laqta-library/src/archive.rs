//! The data-folder archive: a zip of the
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

/// Where a staged import waits for the next start.
const STAGED: &str = "import-pending.zip";

/// Checks `zip_path` the way an import would and keeps a copy of it for the next start, where
/// [`apply_staged_import`] extracts it before the history database opens. Nothing in `data_dir` that
/// the running app uses is touched, so an open database or a later save of the settings cannot spoil it.
/// Returns how many known entries the archive holds; an archive with none, or one with an unsafe
/// entry name, is refused and nothing is staged. A second call replaces the staged archive.
pub fn stage_archive(zip_path: &Path, data_dir: &Path) -> Result<usize, LibraryError> {
    // The copy is the one checked, so what the next start applies is what was checked.
    fs::create_dir_all(data_dir)?;
    let staged = data_dir.join(STAGED);
    let part = part_path(&staged);
    fs::copy(zip_path, &part)?;
    let known = match check_archive(&part) {
        Ok(known) => known,
        Err(error) => {
            let _ = fs::remove_file(&part);
            return Err(error);
        }
    };
    // Stamped now: a copy on Windows keeps the source zip's time, and the pre-import backup is judged
    // against the staging time (see `apply_archive`).
    let copy = File::options().write(true).open(&part)?;
    copy.set_modified(std::time::SystemTime::now())?;
    copy.sync_all()?;
    drop(copy);
    fs::rename(&part, &staged)?;
    Ok(known)
}

/// Opens `zip_path` and reads every entry, as an import would. Returns how many known entries it holds.
fn check_archive(zip_path: &Path) -> Result<usize, LibraryError> {
    let mut archive = ZipArchive::new(BufReader::new(File::open(zip_path)?))?;
    let names = archive.file_names().map(|name| name.map(Cow::into_owned)).collect::<Result<Vec<String>, ZipError>>()?;
    if let Some(name) = names.iter().find(|name| !is_safe(name)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("archive entry {name:?} would leave the data folder")).into());
    }
    // Two entries for one file would land on the same path: the second would fail after the first was
    // applied, and the import would be tried again at every start. Windows and macOS fold case.
    let mut seen = std::collections::HashSet::new();
    if let Some(name) = names.iter().filter(|name| !name.ends_with('/')).find(|name| !seen.insert(name.to_lowercase())) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("the archive holds {name:?} twice")).into());
    }
    let known = names.iter().filter(|name| !name.ends_with('/') && is_known(name)).count();
    if known == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "the archive holds no Laqta settings, history or uploaders").into());
    }
    // Every entry must be readable now, so a damaged archive is refused here and not at the next start.
    for index in 0..archive.len() {
        io::copy(&mut archive.by_index(index)?, &mut io::sink())?;
    }
    Ok(known)
}

/// Whether an import waits for the next start.
pub fn has_staged_import(data_dir: &Path) -> bool {
    data_dir.join(STAGED).is_file()
}

/// A scratch folder under the data folder where a staged import is unpacked before anything moves.
const UNPACKING: &str = "import-unpacking";

/// Applies the archive [`stage_archive`] kept, if any. Call it before the history database or the
/// settings are opened. Returns the files written, or `None` when nothing was staged.
///
/// Everything is unpacked into a folder under `data_dir` first and only then renamed into place
/// (the settings last), so a failure while unpacking changes nothing. The history database's
/// `-wal` and `-shm` files are removed after `history.db` is in place, not before.
///
/// The staged archive is removed once it was applied, and when it is bad (damaged, or with an unsafe
/// entry name): retrying would fail the same way at every start. An I/O error (a locked file, a full
/// disk) keeps it, so the next start tries again.
pub fn apply_staged_import(data_dir: &Path) -> Result<Option<Vec<PathBuf>>, LibraryError> {
    let staged = data_dir.join(STAGED);
    if !staged.is_file() {
        return Ok(None);
    }
    let result = apply_archive(&staged, data_dir);
    let _ = fs::remove_dir_all(data_dir.join(UNPACKING));
    match &result {
        Ok(_) => retire_staged(&staged),
        Err(error) if is_bad_archive(error) => retire_staged(&staged),
        Err(_) => {}
    }
    result.map(Some)
}

/// Takes the staged archive out of the way. One that cannot be removed is renamed instead, since left
/// in place it would be applied again at the next start, over whatever changed since.
fn retire_staged(staged: &Path) {
    if let Err(error) = fs::remove_file(staged) {
        let applied = staged.with_extension("applied");
        let _ = fs::remove_file(&applied);
        if let Err(rename) = fs::rename(staged, &applied) {
            tracing::error!(%error, %rename, path = %staged.display(), "archive: the applied import could not be removed and will be applied again");
        }
    }
}

/// Where the replaced files are kept when a rollback could not put them all back, so none is lost.
const KEPT: &str = "import-kept";

/// Whether `path` exists and was modified no earlier than `than`.
fn is_newer(path: &Path, than: &Path) -> bool {
    let modified = |p: &Path| fs::metadata(p).and_then(|m| m.modified()).ok();
    matches!((modified(path), modified(than)), (Some(a), Some(b)) if a >= b)
}

/// Whether `error` says the archive itself cannot be applied, as opposed to the folder it goes to.
fn is_bad_archive(error: &LibraryError) -> bool {
    match error {
        // A read error under the zip reader is about the disk, not the archive: it is judged as one.
        LibraryError::Zip(ZipError::Io(error)) => matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof),
        LibraryError::Zip(_) => true,
        // A truncated entry ends early; reading it again at the next start would end the same way.
        LibraryError::Io(error) => matches!(error.kind(), io::ErrorKind::InvalidData | io::ErrorKind::UnexpectedEof),
        _ => false,
    }
}

fn apply_archive(zip_path: &Path, data_dir: &Path) -> Result<Vec<PathBuf>, LibraryError> {
    let mut archive = ZipArchive::new(BufReader::new(File::open(zip_path)?))?;
    let names = archive.file_names().map(|name| name.map(Cow::into_owned)).collect::<Result<Vec<String>, ZipError>>()?;
    if let Some(name) = names.iter().find(|name| !is_safe(name)) {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("archive entry {name:?} would leave the data folder")).into());
    }
    // The settings go last: until they are in place a retry starts from the same files.
    let mut plan: Vec<usize> =
        names.iter().enumerate().filter(|(_, name)| !name.ends_with('/') && is_known(name)).map(|(index, _)| index).collect();
    plan.sort_by_key(|index| names[*index] == SETTINGS);

    let unpacking = data_dir.join(UNPACKING);
    let _ = fs::remove_dir_all(&unpacking);
    fs::create_dir_all(&unpacking)?;
    for index in &plan {
        extract(&mut archive, *index, &unpacking.join(&names[*index]))?;
    }

    let mut written = Vec::new();
    if data_dir.join(SETTINGS).is_file() {
        fs::create_dir_all(data_dir.join(BACKUPS))?;
        let backup = data_dir.join(SETTINGS_BEFORE_IMPORT);
        // A backup newer than the staged archive was made for this import by a start that stopped
        // before it finished; the settings now may be the imported ones, so that backup stays.
        if !is_newer(&backup, zip_path) {
            fs::copy(data_dir.join(SETTINGS), &backup)?;
        }
        written.push(backup);
    }
    // Each file a rename replaces is set aside first, so a rename that fails halfway puts every
    // replaced file back: the profile is either the imported one or the one it was, never a mix.
    let previous = unpacking.join(".previous");
    let mut moved: Vec<(PathBuf, Option<PathBuf>)> = Vec::new();
    let mut history = false;
    let mut place = |name: &str| -> io::Result<()> {
        let dest = data_dir.join(name);
        if let Some(parent) = dest.parent() {
            fs::create_dir_all(parent)?;
        }
        let old = if dest.is_file() {
            let old = previous.join(name);
            if let Some(parent) = old.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::rename(&dest, &old)?;
            Some(old)
        } else {
            None
        };
        moved.push((dest.clone(), old));
        fs::rename(unpacking.join(name), &dest)?;
        history |= name == HISTORY_DB;
        Ok(())
    };
    let placed = plan.iter().try_for_each(|index| place(&names[*index])).and_then(|()| {
        // The old database's log would be read into the imported one, so it is set aside too, and
        // put back like the rest if anything failed.
        if !history {
            return Ok(());
        }
        for suffix in ["-wal", "-shm"] {
            let name = format!("{HISTORY_DB}{suffix}");
            let dest = data_dir.join(&name);
            if dest.is_file() {
                fs::create_dir_all(&previous)?;
                let old = previous.join(&name);
                fs::rename(&dest, &old)?;
                moved.push((dest, Some(old)));
            }
        }
        Ok(())
    });
    if let Err(error) = placed {
        let mut kept = false;
        for (dest, old) in moved.into_iter().rev() {
            let _ = fs::remove_file(&dest);
            if let Some(old) = old
                && let Err(rename) = fs::rename(&old, &dest)
            {
                tracing::error!(%rename, path = %dest.display(), "archive: a replaced file could not be put back");
                kept = true;
            }
        }
        // The unpacking folder is removed next; what could not go back moves out of it first.
        if kept {
            // A set kept by an earlier failed import stays: this one takes the next free name.
            let keep = (1..)
                .map(|n| if n == 1 { data_dir.join(KEPT) } else { data_dir.join(format!("{KEPT}-{n}")) })
                .find(|p| !p.exists())
                .expect("a free name");
            if let Err(rename) = fs::rename(&previous, keep) {
                tracing::error!(%rename, "archive: the replaced files could not be kept");
            }
        }
        return Err(error.into());
    }
    // The set-aside files (the old log included) go with the unpacking folder.
    written.extend(moved.into_iter().filter(|(dest, _)| dest.is_file()).map(|(dest, _)| dest));
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
    if name.eq_ignore_ascii_case(SETTINGS_BEFORE_IMPORT) {
        // The importer writes this file itself. An archived copy must not replace the backup it just made.
        return false;
    }
    if let Some(file) = name.strip_prefix("uploaders/") {
        return !file.contains('/') && is_sxcu(file);
    }
    name.strip_prefix("backups/").is_some_and(|rest| !rest.is_empty())
}

/// A relative name with `/` separators only, no `.` or `..` part, and no backslash, colon or NUL.
/// Joined to a folder, such a name stays inside it on every platform. No part ends in a dot or a space
/// either: Windows drops those, so `settings.json.` would be written as `settings.json`.
fn is_safe(name: &str) -> bool {
    !name.starts_with('/')
        && !name.contains(['\\', ':', '\0'])
        && name.split('/').all(|part| part != ".." && part != "." && !part.ends_with(['.', ' ']) && !is_device_name(part))
}

/// A Windows device name (`CON`, `NUL`, `COM1`…), with or without an extension: written to, it is the
/// device and not a file.
fn is_device_name(part: &str) -> bool {
    let stem = part.split('.').next().unwrap_or_default().trim_end().to_ascii_uppercase();
    // COM and LPT take a digit or a superscript one, two or three.
    let port = stem.strip_prefix("COM").or_else(|| stem.strip_prefix("LPT"));
    matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || port.is_some_and(|n| matches!(n.chars().collect::<Vec<_>>()[..], [c] if c.is_ascii_digit() || matches!(c, '¹' | '²' | '³')))
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

#[cfg(test)]
mod name_tests {
    use super::{is_device_name, is_safe};

    #[test]
    fn names_windows_would_change_or_open_as_devices_are_refused() {
        assert!(!is_safe("backups/settings-before-import.json."), "Windows drops the trailing dot");
        assert!(!is_safe("uploaders/a.sxcu "), "and a trailing space");
        assert!(!is_safe("backups/COM¹.txt"));
        assert!(is_device_name("lpt³"));
        assert!(is_device_name("COM1.json"));
        assert!(!is_device_name("COM10"));
        assert!(!is_device_name("COMPUTER.txt"));
        assert!(is_safe("backups/settings-2026.json"));
    }
}

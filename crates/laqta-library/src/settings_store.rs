//! Versioned JSON settings (ADR 006): migrations on load, atomic saves, rotating and weekly backups.

use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde::ser::Error as _;
use serde_json::Value;

use crate::LibraryError;

/// Upgrades settings from version `i + 1` to version `i + 2`. Version 1 is the first.
pub type Migration = fn(Value) -> Value;

/// Daily backups kept in `backups/`. Weekly ones are kept without a limit.
const DAILY_BACKUPS_KEPT: usize = 5;

#[derive(Clone, Debug)]
pub struct SettingsStore {
    dir: PathBuf,
}

impl SettingsStore {
    pub fn new(dir: PathBuf) -> Self {
        SettingsStore { dir }
    }

    /// `<dir>/settings.json`.
    pub fn path(&self) -> PathBuf {
        self.dir.join("settings.json")
    }

    /// Loads the settings and migrates them to `current`.
    ///
    /// A missing file gives `T::default()`. A file that cannot be parsed or migrated into `T` is
    /// corrupt: the newest backup that loads is returned instead. If no backup loads, the bad file
    /// is renamed to `settings.json.corrupt-<n>` and `T::default()` is returned. A file from a newer
    /// version is an error, and nothing is renamed.
    pub fn load<T: DeserializeOwned + Default>(&self, current: u32, migrations: &[Migration]) -> Result<T, LibraryError> {
        let path = self.path();
        let bytes = match fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(T::default()),
            Err(error) => return Err(error.into()),
        };
        if let Some(value) = decode::<T>(&bytes, current, migrations)? {
            return Ok(value);
        }

        tracing::warn!(path = %path.display(), "settings file is corrupt; trying backups");
        for backup in backup_files(&self.dir.join("backups")).into_iter().rev() {
            let loaded = fs::read(&backup).map_err(LibraryError::from).and_then(|bytes| decode::<T>(&bytes, current, migrations));
            match loaded {
                Ok(Some(value)) => {
                    tracing::warn!(backup = %backup.display(), "restored settings from a backup");
                    return Ok(value);
                }
                Ok(None) => tracing::warn!(backup = %backup.display(), "backup is also corrupt"),
                Err(error) => {
                    tracing::warn!(backup = %backup.display(), %error, "backup cannot be used")
                }
            }
        }

        let aside = self.free_corrupt_path();
        match fs::rename(&path, &aside) {
            Ok(()) => {
                tracing::warn!(moved_to = %aside.display(), "no backup loads; starting from defaults")
            }
            Err(error) => {
                tracing::warn!(%error, "could not move the corrupt settings file aside; starting from defaults")
            }
        }
        Ok(T::default())
    }

    /// Writes `value` with `"version": current` at the top level. The old file, if any, is copied to
    /// `backups/settings-<yyyymmdd-HHMMSS>.json` first, and the first save of each ISO week also
    /// copies it to `backups/weekly/settings-<yyyy>-W<ww>.json`. The write goes through
    /// `settings.json.part` and a rename, so an interrupted save leaves the old file in place.
    pub fn save<T: Serialize>(&self, value: &T, current: u32, now: jiff::civil::DateTime) -> Result<(), LibraryError> {
        let mut json = serde_json::to_value(value)?;
        let Some(object) = json.as_object_mut() else {
            return Err(serde_json::Error::custom("settings must serialize to a JSON object").into());
        };
        object.insert("version".to_owned(), Value::from(current));
        let bytes = serde_json::to_vec_pretty(&json)?;

        fs::create_dir_all(&self.dir)?;
        let path = self.path();
        if path.exists() {
            self.backup(&path, now)?;
        }
        let part = self.dir.join("settings.json.part");
        let mut file = File::create(&part)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&part, &path)?;
        Ok(())
    }

    fn backup(&self, current_file: &Path, now: jiff::civil::DateTime) -> Result<(), LibraryError> {
        let backups = self.dir.join("backups");
        fs::create_dir_all(&backups)?;
        let stamp = format!("{:04}{:02}{:02}-{:02}{:02}{:02}", now.year(), now.month(), now.day(), now.hour(), now.minute(), now.second());
        fs::copy(current_file, backups.join(format!("settings-{stamp}.json")))?;
        prune_daily_backups(&backups);

        let weekly = backups.join("weekly");
        let iso = now.date().iso_week_date();
        let week_file = weekly.join(format!("settings-{:04}-W{:02}.json", iso.year(), iso.week()));
        if !week_file.exists() {
            fs::create_dir_all(&weekly)?;
            fs::copy(current_file, week_file)?;
        }
        Ok(())
    }

    /// The lowest `settings.json.corrupt-<n>` that is not taken.
    fn free_corrupt_path(&self) -> PathBuf {
        let mut n = 1;
        loop {
            let candidate = self.dir.join(format!("settings.json.corrupt-{n}"));
            if !candidate.exists() {
                return candidate;
            }
            n += 1;
        }
    }
}

/// Parses `bytes` and migrates them to `current`. `Ok(None)` means the bytes are corrupt: bad JSON,
/// a bad version field, or a shape `T` does not accept. `Err` means a version newer than this build,
/// or a missing migration.
fn decode<T: DeserializeOwned>(bytes: &[u8], current: u32, migrations: &[Migration]) -> Result<Option<T>, LibraryError> {
    let Ok(mut value) = serde_json::from_slice::<Value>(bytes) else {
        return Ok(None);
    };
    if !value.is_object() {
        return Ok(None);
    }
    let found = match value.get("version") {
        None => 1,
        Some(version) => match version.as_u64().and_then(|v| u32::try_from(v).ok()) {
            Some(version) if version >= 1 => version,
            _ => return Ok(None),
        },
    };
    if found > current {
        return Err(LibraryError::TooNew { found, supported: current });
    }
    for version in found..current {
        let Some(migrate) = migrations.get((version - 1) as usize) else {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, format!("no migration from settings version {version}")).into());
        };
        value = migrate(value);
    }
    if let Some(object) = value.as_object_mut() {
        object.insert("version".to_owned(), Value::from(current));
    }
    match serde_json::from_value(value) {
        Ok(settings) => Ok(Some(settings)),
        Err(error) => {
            tracing::warn!(%error, "settings do not match the current shape");
            Ok(None)
        }
    }
}

/// `settings-*.json` files directly in `dir`, oldest first. The `weekly` folder is skipped. A folder
/// that cannot be read counts as empty.
fn backup_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> =
        entries.filter_map(Result::ok).map(|entry| entry.path()).filter(|path| path.is_file() && is_backup_name(path)).collect();
    files.sort();
    files
}

fn is_backup_name(path: &Path) -> bool {
    path.file_name().and_then(|name| name.to_str()).is_some_and(|name| name.starts_with("settings-") && name.ends_with(".json"))
}

fn prune_daily_backups(backups: &Path) {
    let files = backup_files(backups);
    let excess = files.len().saturating_sub(DAILY_BACKUPS_KEPT);
    for old in &files[..excess] {
        if let Err(error) = fs::remove_file(old) {
            tracing::warn!(path = %old.display(), %error, "could not remove an old settings backup");
        }
    }
}

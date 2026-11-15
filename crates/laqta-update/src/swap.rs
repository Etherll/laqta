//! Putting a downloaded copy in place of the running one, for the copies Velopack does not manage:
//! a Linux AppImage (one file) and a macOS app bundle (a folder). Both are renames inside one
//! folder, so the copy that is running keeps its open files and the next start runs the new one.

use std::io;
use std::path::{Path, PathBuf};

/// Replaces the file `target` with `new`, which must be in the same folder (a rename does not cross
/// file systems), and makes it executable.
pub(crate) fn replace_file(new: &Path, target: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(new, std::fs::Permissions::from_mode(0o755))?;
    }
    std::fs::rename(new, target)
}

/// Replaces the folder `target` with `new` (both in one folder) and returns where the old one went,
/// for the caller to remove once nothing runs from it. A failure puts the old one back. The name is
/// fixed, so an old copy a cut-short restart left behind goes with the next update.
pub(crate) fn replace_bundle(new: &Path, target: &Path) -> io::Result<PathBuf> {
    let name = target.file_name().ok_or_else(|| io::Error::other("the app has no folder name"))?;
    let mut old = target.with_file_name(format!(".{}-old", name.to_string_lossy()));
    if old.exists() && std::fs::remove_dir_all(&old).is_err() {
        // A leftover this user can't remove (another account's files) must not block every update.
        old = target.with_file_name(format!(".{}-old-{}", name.to_string_lossy(), std::process::id()));
        if old.exists() {
            std::fs::remove_dir_all(&old)?;
        }
    }
    std::fs::rename(target, &old)?;
    if let Err(error) = std::fs::rename(new, target) {
        if let Err(restore) = std::fs::rename(&old, target) {
            return Err(io::Error::other(format!("{error}; putting the old app back also failed ({restore}), it is at {}", old.display())));
        }
        return Err(error);
    }
    Ok(old)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_file_is_replaced_and_made_executable() {
        let dir = tempfile::tempdir().unwrap();
        let (target, new) = (dir.path().join("Laqta.AppImage"), dir.path().join(".update"));
        std::fs::write(&target, "old").unwrap();
        std::fs::write(&new, "new").unwrap();
        replace_file(&new, &target).unwrap();
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!new.exists());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(&target).unwrap().permissions().mode() & 0o777, 0o755);
        }
    }

    #[test]
    fn a_bundle_is_replaced_and_the_old_one_set_aside() {
        let dir = tempfile::tempdir().unwrap();
        let (target, new) = (dir.path().join("Laqta.app"), dir.path().join("staging").join("Laqta.app"));
        std::fs::create_dir_all(target.join("Contents")).unwrap();
        std::fs::write(target.join("Contents/version"), "old").unwrap();
        std::fs::create_dir_all(new.join("Contents")).unwrap();
        std::fs::write(new.join("Contents/version"), "new").unwrap();
        let old = replace_bundle(&new, &target).unwrap();
        assert_eq!(std::fs::read_to_string(target.join("Contents/version")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old.join("Contents/version")).unwrap(), "old");
    }

    #[cfg(unix)]
    #[test]
    fn a_leftover_old_bundle_that_cannot_be_removed_does_not_block_the_swap() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (target, new) = (dir.path().join("Laqta.app"), dir.path().join("staging").join("Laqta.app"));
        std::fs::create_dir_all(&target).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        let locked = dir.path().join(".Laqta.app-old").join("Contents");
        std::fs::create_dir_all(&locked).unwrap();
        std::fs::write(locked.join("file"), "").unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let old = replace_bundle(&new, &target);
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        let old = old.unwrap();
        assert!(target.is_dir());
        assert!(old.is_dir());
    }

    #[test]
    fn a_bundle_that_cannot_move_in_leaves_the_old_one_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("Laqta.app");
        std::fs::create_dir_all(&target).unwrap();
        std::fs::write(target.join("version"), "old").unwrap();
        assert!(replace_bundle(&dir.path().join("missing.app"), &target).is_err());
        assert_eq!(std::fs::read_to_string(target.join("version")).unwrap(), "old");
    }
}

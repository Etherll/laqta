//! Which copy of Laqta this is, and putting a release in its place. The caller downloads into a
//! [`Staging`] folder with [`crate::download_asset`], so every file's SHA-256 comes from the
//! release's `SHA256SUMS`.
//!
//! Nothing here starts a process or touches the network: the caller runs the downloads and the
//! Velopack, unpack and restart commands this module builds, so the decisions stay testable without a
//! desktop and the file work stays off the HTTP runtime.

use std::path::{Path, PathBuf};

use crate::{UpdateError, swap};

/// The variables the AppImage runtime sets for the process it starts. They describe that copy's
/// mount, so a restarted copy must not inherit them.
pub const APPIMAGE_RUNTIME_VARS: [&str; 4] = ["APPIMAGE", "APPDIR", "ARGV0", "OWD"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Windows,
    MacOs,
    /// Linux on x86_64, the only Linux the release builds an AppImage for.
    LinuxX64,
    Other,
}

impl Platform {
    pub fn current() -> Platform {
        if cfg!(windows) {
            Platform::Windows
        } else if cfg!(target_os = "macos") {
            Platform::MacOs
        } else if cfg!(all(target_os = "linux", target_arch = "x86_64")) {
            Platform::LinuxX64
        } else {
            Platform::Other
        }
    }
}

/// How this copy updates itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Installer {
    /// The Windows setup program's copy, applied by Velopack's `Update.exe`.
    Velopack { update_exe: PathBuf },
    /// A Linux AppImage, replaced by the release's AppImage.
    AppImage { image: PathBuf },
    /// A macOS app bundle (`…/Laqta.app`), replaced by the release's zipped bundle.
    MacApp { bundle: PathBuf },
}

impl Installer {
    /// The installer for the program at `exe`, or `None` for a copy something else updates: a portable
    /// `.exe`, the `.deb`, the archive or Nix, or a Mac app on a read-only volume. An AppImage or a
    /// bundle in a folder this user can't write to (`/opt`, `/Applications` for a standard user) is
    /// `None` too, so the offer is never one that fails. `appimage` and `appdir` are the AppImage
    /// runtime's `APPIMAGE` and `APPDIR`.
    pub fn detect(platform: Platform, exe: &Path, appimage: Option<&Path>, appdir: Option<&Path>) -> Option<Installer> {
        match platform {
            // Velopack's layout puts Update.exe one folder above the running `current\laqta.exe`.
            Platform::Windows => {
                let update_exe = exe.parent()?.parent()?.join("Update.exe");
                update_exe.is_file().then_some(Installer::Velopack { update_exe })
            }
            Platform::MacOs => {
                mac_bundle(exe).filter(|bundle| writable_folder_of(bundle)).map(|bundle| Installer::MacApp { bundle: bundle.to_path_buf() })
            }
            // A program started from another AppImage (a terminal, an editor) inherits that one's
            // variables, so the image counts only when this program runs from its mount.
            Platform::LinuxX64 => {
                if !runs_from_appimage(exe, appdir) {
                    return None;
                }
                let image = appimage?;
                // The swap replaces the file itself, not a link to it.
                let image = std::fs::canonicalize(image).ok().filter(|image| image.is_file())?;
                writable_folder_of(&image).then_some(Installer::AppImage { image })
            }
            Platform::Other => None,
        }
    }
}

/// Whether the program at `exe` runs from an AppImage mount (`APPDIR`). A program started from another
/// AppImage (a terminal, an editor) inherits that one's variables, but not its mount.
pub fn runs_from_appimage(exe: &Path, appdir: Option<&Path>) -> bool {
    appdir.is_some_and(|appdir| exe.starts_with(appdir))
}

/// The bundle of `…/Laqta.app/Contents/MacOS/laqta`, unless it is on a read-only volume: a disk
/// image (`/Volumes`) or the copy Gatekeeper translocates a quarantined app to.
fn mac_bundle(exe: &Path) -> Option<&Path> {
    let bundle = exe.ancestors().nth(3).filter(|p| p.extension().is_some_and(|e| e == "app"))?;
    let read_only = bundle.starts_with("/Volumes") || bundle.components().any(|c| c.as_os_str() == "AppTranslocation");
    (!read_only).then_some(bundle)
}

/// Whether this user can create the staging folder beside `target` (the swap needs the same right).
fn writable_folder_of(target: &Path) -> bool {
    let Some(folder) = target.parent() else { return false };
    static PROBES: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = PROBES.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = folder.join(format!(".laqta-write-test-{}-{n}", std::process::id()));
    std::fs::create_dir(&probe).and_then(|()| std::fs::remove_dir(&probe)).is_ok()
}

/// The folder an update is downloaded and unpacked in, beside the copy it replaces, so the swap is
/// a rename on one file system. It is per user: Laqta runs once per user, and `/Applications` is
/// shared by every account on the Mac. A run that was cut short is cleared by the same user's next
/// one; dropping it removes the folder.
#[derive(Debug)]
pub struct Staging {
    dir: PathBuf,
}

impl Staging {
    pub fn beside(target: &Path) -> Result<Staging, UpdateError> {
        let folder = target.parent().ok_or_else(|| UpdateError::Io(format!("{} has no folder", target.display())))?;
        let dir = folder.join(format!(".laqta-update-{}", user_tag()));
        if dir.exists() {
            std::fs::remove_dir_all(&dir).map_err(|e| io(&dir, &e))?;
        }
        std::fs::create_dir(&dir).map_err(|e| io(folder, &e))?;
        Ok(Staging { dir })
    }

    pub fn path(&self) -> &Path {
        &self.dir
    }
}

impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// The account name, reduced to characters safe in a file name.
fn user_tag() -> String {
    let name = ["USER", "LOGNAME", "USERNAME"].iter().find_map(|var| std::env::var(var).ok()).unwrap_or_default();
    let tag: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')).collect();
    if tag.is_empty() { "user".to_owned() } else { tag }
}

fn io(path: &Path, error: &std::io::Error) -> UpdateError {
    UpdateError::Io(format!("{}: {error}", path.display()))
}

/// Moves the downloaded AppImage `new` (in a [`Staging`] beside it) over `image`. The running copy
/// keeps its mount, and the next start runs the new one.
pub fn replace_appimage(new: &Path, image: &Path) -> Result<(), UpdateError> {
    swap::replace_file(new, image).map_err(|e| io(image, &e))
}

/// `ditto -x -k <zip> <into>`, which unpacks the bundle with its signature intact.
pub fn unpack_command(zip: &Path, into: &Path) -> (PathBuf, Vec<String>) {
    let args = ["-x".to_owned(), "-k".to_owned(), zip.to_string_lossy().into_owned(), into.to_string_lossy().into_owned()];
    (PathBuf::from("/usr/bin/ditto"), args.into())
}

/// Puts the `Laqta.app` unpacked in `staging` in place of `bundle` and returns where the old one
/// went, for the restart to remove.
pub fn replace_mac_app(staging: &Staging, bundle: &Path) -> Result<PathBuf, UpdateError> {
    let new = staging.path().join("Laqta.app");
    if !new.join("Contents/MacOS/laqta").is_file() {
        return Err(UpdateError::Io("the downloaded update holds no Laqta.app".to_owned()));
    }
    swap::replace_bundle(&new, bundle).map_err(|e| io(bundle, &e))
}

/// The shell command that waits for process `pid` to exit, starts `target` (through `open` for a
/// bundle) and then removes `remove`. A copy started sooner would find the old one still running and
/// hand over to it. The caller clears [`APPIMAGE_RUNTIME_VARS`] from its environment.
pub fn restart_command(target: &Path, pid: u32, remove: Option<&Path>, bundle: bool) -> (PathBuf, Vec<String>) {
    let start = if bundle { "/usr/bin/open \"$0\";" } else { "\"$0\" &" };
    let script = format!("while kill -0 \"$1\" 2>/dev/null; do sleep 0.2; done; {start} if [ -n \"$2\" ]; then rm -rf \"$2\"; fi");
    let args = vec![
        "-c".to_owned(),
        script,
        target.to_string_lossy().into_owned(),
        pid.to_string(),
        remove.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
    ];
    (PathBuf::from("/bin/sh"), args)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_windows_copy_updates_through_velopack_only_when_installed() {
        let dir = tempfile::tempdir().unwrap();
        let exe = dir.path().join("current").join("laqta.exe");
        assert_eq!(Installer::detect(Platform::Windows, &exe, None, None), None, "a portable copy");
        std::fs::write(dir.path().join("Update.exe"), "").unwrap();
        let update_exe = dir.path().join("Update.exe");
        assert_eq!(Installer::detect(Platform::Windows, &exe, None, None), Some(Installer::Velopack { update_exe }));
    }

    #[test]
    fn an_appimage_counts_only_when_this_program_runs_from_its_mount() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("Laqta.AppImage");
        std::fs::write(&image, "").unwrap();
        let mount = Path::new("/tmp/.mount_LaqtaX");
        let exe = mount.join("usr/bin/laqta");
        let found = Installer::detect(Platform::LinuxX64, &exe, Some(&image), Some(mount));
        let real = std::fs::canonicalize(&image).unwrap();
        assert_eq!(found, Some(Installer::AppImage { image: real.clone() }));
        #[cfg(unix)]
        {
            // A link to the image swaps the image it points at.
            let link = dir.path().join("laqta");
            std::os::unix::fs::symlink(&image, &link).unwrap();
            let found = Installer::detect(Platform::LinuxX64, &exe, Some(&link), Some(mount));
            assert_eq!(found, Some(Installer::AppImage { image: real }));
        }
        // Started from another AppImage's terminal: the variables are that image's.
        let deb = Path::new("/usr/bin/laqta");
        assert_eq!(Installer::detect(Platform::LinuxX64, deb, Some(&image), Some(mount)), None);
        assert_eq!(Installer::detect(Platform::LinuxX64, deb, None, None), None, "the .deb, the archive or Nix");
        assert_eq!(Installer::detect(Platform::Other, &exe, Some(&image), Some(mount)), None);
    }

    #[test]
    fn a_mac_bundle_counts_unless_it_is_on_a_read_only_volume() {
        assert_eq!(mac_bundle(Path::new("/Applications/Laqta.app/Contents/MacOS/laqta")), Some(Path::new("/Applications/Laqta.app")));
        assert_eq!(mac_bundle(Path::new("/Volumes/Laqta/Laqta.app/Contents/MacOS/laqta")), None);
        assert_eq!(mac_bundle(Path::new("/private/var/folders/x/T/AppTranslocation/ABC/d/Laqta.app/Contents/MacOS/laqta")), None);
        assert_eq!(mac_bundle(Path::new("/usr/local/bin/laqta")), None);
    }

    #[test]
    fn a_mac_bundle_updates_only_where_this_user_can_write() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Laqta.app");
        let exe = bundle.join("Contents/MacOS/laqta");
        assert_eq!(Installer::detect(Platform::MacOs, &exe, None, None), Some(Installer::MacApp { bundle }));
        assert!(std::fs::read_dir(dir.path()).unwrap().next().is_none(), "the write probe leaves nothing behind");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o555)).unwrap();
            // root writes anywhere, so the refusal shows only for a regular user.
            if std::fs::create_dir(dir.path().join("probe")).is_err() {
                assert_eq!(Installer::detect(Platform::MacOs, &exe, None, None), None);
            }
            std::fs::set_permissions(dir.path(), std::fs::Permissions::from_mode(0o755)).unwrap();
        }
    }

    #[test]
    fn a_staged_bundle_without_the_app_is_refused_and_the_staging_goes() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Laqta.app");
        std::fs::create_dir(&bundle).unwrap();
        let staging = Staging::beside(&bundle).unwrap();
        let path = staging.path().to_path_buf();
        assert!(matches!(replace_mac_app(&staging, &bundle), Err(UpdateError::Io(_))));
        assert!(bundle.is_dir(), "the installed app stays");
        drop(staging);
        assert!(!path.exists());
    }

    #[test]
    fn a_staged_bundle_replaces_the_installed_one() {
        let dir = tempfile::tempdir().unwrap();
        let bundle = dir.path().join("Laqta.app");
        std::fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        std::fs::write(bundle.join("Contents/MacOS/laqta"), "old").unwrap();
        let staging = Staging::beside(&bundle).unwrap();
        std::fs::create_dir_all(staging.path().join("Laqta.app/Contents/MacOS")).unwrap();
        std::fs::write(staging.path().join("Laqta.app/Contents/MacOS/laqta"), "new").unwrap();
        let zip = staging.path().join("update.zip");
        let (_, args) = unpack_command(&zip, staging.path());
        assert_eq!(args[..3], ["-x".to_owned(), "-k".to_owned(), zip.to_string_lossy().into_owned()]);
        let old = replace_mac_app(&staging, &bundle).unwrap();
        assert_eq!(std::fs::read_to_string(bundle.join("Contents/MacOS/laqta")).unwrap(), "new");
        assert_eq!(std::fs::read_to_string(old.join("Contents/MacOS/laqta")).unwrap(), "old");
    }

    #[test]
    fn an_appimage_replaces_the_image() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("Laqta.AppImage");
        std::fs::write(&image, "old").unwrap();
        let staging = Staging::beside(&image).unwrap();
        let new = staging.path().join("new.AppImage");
        std::fs::write(&new, "new").unwrap();
        replace_appimage(&new, &image).unwrap();
        assert_eq!(std::fs::read_to_string(&image).unwrap(), "new");
    }

    #[test]
    fn staging_is_per_user_and_clears_what_an_interrupted_run_left() {
        let dir = tempfile::tempdir().unwrap();
        let image = dir.path().join("Laqta.AppImage");
        let leftover = dir.path().join(format!(".laqta-update-{}", user_tag()));
        std::fs::create_dir_all(&leftover).unwrap();
        std::fs::write(leftover.join("partial"), "").unwrap();
        let other_user = dir.path().join(".laqta-update-someone-else");
        std::fs::create_dir(&other_user).unwrap();
        let staging = Staging::beside(&image).unwrap();
        assert_eq!(staging.path(), leftover);
        assert_eq!(std::fs::read_dir(staging.path()).unwrap().count(), 0);
        drop(staging);
        assert!(!leftover.exists());
        assert!(other_user.exists(), "another account's staging is left alone");
    }

    #[test]
    fn running_from_an_appimage_means_running_inside_its_mount() {
        let mount = Path::new("/tmp/.mount_LaqtaX");
        assert!(runs_from_appimage(&mount.join("usr/bin/laqta"), Some(mount)));
        assert!(!runs_from_appimage(Path::new("/usr/bin/laqta"), Some(mount)));
        assert!(!runs_from_appimage(Path::new("/usr/bin/laqta"), None));
    }

    #[cfg(unix)]
    #[test]
    fn the_restart_waits_for_the_old_copy_then_starts_the_new_one_and_removes_the_old_files() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (target, marker, old) = (dir.path().join("laqta"), dir.path().join("started"), dir.path().join("old"));
        std::fs::create_dir(&old).unwrap();
        std::fs::write(&target, format!("#!/bin/sh\ntouch '{}'\n", marker.display())).unwrap();
        std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o755)).unwrap();
        // Stands in for the old copy; it runs until the test ends it.
        let mut running = std::process::Command::new("/bin/sleep").arg("60").spawn().unwrap();
        let (program, args) = restart_command(&target, running.id(), Some(&old), false);
        let mut restart = std::process::Command::new(program).args(args).spawn().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
        assert!(!marker.exists(), "nothing starts while the old copy runs");
        assert!(old.exists());
        running.kill().unwrap();
        running.wait().unwrap();
        assert!(restart.wait().unwrap().success());
        for _ in 0..50 {
            if marker.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(marker.exists(), "the new copy started");
        assert!(!old.exists(), "the old files are gone");
    }
}

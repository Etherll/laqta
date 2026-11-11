//! Run at login: `laqta.desktop` in the XDG autostart directory. The directory is a field,
//! so tests use a temp dir; `default()` uses `$XDG_CONFIG_HOME/autostart`.

use std::env;
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use laqta_ports::{Autostart, PortError};

const FILE_NAME: &str = "laqta.desktop";

/// The XDG autostart entry for Laqta.
#[derive(Debug)]
pub struct LinuxAutostart {
    dir: PathBuf,
}

impl Default for LinuxAutostart {
    /// `$XDG_CONFIG_HOME/autostart`, or `~/.config/autostart`.
    fn default() -> Self {
        Self { dir: xdg_dir("XDG_CONFIG_HOME", ".config").join("autostart") }
    }
}

impl Autostart for LinuxAutostart {
    fn is_enabled(&self) -> Result<bool, PortError> {
        let path = self.dir.join(FILE_NAME);
        match fs::read_to_string(&path) {
            Ok(text) => Ok(!is_hidden(&text)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
            Err(e) => Err(io_error(&path, e)),
        }
    }

    fn set_enabled(&self, enabled: bool, executable: &Path) -> Result<(), PortError> {
        let path = self.dir.join(FILE_NAME);
        if enabled { write_file(&path, &desktop_entry(&launcher(executable)), 0o644) } else { remove_if_exists(&path) }
    }
}

/// `$var` when it holds an absolute path, else `~/<fallback>`, as the XDG base directory spec says.
/// With no home directory the result is relative, and the writers refuse it.
pub(crate) fn xdg_dir(var: &str, fallback: &str) -> PathBuf {
    env::var_os(var)
        .map(PathBuf::from)
        .filter(|dir| dir.is_absolute())
        .unwrap_or_else(|| env::home_dir().unwrap_or_default().join(fallback))
}

/// The path a desktop entry should start. Inside an AppImage, `executable` lives in a mount that
/// changes on every run, so the entry starts the AppImage file (`$APPIMAGE`) instead. Otherwise it
/// is the `PATH` entry that leads to `executable`, when one does.
pub(crate) fn launcher(executable: &Path) -> PathBuf {
    launcher_from(executable, env::var_os("APPIMAGE"), env::var_os("APPDIR"), env::var_os("PATH"))
}

fn launcher_from(executable: &Path, appimage: Option<OsString>, appdir: Option<OsString>, path_var: Option<OsString>) -> PathBuf {
    match (appimage.map(PathBuf::from), appdir.map(PathBuf::from)) {
        (Some(image), Some(dir)) if image.is_absolute() && executable.starts_with(&dir) => image,
        _ => path_link(executable, &path_var.unwrap_or_default()).unwrap_or_else(|| executable.to_path_buf()),
    }
}

/// The first absolute `PATH` entry whose file of the same name is `executable` itself, possibly
/// through symlinks. A package manager's link there (`/usr/bin/laqta`, a package profile's
/// `bin/laqta`) keeps working across updates, while the file it points to may move and be removed.
fn path_link(executable: &Path, path_var: &OsStr) -> Option<PathBuf> {
    let name = executable.file_name()?;
    let target = fs::canonicalize(executable).ok()?;
    env::split_paths(path_var)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(name))
        .find(|link| fs::canonicalize(link).is_ok_and(|resolved| resolved == target))
}

/// The `Exec` value that runs `executable` followed by `args`. The path is quoted with the reserved
/// characters escaped and `%` doubled; the desktop file's string escaping then applies to the whole.
pub(crate) fn exec_value(executable: &Path, args: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in executable.to_string_lossy().chars() {
        match ch {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            other => quoted.push(other),
        }
    }
    quoted.push('"');
    quoted.push_str(args);
    escape_string(&quoted)
}

/// Desktop-file string escaping for the characters that need it.
fn escape_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        match ch {
            '\\' => out.push_str(r"\\"),
            '\n' => out.push_str(r"\n"),
            '\t' => out.push_str(r"\t"),
            '\r' => out.push_str(r"\r"),
            other => out.push(other),
        }
    }
    out
}

/// The entry text. `X-GNOME-Autostart-enabled` is what GNOME's own switch reads.
fn desktop_entry(executable: &Path) -> String {
    format!("[Desktop Entry]\nType=Application\nName=Laqta\nExec={}\nX-GNOME-Autostart-enabled=true\n", exec_value(executable, " -silent"))
}

/// True when the entry says `Hidden=true`, which desktops write when the user switches it off.
fn is_hidden(text: &str) -> bool {
    text.lines().any(|line| line.trim() == "Hidden=true")
}

/// Writes `contents` to `path` with `mode`, creating its directory and replacing any file there.
pub(crate) fn write_file(path: &Path, contents: &str, mode: u32) -> Result<(), PortError> {
    require_absolute(path)?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    }
    fs::write(path, contents).map_err(|e| io_error(path, e))?;
    fs::set_permissions(path, fs::Permissions::from_mode(mode)).map_err(|e| io_error(path, e))
}

/// Removes `path`. A missing file is not an error.
pub(crate) fn remove_if_exists(path: &Path) -> Result<(), PortError> {
    require_absolute(path)?;
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(path, e)),
    }
}

fn require_absolute(path: &Path) -> Result<(), PortError> {
    if path.is_absolute() { Ok(()) } else { Err(PortError::Unsupported(format!("{}: no home directory is set", path.display()))) }
}

pub(crate) fn io_error(path: &Path, e: io::Error) -> PortError {
    if e.kind() == io::ErrorKind::PermissionDenied {
        PortError::PermissionDenied
    } else {
        PortError::Failed(format!("{}: {e}", path.display()))
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use laqta_ports::Autostart;

    use super::{LinuxAutostart, exec_value, is_hidden, launcher_from};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laqta-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn enable_and_disable_write_and_remove_the_entry_idempotently() {
        let dir = scratch("autostart-enable");
        let autostart = LinuxAutostart { dir: dir.join("autostart") };
        let exe = Path::new("/opt/Laqta Apps/laqta");
        assert!(!autostart.is_enabled().expect("read"));
        autostart.set_enabled(true, exe).expect("enable");
        autostart.set_enabled(true, exe).expect("enable again");
        assert!(autostart.is_enabled().expect("read"));
        let text = fs::read_to_string(dir.join("autostart").join("laqta.desktop")).expect("entry");
        assert!(text.contains("Type=Application\nName=Laqta\nExec=\"/opt/Laqta Apps/laqta\" -silent\n"));
        autostart.set_enabled(false, exe).expect("disable");
        autostart.set_enabled(false, exe).expect("disable again");
        assert!(!autostart.is_enabled().expect("read"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_entry_the_desktop_hid_reads_as_disabled() {
        assert!(is_hidden("[Desktop Entry]\nHidden=true\n"));
        assert!(!is_hidden("[Desktop Entry]\nHidden=false\n"));
    }

    #[test]
    fn exec_quotes_reserved_characters_and_doubles_percent() {
        assert_eq!(exec_value(Path::new("/a b/laqta"), " %F"), "\"/a b/laqta\" %F");
        // Inside quotes `"`, `` ` ``, `$` and `\` are escaped, `%` is doubled, and the file's own
        // string escaping doubles every backslash again.
        assert_eq!(exec_value(Path::new(r#"/x/"$`\%"#), ""), r#""/x/\\"\\$\\`\\\\%%""#);
    }

    #[test]
    fn an_appimage_starts_the_image_file_not_its_mount() {
        let exe = Path::new("/tmp/.mount_LaqtaX1/usr/bin/laqta");
        let image = || Some("/home/me/Apps/Laqta.AppImage".into());
        let dir = || Some("/tmp/.mount_LaqtaX1".into());
        assert_eq!(launcher_from(exe, image(), dir(), None), Path::new("/home/me/Apps/Laqta.AppImage"));
        // Outside an AppImage, or with a stray variable, the executable itself is started.
        assert_eq!(launcher_from(exe, None, None, None), exe);
        assert_eq!(launcher_from(Path::new("/usr/bin/laqta"), image(), dir(), None), Path::new("/usr/bin/laqta"));
    }

    #[test]
    fn a_path_link_to_the_executable_is_started_instead_of_its_target() {
        // A package manager that installs the program elsewhere and links it into a `PATH` folder.
        let root = scratch("autostart-path-link");
        let exe = root.join("store").join("laqta-0.1.0").join("bin").join("laqta");
        let profile = root.join("profile").join("bin");
        let other = root.join("other").join("bin");
        fs::create_dir_all(exe.parent().expect("parent")).expect("store dir");
        fs::create_dir_all(&profile).expect("profile dir");
        fs::create_dir_all(&other).expect("other dir");
        fs::write(&exe, b"").expect("executable");
        std::os::unix::fs::symlink(&exe, profile.join("laqta")).expect("profile link");
        // An unrelated program of the same name earlier on `PATH` is passed over.
        fs::write(other.join("laqta"), b"").expect("other program");
        let path_var = std::env::join_paths([other.as_path(), Path::new("relative/bin"), profile.as_path()]).expect("PATH");
        assert_eq!(launcher_from(&exe, None, None, Some(path_var)), profile.join("laqta"));
        // Not on `PATH` (run from a build folder): the executable itself.
        assert_eq!(launcher_from(&exe, None, None, Some(other.into_os_string())), exe);
        let _ = fs::remove_dir_all(&root);
    }
}

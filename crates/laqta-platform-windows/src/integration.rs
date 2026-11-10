//! Shell integration on Windows: the "Upload with Laqta" verbs under `HKCU\Software\Classes`,
//! the Send To script and the browser native-messaging host registrations. The text and file parts
//! build on every target and are tested on Linux with temp dirs; the registry calls are Windows-only.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(windows)]
use std::env;

use laqta_ports::PortError;
#[cfg(windows)]
use laqta_ports::ShellIntegration;
#[cfg(windows)]
use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
#[cfg(windows)]
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, KEY_WRITE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey, RegCreateKeyExW, RegDeleteTreeW,
    RegDeleteValueW, RegOpenKeyExW, RegQueryValueExW, RegSetValueExW,
};
#[cfg(windows)]
use windows::core::PCWSTR;

const HOST_NAME: &str = "com.laqta.nmh";
const CHROME_ORIGIN: &str = "chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/";
const FIREFOX_ID: &str = "ShareX@ShareX";
const SEND_TO_FILE: &str = "Laqta.cmd";
/// Chrome and Firefox read different manifests, so each browser gets its own file.
const CHROME_MANIFEST: &str = "com.laqta.nmh.json";
const FIREFOX_MANIFEST: &str = "com.laqta.nmh.firefox.json";
#[cfg(windows)]
const VERB: &str = "Upload with Laqta";

/// The Windows shell and browser registrations, below the roots they were built with. `software`
/// is the `HKCU` key above `Classes` and the browser host keys; the two directories hold the Send To
/// script and the host manifests.
#[cfg(windows)]
#[derive(Debug)]
pub struct WindowsShellIntegration {
    software: String,
    /// An error when its variable is unset. The methods that write to it report that before they write anything.
    send_to_dir: Result<PathBuf, PortError>,
    nmh_dir: Result<PathBuf, PortError>,
}

#[cfg(windows)]
impl Default for WindowsShellIntegration {
    fn default() -> Self {
        Self {
            software: "Software".to_owned(),
            send_to_dir: env_dir("APPDATA", r"Microsoft\Windows\SendTo"),
            nmh_dir: env_dir("LOCALAPPDATA", r"Laqta\nmh"),
        }
    }
}

/// `rest` under the directory `var` names. An unset or empty variable is an error that names it.
#[cfg(windows)]
fn env_dir(var: &str, rest: &str) -> Result<PathBuf, PortError> {
    let base = env::var_os(var).filter(|base| !base.is_empty()).ok_or_else(|| PortError::Failed(format!("{var} is not set")))?;
    Ok(PathBuf::from(base).join(rest))
}

/// The verb key for `target`: `*` for files, `Directory` for folders.
/// ShareX's file types that open in Laqta (imported): (extension, ProgID, description).
pub(crate) const FILE_TYPES: [(&str, &str, &str); 3] =
    [(".sxcu", "Laqta.sxcu", "Custom uploader"), (".sxie", "Laqta.sxie", "Image effect"), (".sxb", "Laqta.sxb", "ShareX backup")];

/// `HKCU\Software\Classes\<name>`.
fn class_key(software: &str, name: &str) -> String {
    format!(r"{software}\Classes\{name}")
}

fn verb_key(software: &str, target: &str) -> String {
    format!(r"{software}\Classes\{target}\shell\Laqta")
}

/// The native-messaging registration key for the browser whose vendor key is `vendor`.
fn host_key(software: &str, vendor: &str) -> String {
    format!(r"{software}\{vendor}\NativeMessagingHosts\{HOST_NAME}")
}

/// The Send To script: starts Laqta with the selected paths. cmd wants CRLF line endings. cmd expands `%name%`
/// in a batch file, so each `%` in the executable path is doubled. `%*` stays as it is.
pub(crate) fn send_to_script(executable: &Path) -> String {
    let exe = executable.display().to_string().replace('%', "%%");
    format!("@start \"\" \"{exe}\" %*\r\n")
}

/// Writes the Send To script into `dir`.
fn write_send_to(dir: &Path, executable: &Path) -> Result<(), PortError> {
    write_text(&dir.join(SEND_TO_FILE), &send_to_script(executable))
}

/// Removes the Send To script from `dir`. A missing script is not an error.
fn remove_send_to(dir: &Path) -> Result<(), PortError> {
    let path = dir.join(SEND_TO_FILE);
    match fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_error(&path, e)),
    }
}

/// Writes the Chrome and Firefox manifests into `dir`, returning their paths.
fn write_host_manifests(dir: &Path, nmh: &Path) -> Result<(PathBuf, PathBuf), PortError> {
    let chrome = dir.join(CHROME_MANIFEST);
    let firefox = dir.join(FIREFOX_MANIFEST);
    write_text(&chrome, &chrome_manifest(nmh))?;
    write_text(&firefox, &firefox_manifest(nmh))?;
    Ok((chrome, firefox))
}

fn chrome_manifest(nmh: &Path) -> String {
    manifest(nmh, &format!("\"allowed_origins\": [{}]", json_string(CHROME_ORIGIN)))
}

fn firefox_manifest(nmh: &Path) -> String {
    manifest(nmh, &format!("\"allowed_extensions\": [{}]", json_string(FIREFOX_ID)))
}

fn manifest(nmh: &Path, allowed: &str) -> String {
    format!(
        "{{\n  \"name\": {},\n  \"description\": \"Laqta\",\n  \"path\": {},\n  \"type\": \"stdio\",\n  {allowed}\n}}\n",
        json_string(HOST_NAME),
        json_string(&nmh.display().to_string()),
    )
}

/// `text` as a JSON string literal, with quotes, backslashes and control characters escaped. Windows
/// paths need the backslashes doubled, or the browser rejects the manifest.
fn json_string(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            c if u32::from(c) < 0x20 => out.push_str(&format!("\\u{:04x}", u32::from(c))),
            other => out.push(other),
        }
    }
    out.push('"');
    out
}

/// Writes `contents` to `path`, creating its directory and replacing any file there.
fn write_text(path: &Path, contents: &str) -> Result<(), PortError> {
    if !path.is_absolute() {
        return Err(PortError::Unsupported(format!("{}: no user profile directory is set", path.display())));
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).map_err(|e| io_error(dir, e))?;
    }
    fs::write(path, contents).map_err(|e| io_error(path, e))
}

fn io_error(path: &Path, e: io::Error) -> PortError {
    if e.kind() == io::ErrorKind::PermissionDenied {
        PortError::PermissionDenied
    } else {
        PortError::Failed(format!("{}: {e}", path.display()))
    }
}

/// An open key under `HKCU`, closed on drop.
#[cfg(windows)]
pub(crate) struct OpenKey(HKEY);

#[cfg(windows)]
impl OpenKey {
    /// Opens `subkey` under `HKCU`, creating it and any missing parents.
    pub(crate) fn create(subkey: &str) -> Result<Self, PortError> {
        let path = wide(subkey);
        let mut key = HKEY(std::ptr::null_mut());
        // SAFETY: `path` is NUL-terminated and alive for the call; `key` receives the new handle,
        // which `Drop` closes.
        let status = unsafe {
            RegCreateKeyExW(
                HKEY_CURRENT_USER,
                PCWSTR(path.as_ptr()),
                None,
                PCWSTR::null(),
                REG_OPTION_NON_VOLATILE,
                KEY_READ | KEY_WRITE,
                None,
                &mut key,
                None,
            )
        };
        status.ok().map_err(|e| registry("create", e))?;
        Ok(Self(key))
    }

    /// Opens `subkey` under `HKCU`, or returns None when it does not exist.
    pub(crate) fn open(subkey: &str) -> Result<Option<Self>, PortError> {
        let path = wide(subkey);
        let mut key = HKEY(std::ptr::null_mut());
        // SAFETY: as in `create`.
        let status = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr()), None, KEY_READ | KEY_WRITE, &mut key) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        status.ok().map_err(|e| registry("open", e))?;
        Ok(Some(Self(key)))
    }

    /// Deletes `subkey` and everything under it. A missing key is not an error.
    pub(crate) fn delete_tree(subkey: &str) -> Result<(), PortError> {
        let path = wide(subkey);
        // SAFETY: `path` is NUL-terminated and alive for the call.
        let status = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(path.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        status.ok().map_err(|e| registry("delete", e))
    }

    /// Whether the value `name` exists.
    pub(crate) fn has_value(&self, name: &str) -> Result<bool, PortError> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and alive for the call. Null data and size pointers only
        // ask whether the value exists.
        let status = unsafe { RegQueryValueExW(self.0, PCWSTR(name.as_ptr()), None, None, None, None) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(false);
        }
        status.ok().map_err(|e| registry("query", e))?;
        Ok(true)
    }

    /// Sets the REG_SZ value `name`, or the default value for `None`.
    pub(crate) fn set_string(&self, name: Option<&str>, data: &str) -> Result<(), PortError> {
        let name = name.map(wide);
        let name_ptr = name.as_ref().map_or(PCWSTR::null(), |name| PCWSTR(name.as_ptr()));
        // REG_SZ data is UTF-16LE, terminating NUL included.
        let bytes: Vec<u8> = wide(data).iter().flat_map(|unit| unit.to_le_bytes()).collect();
        // SAFETY: `name_ptr` is null or points into `name`, which lives for the call; `bytes` holds
        // the data for the length it names.
        let status = unsafe { RegSetValueExW(self.0, name_ptr, None, REG_SZ, Some(bytes.as_slice())) };
        status.ok().map_err(|e| registry("set", e))
    }

    /// Deletes the value `name`. A missing value is not an error.
    pub(crate) fn delete_value(&self, name: &str) -> Result<(), PortError> {
        let name = wide(name);
        // SAFETY: `name` is NUL-terminated and alive for the call.
        let status = unsafe { RegDeleteValueW(self.0, PCWSTR(name.as_ptr())) };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(());
        }
        status.ok().map_err(|e| registry("delete value", e))
    }
}

#[cfg(windows)]
impl Drop for OpenKey {
    fn drop(&mut self) {
        // SAFETY: the handle came from a successful open or create and is closed exactly once. A
        // failed close leaves nothing a caller could act on, so the status is dropped.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

/// `text` as NUL-terminated UTF-16, the form the registry and `PCWSTR` take.
#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
fn registry(action: &str, e: windows::core::Error) -> PortError {
    PortError::Failed(format!("registry {action}: {e}"))
}

#[cfg(windows)]
impl ShellIntegration for WindowsShellIntegration {
    fn is_installed(&self) -> Result<bool, PortError> {
        for target in ["*", "Directory"] {
            if OpenKey::open(&verb_key(&self.software, target))?.is_none() {
                return Ok(false);
            }
        }
        // The registry verbs are there; only the Send To file needs the folder.
        Ok(self.send_to_dir.clone()?.join(SEND_TO_FILE).is_file())
    }

    fn install(&self, executable: &Path) -> Result<(), PortError> {
        let send_to_dir = self.send_to_dir.clone()?;
        for target in ["*", "Directory"] {
            let key = verb_key(&self.software, target);
            let verb = OpenKey::create(&key)?;
            verb.set_string(None, VERB)?;
            verb.set_string(Some("Icon"), &executable.display().to_string())?;
            let command = OpenKey::create(&format!(r"{key}\command"))?;
            command.set_string(None, &format!("\"{}\" \"%1\"", executable.display()))?;
        }
        for (ext, prog_id, description) in FILE_TYPES {
            OpenKey::create(&class_key(&self.software, ext))?.set_string(None, prog_id)?;
            let class = class_key(&self.software, prog_id);
            OpenKey::create(&class)?.set_string(None, description)?;
            OpenKey::create(&format!(r"{class}\DefaultIcon"))?.set_string(None, &format!("\"{}\",0", executable.display()))?;
            OpenKey::create(&format!(r"{class}\shell\open\command"))?.set_string(None, &format!("\"{}\" \"%1\"", executable.display()))?;
        }
        write_send_to(&send_to_dir, executable)
    }

    /// The registry entries go first, whatever happens to the Send To folder: an uninstall with
    /// no `APPDATA` must not leave verbs and file types that point at a removed program.
    fn uninstall(&self) -> Result<(), PortError> {
        // Run every step even when one fails, then report the first failure.
        let mut steps: Vec<Result<(), PortError>> = Vec::new();
        for target in ["*", "Directory"] {
            steps.push(OpenKey::delete_tree(&verb_key(&self.software, target)));
        }
        for (ext, prog_id, _) in FILE_TYPES {
            // Leave an extension that another program has taken over.
            steps.push(match OpenKey::open(&class_key(&self.software, ext)) {
                Ok(Some(key)) => {
                    drop(key);
                    OpenKey::delete_tree(&class_key(&self.software, ext))
                }
                Ok(None) => Ok(()),
                Err(e) => Err(e),
            });
            steps.push(OpenKey::delete_tree(&class_key(&self.software, prog_id)));
        }
        steps.push(self.send_to_dir.clone().and_then(|dir| remove_send_to(&dir)));
        steps.into_iter().collect()
    }

    fn install_native_messaging(&self, nmh: &Path) -> Result<(), PortError> {
        let nmh_dir = self.nmh_dir.clone()?;
        if !nmh.is_absolute() {
            return Err(PortError::Failed(format!("the native host path must be absolute: {}", nmh.display())));
        }
        let (chrome, firefox) = write_host_manifests(&nmh_dir, nmh)?;
        register_host(&host_key(&self.software, r"Google\Chrome"), &chrome)?;
        register_host(&host_key(&self.software, "Mozilla"), &firefox)
    }
}

#[cfg(windows)]
impl WindowsShellIntegration {
    /// Removes what `install_native_messaging` wrote: the two browser keys and the manifests. The
    /// uninstaller calls it, because the host program the manifests name is about to be deleted.
    pub fn remove_native_messaging(&self) -> Result<(), PortError> {
        OpenKey::delete_tree(&host_key(&self.software, r"Google\Chrome"))?;
        OpenKey::delete_tree(&host_key(&self.software, "Mozilla"))?;
        let dir = self.nmh_dir.clone()?;
        for manifest in [CHROME_MANIFEST, FIREFOX_MANIFEST] {
            match std::fs::remove_file(dir.join(manifest)) {
                Ok(()) => {}
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(PortError::Failed(format!("{manifest}: {e}"))),
            }
        }
        Ok(())
    }
}

/// Points a browser's native-messaging key at its manifest.
#[cfg(windows)]
fn register_host(key: &str, manifest: &Path) -> Result<(), PortError> {
    OpenKey::create(key)?.set_string(None, &manifest.display().to_string())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    use super::{
        FILE_TYPES, chrome_manifest, class_key, firefox_manifest, host_key, json_string, remove_send_to, send_to_script, verb_key,
        write_host_manifests, write_send_to,
    };

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laqta-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn registry_paths_sit_under_the_software_key() {
        assert_eq!(verb_key("Software", "*"), r"Software\Classes\*\shell\Laqta");
        assert_eq!(class_key("Software", ".sxcu"), r"Software\Classes\.sxcu");
        assert_eq!(FILE_TYPES.map(|t| t.0), [".sxcu", ".sxie", ".sxb"]);
        assert_eq!(verb_key("Software", "Directory"), r"Software\Classes\Directory\shell\Laqta");
        assert_eq!(host_key("Software", r"Google\Chrome"), r"Software\Google\Chrome\NativeMessagingHosts\com.laqta.nmh");
        assert_eq!(host_key("Software", "Mozilla"), r"Software\Mozilla\NativeMessagingHosts\com.laqta.nmh");
    }

    #[test]
    fn json_string_doubles_windows_backslashes_and_escapes_quotes() {
        assert_eq!(json_string(r#"C:\a "b""#), r#""C:\\a \"b\"""#);
        assert_eq!(json_string("\u{1}"), r#""\u0001""#);
    }

    #[test]
    fn chrome_manifest_names_the_host_path_and_origin() {
        let text = chrome_manifest(Path::new(r"C:\Program Files\Laqta\nmh.exe"));
        assert!(text.contains("\"name\": \"com.laqta.nmh\""));
        assert!(text.contains(r#""path": "C:\\Program Files\\Laqta\\nmh.exe""#));
        assert!(text.contains("\"allowed_origins\": [\"chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/\"]"));
    }

    #[test]
    fn firefox_manifest_lists_the_addon_id_not_the_origin() {
        let text = firefox_manifest(Path::new(r"C:\Laqta\nmh.exe"));
        assert!(text.contains("\"allowed_extensions\": [\"ShareX@ShareX\"]"));
        assert!(!text.contains("allowed_origins"));
    }

    #[test]
    fn send_to_script_starts_laqta_with_the_selection_and_uses_crlf() {
        assert_eq!(send_to_script(Path::new(r"C:\Laqta\laqta.exe")), "@start \"\" \"C:\\Laqta\\laqta.exe\" %*\r\n");
    }

    #[test]
    fn send_to_script_doubles_percent_signs_in_the_path() {
        assert_eq!(send_to_script(Path::new(r"C:\50%off\laqta.exe")), "@start \"\" \"C:\\50%%off\\laqta.exe\" %*\r\n");
    }

    #[test]
    fn host_manifests_and_send_to_script_write_and_remove_idempotently() {
        let dir = scratch("integration-files");
        let exe = Path::new(r"C:\Laqta\laqta.exe");
        let (chrome, firefox) = write_host_manifests(&dir, exe).expect("manifests");
        assert_ne!(chrome, firefox);
        assert!(fs::read_to_string(&chrome).expect("chrome").contains("allowed_origins"));
        assert!(fs::read_to_string(&firefox).expect("firefox").contains("allowed_extensions"));
        write_send_to(&dir, exe).expect("send to");
        write_send_to(&dir, exe).expect("send to again");
        assert!(dir.join("Laqta.cmd").is_file());
        remove_send_to(&dir).expect("remove");
        remove_send_to(&dir).expect("remove again");
        assert!(!dir.join("Laqta.cmd").exists());
        let _ = fs::remove_dir_all(&dir);
    }
}

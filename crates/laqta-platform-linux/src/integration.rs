//! Shell integration on Linux: "Upload with Laqta" for the file managers, and the
//! browser native-messaging host manifests. Each root is a field, so tests point them at a temp
//! dir; `default()` uses the XDG data directory and the browsers' own locations.

use std::path::{Path, PathBuf};

use laqta_ports::{PortError, ShellIntegration};

use crate::autostart::{exec_value, launcher, remove_if_exists, write_file, xdg_dir};

/// The native-messaging host name both browsers look up, and the callers it accepts.
const HOST_NAME: &str = "com.laqta.nmh";
const CHROME_ORIGIN: &str = "chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/";
const FIREFOX_ID: &str = "ShareX@ShareX";
const MANIFEST_FILE: &str = "com.laqta.nmh.json";

/// Named after the portal app id, so the visible entry and the portals' entry are one file.
const DESKTOP_FILE: &str = "io.github.etherll.Laqta.desktop";
const SERVICE_MENU_FILE: &str = "laqta-upload.desktop";
const SCRIPT_NAME: &str = "Upload with Laqta";

/// The Linux shell entries and browser host manifests, below the roots it was built with.
#[derive(Debug)]
pub struct LinuxShellIntegration {
    /// `$XDG_DATA_HOME`: holds `applications`, `kio/servicemenus` and `nautilus/scripts`.
    data_home: PathBuf,
    /// The Chrome and Chromium `NativeMessagingHosts` directories.
    chromium_hosts: Vec<PathBuf>,
    /// Firefox's `native-messaging-hosts` directory.
    firefox_hosts: PathBuf,
}

impl Default for LinuxShellIntegration {
    fn default() -> Self {
        let home = std::env::home_dir().unwrap_or_default();
        Self {
            data_home: xdg_dir("XDG_DATA_HOME", ".local/share"),
            chromium_hosts: vec![
                home.join(".config/google-chrome/NativeMessagingHosts"),
                home.join(".config/chromium/NativeMessagingHosts"),
            ],
            firefox_hosts: home.join(".mozilla/native-messaging-hosts"),
        }
    }
}

impl LinuxShellIntegration {
    /// The three file manager entries: the application's desktop file, the KDE service menu and the
    /// Nautilus script.
    fn entries(&self) -> [PathBuf; 4] {
        [
            self.data_home.join("applications").join(DESKTOP_FILE),
            self.data_home.join("kio").join("servicemenus").join(SERVICE_MENU_FILE),
            self.data_home.join("nautilus").join("scripts").join(SCRIPT_NAME),
            self.data_home.join("mime").join("packages").join(MIME_FILE),
        ]
    }

    /// Refreshes the MIME and desktop databases so the file types take effect. Best effort: the
    /// tools are optional and a missing one only delays the change until the next login.
    fn refresh_databases(&self) {
        for (tool, dir) in
            [("update-mime-database", self.data_home.join("mime")), ("update-desktop-database", self.data_home.join("applications"))]
        {
            let _ = std::process::Command::new(tool)
                .arg(&dir)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }
}

impl ShellIntegration for LinuxShellIntegration {
    fn is_installed(&self) -> Result<bool, PortError> {
        Ok(self.entries().iter().all(|path| path.is_file()))
    }

    fn install(&self, executable: &Path) -> Result<(), PortError> {
        let executable = &launcher(executable);
        let [desktop, service_menu, script, mime] = self.entries();
        write_file(&desktop, &desktop_file(executable), 0o644)?;
        write_file(&service_menu, &service_menu_file(executable), 0o755)?;
        write_file(&script, &nautilus_script(executable), 0o755)?;
        write_file(&mime, MIME_XML, 0o644)?;
        self.refresh_databases();
        Ok(())
    }

    fn uninstall(&self) -> Result<(), PortError> {
        for path in self.entries() {
            remove_if_exists(&path)?;
        }
        self.refresh_databases();
        Ok(())
    }

    fn install_native_messaging(&self, nmh: &Path) -> Result<(), PortError> {
        if !nmh.is_absolute() {
            return Err(PortError::Failed(format!("the native host path must be absolute: {}", nmh.display())));
        }
        for dir in &self.chromium_hosts {
            write_file(&dir.join(MANIFEST_FILE), &chrome_manifest(nmh), 0o644)?;
        }
        write_file(&self.firefox_hosts.join(MANIFEST_FILE), &firefox_manifest(nmh), 0o644)
    }
}

/// ShareX's file types (custom uploaders, image effects, backups), opened by Laqta to import them.
const MIME_FILE: &str = "laqta.xml";
const MIME_XML: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<mime-info xmlns="http://www.freedesktop.org/standards/shared-mime-info">
  <mime-type type="application/x-sharex-custom-uploader">
    <comment>ShareX custom uploader</comment>
    <glob pattern="*.sxcu"/>
  </mime-type>
  <mime-type type="application/x-sharex-image-effect">
    <comment>ShareX image effect</comment>
    <glob pattern="*.sxie"/>
  </mime-type>
  <mime-type type="application/x-sharex-backup">
    <comment>ShareX backup</comment>
    <glob pattern="*.sxb"/>
  </mime-type>
</mime-info>
"#;
const MIME_TYPES: &str = "application/x-sharex-custom-uploader;application/x-sharex-image-effect;application/x-sharex-backup;";

/// The application's desktop file. `Actions=upload` puts the upload action on the entry; the MIME
/// types open ShareX's files in Laqta.
fn desktop_file(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Laqta\nIcon={}\nExec={}\nMimeType={MIME_TYPES}\nActions=upload;\n\n[Desktop Action upload]\nName=Upload with Laqta\nExec={}\n",
        crate::app_id::APP_ID,
        exec_value(executable, " %F"),
        exec_value(executable, " %F"),
    )
}

/// A KDE service menu for every file type. KDE only lists an executable service menu.
fn service_menu_file(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Service\nServiceTypes=KonqPopupMenu/Plugin\nMimeType=all/allfiles;\nActions=upload;\n\n[Desktop Action upload]\nName=Upload with Laqta\nExec={}\n",
        exec_value(executable, " %F"),
    )
}

/// A Nautilus script. Nautilus passes the selection in `NAUTILUS_SCRIPT_SELECTED_FILE_PATHS`, one
/// path per line, and no arguments, so the script splits that list into the arguments when none
/// were given.
fn nautilus_script(executable: &Path) -> String {
    let lines = [
        "#!/bin/sh",
        "# Nautilus passes the selection in NAUTILUS_SCRIPT_SELECTED_FILE_PATHS, one path per line.",
        "set -f",
        "IFS='\n'",
        "if [ \"$#\" -eq 0 ]; then",
        "  set -- $NAUTILUS_SCRIPT_SELECTED_FILE_PATHS",
        "fi",
    ];
    let mut script = String::new();
    for line in lines {
        script.push_str(line);
        script.push('\n');
    }
    script.push_str(&format!("exec \"{}\" \"$@\"\n", sh_double_quoted(&executable.display().to_string())));
    script
}

/// `text` for use inside sh double quotes: `\`, `"`, `$` and backticks are escaped.
fn sh_double_quoted(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        if matches!(ch, '\\' | '"' | '$' | '`') {
            out.push('\\');
        }
        out.push(ch);
    }
    out
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

/// `text` as a JSON string literal, with quotes, backslashes and control characters escaped.
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

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use laqta_ports::ShellIntegration;

    use super::{LinuxShellIntegration, json_string, nautilus_script};
    use crate::autostart::write_file;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("laqta-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn rooted(dir: &Path) -> LinuxShellIntegration {
        LinuxShellIntegration {
            data_home: dir.join("share"),
            chromium_hosts: vec![dir.join("chrome"), dir.join("chromium")],
            firefox_hosts: dir.join("firefox"),
        }
    }

    #[test]
    fn install_writes_the_file_manager_entries_and_uninstall_removes_them() {
        let dir = scratch("integration-entries");
        let integration = rooted(&dir);
        let exe = Path::new("/opt/laqta/laqta");
        assert!(!integration.is_installed().expect("check"));
        integration.install(exe).expect("install");
        integration.install(exe).expect("install again");
        assert!(integration.is_installed().expect("check"));

        let desktop = fs::read_to_string(dir.join("share/applications/io.github.etherll.Laqta.desktop")).expect("desktop");
        assert!(desktop.contains("Actions=upload;\n"));
        assert!(desktop.contains("MimeType=application/x-sharex-custom-uploader;"));
        let mime = fs::read_to_string(dir.join("share/mime/packages/laqta.xml")).expect("mime");
        assert!(mime.contains("<glob pattern=\"*.sxb\"/>"));
        assert!(desktop.ends_with("[Desktop Action upload]\nName=Upload with Laqta\nExec=\"/opt/laqta/laqta\" %F\n"));
        let menu_path = dir.join("share/kio/servicemenus/laqta-upload.desktop");
        let menu = fs::read_to_string(&menu_path).expect("service menu");
        assert!(menu.contains("MimeType=all/allfiles;\n"));
        assert!(menu.ends_with("Exec=\"/opt/laqta/laqta\" %F\n"));
        let script_path = dir.join("share/nautilus/scripts/Upload with Laqta");
        for path in [&menu_path, &script_path] {
            let mode = fs::metadata(path).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }

        integration.uninstall().expect("uninstall");
        assert!(!integration.is_installed().expect("check"));
        integration.uninstall().expect("uninstall again");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_messaging_writes_chrome_and_firefox_manifests() {
        let dir = scratch("integration-nmh");
        let integration = rooted(&dir);
        integration.install_native_messaging(Path::new("/opt/Laqta/nmh")).expect("register");
        for chrome in ["chrome", "chromium"] {
            let text = fs::read_to_string(dir.join(chrome).join("com.laqta.nmh.json")).expect("chrome manifest");
            assert!(text.contains("\"name\": \"com.laqta.nmh\""));
            assert!(text.contains("\"path\": \"/opt/Laqta/nmh\""));
            assert!(text.contains("\"allowed_origins\": [\"chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/\"]"));
        }
        let firefox = fs::read_to_string(dir.join("firefox").join("com.laqta.nmh.json")).expect("firefox manifest");
        assert!(firefox.contains("\"allowed_extensions\": [\"ShareX@ShareX\"]"));
        assert!(!firefox.contains("allowed_origins"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn native_host_path_must_be_absolute() {
        let dir = scratch("integration-relative");
        assert!(rooted(&dir).install_native_messaging(Path::new("nmh")).is_err());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn json_string_escapes_quotes_backslashes_and_control_characters() {
        assert_eq!(json_string(r#"C:\a "b""#), r#""C:\\a \"b\"""#);
        assert_eq!(json_string("\u{1}"), r#""\u0001""#);
    }

    #[test]
    fn nautilus_script_uses_the_selection_when_nautilus_passes_no_arguments() {
        let dir = scratch("integration-nautilus");
        let path = dir.join("Upload with Laqta");
        write_file(&path, &nautilus_script(Path::new("/bin/echo")), 0o755).expect("write script");
        let run = |args: &[&str], selection: &str| {
            // A test thread that forks while the script is still open for writing holds that handle in
            // its child until the child's exec, and running the script fails with ETXTBSY meanwhile.
            let mut tries = 0;
            loop {
                match Command::new(&path).args(args).env("NAUTILUS_SCRIPT_SELECTED_FILE_PATHS", selection).output() {
                    Err(e) if e.kind() == std::io::ErrorKind::ExecutableFileBusy && tries < 50 => {
                        tries += 1;
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    other => break other.expect("run script"),
                }
            }
        };
        let from_selection = run(&[], "/a b/c.png\n/d*.png\n");
        assert_eq!(String::from_utf8_lossy(&from_selection.stdout), "/a b/c.png /d*.png\n");
        let given = run(&["/given.png"], "/ignored.png\n");
        assert_eq!(String::from_utf8_lossy(&given.stdout), "/given.png\n");
        let _ = fs::remove_dir_all(&dir);
    }
}

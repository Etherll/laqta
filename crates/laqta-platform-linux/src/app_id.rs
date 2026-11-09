//! Laqta's application id for the desktop portals. A host (unsandboxed) app has no id the
//! portals can see, and KDE's GlobalShortcuts portal refuses a session without one ("An app id is
//! required"). So, before any other portal call, Laqta makes sure a desktop entry named after
//! its id exists and registers the id with `org.freedesktop.host.portal.Registry`. It also installs
//! the app icon into the user's hicolor theme under the same id, which the desktop entry, the
//! windows (their Wayland app id) and the tray all name.

use std::path::{Path, PathBuf};
use std::time::Duration;

use futures_lite::future;

use crate::autostart::{exec_value, write_file, xdg_dir};

/// The reverse-DNS id: the desktop entry's file name and the id the portals key permissions on.
pub const APP_ID: &str = "io.github.etherll.Laqta";

/// The app icon, installed as `icons/hicolor/<size>/apps/<APP_ID>.png` (and `.svg` under
/// `scalable`).
const ICONS: [(&str, &[u8]); 7] = [
    ("16x16", include_bytes!("../../../assets/icon/laqta-16.png")),
    ("32x32", include_bytes!("../../../assets/icon/laqta-32.png")),
    ("64x64", include_bytes!("../../../assets/icon/laqta-64.png")),
    ("128x128", include_bytes!("../../../assets/icon/laqta-128.png")),
    ("256x256", include_bytes!("../../../assets/icon/laqta-256.png")),
    ("512x512", include_bytes!("../../../assets/icon/laqta-512.png")),
    ("scalable", include_bytes!("../../../assets/icon/laqta.svg")),
];

/// The directory the tray adds to its icon search path, so it finds the icon before the
/// desktop's icon cache knows about it.
pub fn icon_theme_dir() -> PathBuf {
    xdg_dir("XDG_DATA_HOME", ".local/share").join("icons")
}

/// Writes each icon size that is missing or out of date. Returns whether it wrote any.
fn install_icons(data_home: &Path) -> bool {
    let mut wrote = false;
    for (size, bytes) in ICONS {
        let ext = if size == "scalable" { "svg" } else { "png" };
        let path = data_home.join("icons/hicolor").join(size).join("apps").join(format!("{APP_ID}.{ext}"));
        if std::fs::read(&path).is_ok_and(|old| old == bytes) {
            continue;
        }
        let written = path.parent().map_or(Ok(()), std::fs::create_dir_all).and_then(|()| std::fs::write(&path, bytes));
        match written {
            Ok(()) => wrote = true,
            Err(e) => tracing::warn!("icon {} not written: {e}", path.display()),
        }
    }
    wrote
}

/// Points an existing entry (written by an older version, or the shell integration) at the app
/// icon. Returns the new text, or `None` when it already names it.
fn with_icon(entry: &str) -> Option<String> {
    let line = format!("Icon={APP_ID}");
    if entry.lines().any(|l| l == line) {
        return None;
    }
    let mut out = String::with_capacity(entry.len() + line.len() + 1);
    let mut placed = false;
    for l in entry.lines().filter(|l| !l.starts_with("Icon=")) {
        out.push_str(l);
        out.push('\n');
        if !placed && l == "[Desktop Entry]" {
            out.push_str(&line);
            out.push('\n');
            placed = true;
        }
    }
    placed.then_some(out)
}

/// How long registration may take before startup carries on without it.
const BUDGET: Duration = Duration::from_secs(2);

/// The desktop entry for `APP_ID`. Hidden from menus: it only names the app for the portals.
/// The shell integration writes a full, visible entry to the same file when the user turns it on.
pub(crate) fn desktop_file_path(data_home: &Path) -> PathBuf {
    data_home.join("applications").join(format!("{APP_ID}.desktop"))
}

fn minimal_entry(executable: &Path) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName=Laqta\nComment=Screen capture, recording and upload\nExec={}\nIcon={APP_ID}\nTerminal=false\nNoDisplay=true\nCategories=Graphics;Utility;\n",
        exec_value(executable, "")
    )
}

/// Writes the minimal entry unless an entry for the id is already there, in which case it only
/// makes sure the entry names the app icon. Returns whether it wrote anything (the desktop's
/// application cache then needs a refresh).
fn ensure_entry(data_home: &Path, executable: &Path) -> bool {
    let path = desktop_file_path(data_home);
    if let Ok(old) = std::fs::read_to_string(&path) {
        let Some(new) = with_icon(&old) else { return false };
        return write_file(&path, &new, 0o644).is_ok();
    }
    match write_file(&path, &minimal_entry(executable), 0o644) {
        Ok(()) => true,
        Err(e) => {
            tracing::warn!("desktop entry {} not written: {e}", path.display());
            false
        }
    }
}

/// Makes sure the desktop entry exists and registers `APP_ID` with the portals. Best effort and
/// bounded: on failure the portals behave as before and the reason is logged.
pub fn register(executable: &Path) {
    let data_home = xdg_dir("XDG_DATA_HOME", ".local/share");
    let icons = install_icons(&data_home);
    if ensure_entry(&data_home, executable) | icons {
        // KDE looks entries up in its service cache; refresh it so the new one is found now.
        for tool in ["kbuildsycoca6", "kbuildsycoca5"] {
            if std::process::Command::new(tool).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).status().is_ok() {
                break;
            }
        }
    }
    let Ok(id) = ashpd::AppID::try_from(APP_ID) else {
        tracing::warn!("{APP_ID} is not a valid app id");
        return;
    };
    let registered = future::block_on(future::or(async { Some(ashpd::register_host_app(id).await) }, async {
        async_io::Timer::after(BUDGET).await;
        None
    }));
    match registered {
        Some(Ok(())) => tracing::info!("registered with the portals as {APP_ID}"),
        Some(Err(e)) => tracing::warn!("portal app id registration failed: {e}"),
        None => tracing::warn!("portal app id registration timed out"),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn the_entry_is_named_after_the_id_and_kept_if_present() {
        let dir = std::env::temp_dir().join(format!("laqta-{}-app-id", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let exe = Path::new("/opt/laqta/laqta");
        assert!(ensure_entry(&dir, exe));
        let path = desktop_file_path(&dir);
        assert!(path.ends_with("applications/io.github.etherll.Laqta.desktop"));
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.contains("/opt/laqta/laqta") && text.contains("NoDisplay=true"), "{text}");
        assert!(!ensure_entry(&dir, exe), "an existing entry is left alone");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn icons_are_installed_once_and_an_old_entry_gets_the_icon() {
        let dir = std::env::temp_dir().join(format!("laqta-{}-icons", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(install_icons(&dir));
        assert!(dir.join("icons/hicolor/scalable/apps/io.github.etherll.Laqta.svg").is_file());
        assert!(dir.join("icons/hicolor/16x16/apps/io.github.etherll.Laqta.png").is_file());
        assert!(!install_icons(&dir), "up-to-date icons are left alone");

        let old = "[Desktop Entry]\nType=Application\nName=Laqta\nIcon=camera-photo\nExec=laqta\n";
        let path = desktop_file_path(&dir);
        write_file(&path, old, 0o644).unwrap();
        assert!(ensure_entry(&dir, Path::new("/opt/laqta/laqta")));
        let text = std::fs::read_to_string(&path).unwrap();
        assert_eq!(text, "[Desktop Entry]\nIcon=io.github.etherll.Laqta\nType=Application\nName=Laqta\nExec=laqta\n");
        assert!(!ensure_entry(&dir, Path::new("/opt/laqta/laqta")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_id_is_a_valid_portal_app_id() {
        assert!(ashpd::AppID::try_from(APP_ID).is_ok());
    }
}

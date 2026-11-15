//! "Check for updates": the latest GitHub release, installed in place where this copy can replace
//! itself (`laqta_update::install::Installer`). This module runs what the crate decides: the downloads
//! on the HTTP runtime, the file work and the commands on the caller's executor. Laqta quits after a
//! successful install and the new version opens in its place.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use laqta_update::install::{self, Installer, Platform, Staging};
use laqta_update::{HttpFetcher, Release, UpdateError, Version};

fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| unreachable!("the crate version parses"))
}

/// What the last check found: the release, and how this copy installs it (`None`: it can't).
#[derive(Clone, Debug)]
struct Found {
    release: Release,
    installer: Option<Installer>,
}

/// The settings window's "Check for updates" and "Install and restart": what the last check found,
/// and one install at a time (a second would clear the first one's staging folder).
#[derive(Debug)]
pub struct Updater {
    data: PathBuf,
    found: Mutex<Option<Found>>,
    installing: AtomicBool,
}

impl Updater {
    pub fn new(data: PathBuf) -> Updater {
        Updater { data, found: Mutex::new(None), installing: AtomicBool::new(false) }
    }

    /// The settings line, and whether this copy can install what was found.
    pub async fn check(&self) -> Result<(String, bool), String> {
        let release = laqta_http::run(check()).await?;
        let installer = installer();
        let line = describe(release.as_ref(), installer.is_some(), runs_from_appimage());
        let installable = release.is_some() && installer.is_some();
        if let Ok(mut slot) = self.found.lock() {
            *slot = release.map(|release| Found { release, installer });
        }
        Ok((line, installable))
    }

    /// Installs what the last check found, the way that check said it would. The caller quits Laqta on
    /// `Ok`.
    pub async fn install(&self) -> Result<(), String> {
        let found = self.found.lock().ok().and_then(|slot| slot.clone()).ok_or("check for updates first")?;
        let installer = found.installer.ok_or("this copy doesn't update itself; download the new version instead")?;
        let _installing = self.begin().ok_or("an update is already being installed")?;
        install(found.release, self.data.clone(), installer).await
    }

    /// Claims the one install slot; the guard frees it on every way out, a panic included.
    fn begin(&self) -> Option<Installing<'_>> {
        (!self.installing.swap(true, Ordering::AcqRel)).then_some(Installing(&self.installing))
    }
}

struct Installing<'a>(&'a AtomicBool);

impl Drop for Installing<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// The latest release, if it is newer than this copy.
pub async fn check() -> Result<Option<Release>, String> {
    let current = current();
    let fetcher = HttpFetcher::new(&current);
    laqta_update::check(&fetcher, &current).await.map_err(|e| e.to_string())
}

/// The settings line for what the check found. `can_install`: this copy replaces itself.
/// `appimage`: it runs from an AppImage (one in a folder this user can't write to lands here).
fn describe(release: Option<&Release>, can_install: bool, appimage: bool) -> String {
    match release {
        None => format!("Laqta {} is up to date.", env!("CARGO_PKG_VERSION")),
        Some(r) if can_install => format!("Laqta {} is available ({}). Install it from here.", r.tag, r.url),
        Some(r) if cfg!(any(windows, target_os = "macos")) || appimage => {
            format!("Laqta {} is available. This copy doesn't update itself: download it from {}", r.tag, r.url)
        }
        Some(r) => format!("Laqta {} is available: {}. Update through your package manager or download it there.", r.tag, r.url),
    }
}

fn var(name: &str) -> Option<PathBuf> {
    std::env::var_os(name).map(PathBuf::from)
}

/// How this copy updates itself; `None` when something else updates it.
fn installer() -> Option<Installer> {
    let exe = std::env::current_exe().ok()?;
    Installer::detect(Platform::current(), &exe, var("APPIMAGE").as_deref(), var("APPDIR").as_deref())
}

fn runs_from_appimage() -> bool {
    std::env::current_exe().is_ok_and(|exe| install::runs_from_appimage(&exe, var("APPDIR").as_deref()))
}

/// Downloads `release` and puts it in place. Only the downloads run on the HTTP runtime
/// (`laqta_http::run`, which needs owned, `'static` work); staging, unpacking, swapping and the
/// process starts block, so they stay on the caller's executor and never hold up uploads.
async fn install(release: Release, data: PathBuf, installer: Installer) -> Result<(), String> {
    let text = |e: UpdateError| e.to_string();
    match installer {
        Installer::Velopack { update_exe } => {
            let dir = data.join("updates");
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let package = laqta_http::run(async move { laqta_update::download_windows_update(&fetcher(), &release, &dir).await })
                .await
                .map_err(text)?;
            let (program, args) = laqta_update::velopack_apply_command(&update_exe, &package);
            Command::new(program).args(args).spawn().map_err(|e| format!("can't start the installer: {e}"))?;
        }
        Installer::AppImage { image } => {
            let staging = Staging::beside(&image).map_err(text)?;
            let new = download(release, laqta_update::APPIMAGE_ASSET, staging.path()).await?;
            install::replace_appimage(&new, &image).map_err(text)?;
            drop(staging);
            restart_after_exit(&image, None, false);
        }
        Installer::MacApp { bundle } => {
            let staging = Staging::beside(&bundle).map_err(text)?;
            let zip = download(release, laqta_update::MAC_APP_ASSET, staging.path()).await?;
            let (program, args) = install::unpack_command(&zip, staging.path());
            match Command::new(program).args(args).status() {
                Ok(status) if status.success() => {}
                Ok(status) => return Err(format!("can't unpack the update (ditto {status})")),
                Err(e) => return Err(format!("can't unpack the update: {e}")),
            }
            let old = install::replace_mac_app(&staging, &bundle).map_err(text)?;
            drop(staging);
            restart_after_exit(&bundle, Some(&old), true);
        }
    }
    Ok(())
}

/// Downloads the release's `asset` into `dir` on the HTTP runtime, checked against `SHA256SUMS`.
async fn download(release: Release, asset: &'static str, dir: &Path) -> Result<PathBuf, String> {
    let dir = dir.to_path_buf();
    laqta_http::run(async move { laqta_update::download_asset(&fetcher(), &release, asset, &dir).await }).await.map_err(|e| e.to_string())
}

fn fetcher() -> HttpFetcher {
    HttpFetcher::new(&current())
}

/// Starts the updated copy once this process has exited.
/// The new version is already in place, so a restart that can't start is not an install failure:
/// Laqta still quits, and the next start runs the new version.
fn restart_after_exit(target: &Path, remove: Option<&Path>, bundle: bool) {
    let (program, args) = install::restart_command(target, std::process::id(), remove, bundle);
    let mut command = Command::new(program);
    command.args(args);
    for var in install::APPIMAGE_RUNTIME_VARS {
        command.env_remove(var);
    }
    if let Err(error) = command.spawn() {
        tracing::warn!(%error, "the update is installed, but Laqta can't restart itself");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        Release {
            version: Version::parse("9.0.0").unwrap_or_else(|_| unreachable!()),
            tag: "v9.0.0".into(),
            notes: String::new(),
            url: "https://example.invalid/v9".into(),
            assets: Vec::new(),
        }
    }

    #[test]
    fn the_settings_line_says_how_to_get_the_release() {
        let r = release();
        assert!(describe(None, false, false).ends_with("is up to date."));
        assert!(describe(Some(&r), true, false).ends_with("Install it from here."));
        let appimage = describe(Some(&r), false, true);
        assert!(appimage.contains("download it from"), "{appimage}");
        if cfg!(target_os = "linux") {
            assert!(describe(Some(&r), false, false).contains("package manager"), "a .deb, archive or Nix copy");
        }
    }

    #[test]
    fn an_install_follows_what_the_check_found() {
        let updater = Updater::new(PathBuf::from("data"));
        let none = futures_lite_block_on(updater.install());
        assert_eq!(none, Err("check for updates first".to_owned()));
        *updater.found.lock().unwrap_or_else(|e| e.into_inner()) = Some(Found { release: release(), installer: None });
        let refused = futures_lite_block_on(updater.install());
        assert_eq!(refused, Err("this copy doesn't update itself; download the new version instead".to_owned()));
    }

    /// Polls a future that finishes without waiting (these calls return before any I/O).
    fn futures_lite_block_on<F: std::future::Future>(future: F) -> F::Output {
        let waker = std::task::Waker::noop();
        let mut context = std::task::Context::from_waker(waker);
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(output) => output,
            std::task::Poll::Pending => unreachable!("the call returned before any I/O"),
        }
    }

    #[test]
    fn one_install_runs_at_a_time_and_the_slot_frees_when_it_ends() {
        let updater = Updater::new(PathBuf::from("data"));
        let first = updater.begin();
        assert!(first.is_some());
        assert!(updater.begin().is_none(), "a second install is refused while one runs");
        drop(first);
        assert!(updater.begin().is_some(), "the slot is free again");
    }

    #[test]
    fn the_slot_frees_when_an_install_panics() {
        let updater = Updater::new(PathBuf::from("data"));
        let caught = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let _installing = updater.begin();
            panic!("install failed");
        }));
        assert!(caught.is_err());
        assert!(updater.begin().is_some());
    }
}

//! "Check for updates": the latest GitHub release; on Windows the Velopack package is
//! downloaded through verified fetch and applied with `Update.exe`, then Laqta quits.

use std::path::PathBuf;

use laqta_update::{HttpFetcher, Release, Version};

fn current() -> Version {
    Version::parse(env!("CARGO_PKG_VERSION")).unwrap_or_else(|_| unreachable!("the crate version parses"))
}

/// A line for the settings window: "up to date", or the new version and its page.
pub async fn check() -> Result<Option<Release>, String> {
    let current = current();
    let fetcher = HttpFetcher::new(&current);
    laqta_update::check(&fetcher, &current).await.map_err(|e| e.to_string())
}

pub fn describe(release: &Option<Release>) -> String {
    match release {
        None => format!("Laqta {} is up to date.", env!("CARGO_PKG_VERSION")),
        Some(r) if update_exe().is_some() => format!("Laqta {} is available ({}). Install it from here.", r.tag, r.url),
        Some(r) if cfg!(windows) => format!("Laqta {} is available. This copy doesn't update itself: download it from {}", r.tag, r.url),
        Some(r) => format!("Laqta {} is available: {}. Update through your package manager or download it there.", r.tag, r.url),
    }
}

/// The installer's `Update.exe`, when this copy came from the Windows setup program: Velopack's
/// layout puts it one folder above the running `current\laqta.exe`. `None` for a portable copy
/// and on Linux, where Laqta can't update itself.
pub fn update_exe() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    Some(exe.parent()?.parent()?.join("Update.exe")).filter(|path| path.exists())
}

/// Windows: downloads the package next to the install and starts `Update.exe apply`.
pub async fn install(release: Release, data: PathBuf) -> Result<(), String> {
    let update_exe = update_exe().ok_or("this copy was not installed with the setup program; download the new version instead")?;
    let dir = data.join("updates");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let current = current();
    let fetcher = HttpFetcher::new(&current);
    let package = laqta_update::download_windows_update(&fetcher, &release, &dir).await.map_err(|e| e.to_string())?;
    let (program, args) = laqta_update::velopack_apply_command(&update_exe, &package);
    std::process::Command::new(program).args(args).spawn().map_err(|e| e.to_string())?;
    Ok(())
}

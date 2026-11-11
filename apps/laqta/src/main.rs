// A GUI program on Windows: no console window opens with it (see `attach_parent_console`).
#![cfg_attr(windows, windows_subsystem = "windows")]
//! Composition root (ARCHITECTURE §2.5): parses the CLI, claims the single instance,
//! builds the runtime and ports, wires every entry point to `App::dispatch`, and runs GPUI.

mod blocking_pool;
mod cli;
mod cloud;
mod fonts;
mod imports;
mod local_models;
mod logging;
mod media;
mod models;
mod notifier;
mod ocr;
mod requests;
mod spawner;
mod stubs;
mod updates;
mod uploads;
mod wiring;

use std::process::ExitCode;

fn main() -> ExitCode {
    append_packaged_tools_to_path();
    #[cfg(windows)]
    laqta_platform_windows::attach_parent_console();
    // The Windows installer (Velopack) runs the app with `--veloapp-install`, `--veloapp-updated`,
    // `--veloapp-obsolete` or `--veloapp-uninstall` and expects it to exit at once.
    if let Some(hook) = std::env::args().nth(1).filter(|a| a.starts_with("--veloapp-")) {
        // On uninstall, remove the sign-in entry, the Explorer menu items and the browser host
        // registration, which all point at files the installer is about to delete.
        #[cfg(windows)]
        if hook == "--veloapp-uninstall" {
            let ports = laqta_platform_windows::ports();
            if let Err(e) = ports.autostart.set_enabled(false, std::path::Path::new("")) {
                eprintln!("laqta: {e}");
            }
            if let Err(e) = ports.integration.uninstall() {
                eprintln!("laqta: {e}");
            }
            if let Err(e) = laqta_platform_windows::remove_native_messaging() {
                eprintln!("laqta: {e}");
            }
            if let Err(e) = laqta_platform_windows::restore_print_screen_for_snipping() {
                eprintln!("laqta: {e}");
            }
        }
        let _ = hook;
        return ExitCode::SUCCESS;
    }
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::Command::Version) => {
            println!("laqta {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(cli::Command::Run(args)) => args,
        Err(msg) => {
            eprintln!("laqta: {msg}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };

    if args.startup_delay_secs > 0 {
        std::thread::sleep(std::time::Duration::from_secs(args.startup_delay_secs as u64));
    }
    let claim = if args.multi { None } else { Some(laqta_ipc::claim()) };
    let server = match claim {
        None => None,
        Some(Ok(laqta_ipc::Claim::Primary(server))) => Some(server),
        Some(Ok(laqta_ipc::Claim::Secondary(mut client))) => {
            // The running instance resolves paths against its own working directory, so make them absolute here.
            let absolute = |p: &std::path::PathBuf| std::path::absolute(p).unwrap_or_else(|_| p.clone());
            let mut forward: Vec<laqta_ipc::Request> =
                args.import_uploaders.iter().chain(&args.import_effects).map(|p| laqta_ipc::Request::Upload(absolute(p))).collect();
            forward.extend(args.requests.iter().map(|r| match r {
                laqta_ipc::Request::Upload(p) => laqta_ipc::Request::Upload(absolute(p)),
                laqta_ipc::Request::ActionFile(a, p) => laqta_ipc::Request::ActionFile(*a, absolute(p)),
                laqta_ipc::Request::UploadWith { workflow, path } => {
                    laqta_ipc::Request::UploadWith { workflow: workflow.clone(), path: absolute(path) }
                }
                other => other.clone(),
            }));
            if forward.is_empty() {
                println!("Laqta is already running");
                return ExitCode::SUCCESS;
            }
            for request in &forward {
                if let Err(e) = client.send_request(request) {
                    eprintln!("laqta: {e}");
                    return ExitCode::from(1);
                }
            }
            return ExitCode::SUCCESS;
        }
        Some(Err(e)) => {
            eprintln!("laqta: cannot claim the single-instance endpoint: {e}");
            return ExitCode::from(1);
        }
    };

    let _log_guard = logging::init();
    match wiring::run(server, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(msg) => {
            tracing::error!("{msg}");
            eprintln!("laqta: {msg}");
            ExitCode::from(1)
        }
    }
}

/// Appends the folders of helper programs (FFmpeg, `pactl`) that a distribution package named at
/// build time in `LAQTA_TOOLS_PATH` to `PATH`, after the user's own folders, so a copy the user
/// installed still wins. It is built in rather than set by a wrapper script because the desktop
/// entries start `current_exe`, which under a wrapper is the unwrapped program.
fn append_packaged_tools_to_path() {
    let Some(tools) = option_env!("LAQTA_TOOLS_PATH").filter(|tools| !tools.is_empty()) else { return };
    let current = std::env::var_os("PATH").unwrap_or_default();
    let dirs = std::env::split_paths(&current).chain(std::env::split_paths(tools));
    if let Ok(path) = std::env::join_paths(dirs) {
        // SAFETY: called first thing in `main`, before any thread exists that could read the
        // environment concurrently.
        unsafe { std::env::set_var("PATH", path) };
    }
}

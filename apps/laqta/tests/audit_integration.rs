//! Integration tests. Each test runs the real `laqta` binary as a *secondary* instance
//! against a fake primary bound at `$XDG_RUNTIME_DIR/laqta.sock`, and records what it forwards. The
//! secondary path returns before any window, tray or hotkey is created. As a guard, the child gets no
//! display, no session bus, `LAQTA_NO_GPUI=1` and a scratch HOME, and is killed after a timeout.
// The fake primary is a Unix socket in a scratch directory; on Windows the IPC uses named pipes.
#![cfg(unix)]
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use laqta_domain::Action;
use laqta_ipc::{Claim, Request};

struct Fake {
    dir: PathBuf,
    seen: Arc<Mutex<Vec<Request>>>,
}

fn fake(name: &str) -> Fake {
    let dir = std::env::temp_dir().join(format!("laqta-audit-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let sock = dir.join("laqta.sock");
    let Claim::Primary(server) = laqta_ipc::claim_at(sock.to_str().unwrap()).unwrap() else { panic!("fake primary not bound") };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    server.serve_requests(move |r| {
        s.lock().unwrap().push(r);
        Ok(())
    });
    Fake { dir, seen }
}

/// Runs `laqta <args>` in `cwd` as a secondary of `f`. Returns the exit code.
fn run(f: &Fake, cwd: &Path, args: &[&str]) -> Option<i32> {
    let mut child = Command::new(env!("CARGO_BIN_EXE_laqta"))
        .args(args)
        .current_dir(cwd)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .env("XDG_RUNTIME_DIR", &f.dir)
        .env("HOME", &f.dir)
        .env("XDG_CONFIG_HOME", f.dir.join("config"))
        .env("XDG_DATA_HOME", f.dir.join("data"))
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .env("LAQTA_NO_GPUI", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            // Let the fake's thread record the last request.
            std::thread::sleep(Duration::from_millis(100));
            return status.code();
        }
        if start.elapsed() > Duration::from_secs(15) {
            let _ = child.kill();
            panic!("laqta did not exit: it may have become a primary instance");
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn seen(f: &Fake) -> Vec<Request> {
    f.seen.lock().unwrap().clone()
}

/// ShareX: `-ImageEditor "file"` opens that file in the editor (CLIManager gives the next non-dash
/// argument to the command as its parameter; `CheckCLIHotkey` → `ExecuteJob(job, filePath)`). Its
/// own "Edit with ShareX" shell verb relies on it. Laqta forwards it as one action-with-file request.
#[test]
fn audit_bug_hotkey_flag_file_parameter_is_uploaded() {
    let f = fake("editor");
    let img = f.dir.join("x.png");
    std::fs::write(&img, b"png").unwrap();
    assert_eq!(run(&f, &f.dir, &["-ImageEditor", img.to_str().unwrap()]), Some(0));
    let got = seen(&f);
    assert!(
        !got.contains(&Request::Upload(img.clone())),
        "`-ImageEditor x.png` uploaded x.png instead of opening it in the editor: {got:?}"
    );
}

/// A relative path from a terminal is made absolute before it is forwarded, so the primary's
/// working directory does not matter.
#[test]
fn audit_bug_relative_path_forwarded_unresolved() {
    let f = fake("relative");
    let sub = f.dir.join("sub");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("shot.png"), b"png").unwrap();
    assert_eq!(run(&f, &sub, &["shot.png"]), Some(0));
    let got = seen(&f);
    assert_eq!(got, vec![Request::Upload(sub.join("shot.png"))], "relative path not made absolute before forwarding");
}

/// `-CustomUploader x.sxcu` / `-ImageEffect y.sxie` while Laqta runs are forwarded to it as
/// imports.
#[test]
fn audit_bug_import_flags_dropped_by_secondary() {
    let f = fake("imports");
    let sxcu = f.dir.join("x.sxcu");
    std::fs::write(&sxcu, br#"{"Name":"x","RequestURL":"https://example.invalid"}"#).unwrap();
    assert_eq!(run(&f, &f.dir, &["-CustomUploader", sxcu.to_str().unwrap()]), Some(0));
    let got = seen(&f);
    assert!(!got.is_empty(), "the -CustomUploader import never reached the running instance");
}

/// ShareX uploads a bare URL argument with `DownloadAndUploadFile`; so does Laqta.
#[test]
fn audit_bug_bare_url_is_treated_as_a_path() {
    let f = fake("url");
    let url = "https://example.invalid/a.png";
    assert_eq!(run(&f, &f.dir, &[url]), Some(0));
    let got = seen(&f);
    assert_eq!(got, vec![Request::UploadUrl(url.into())], "a URL argument became {got:?}");
}

/// ShareX's short aliases (`-s`, `-m`) and two `HotkeyType` names (`BackgroundRemover`,
/// `AnimatedGifTrimmer`) are accepted.
#[test]
fn audit_bug_sharex_switches_rejected() {
    let f = fake("aliases");
    let mut rejected = Vec::new();
    // ShareX has no `-p` switch (Program.cs reads only -silent/-s, -multi/-m and -NoHotkeys).
    for flag in ["-s", "-m", "-BackgroundRemover", "-AnimatedGifTrimmer"] {
        if run(&f, &f.dir, &[flag]) == Some(2) {
            rejected.push(flag);
        }
    }
    assert!(rejected.is_empty(), "ShareX switches rejected as unknown: {rejected:?}");
}

/// Control: a HotkeyType flag is forwarded as its action.
#[test]
fn hotkey_flag_is_forwarded() {
    let f = fake("control");
    assert_eq!(run(&f, &f.dir, &["-RectangleRegion"]), Some(0));
    assert_eq!(seen(&f), vec![Request::Action(Action::RectangleRegion)]);
}

/// ShareX's `-task <name>` selects the task settings that the file arguments upload with
/// (`FindCLITask`); it does not run the task. Laqta does the same.
#[test]
fn audit_bug_task_runs_the_workflow_instead_of_selecting_settings() {
    let f = fake("task");
    let img = f.dir.join("y.png");
    std::fs::write(&img, b"png").unwrap();
    assert_eq!(run(&f, &f.dir, &["-task", "To FTP", img.to_str().unwrap()]), Some(0));
    let got = seen(&f);
    assert!(!got.contains(&Request::Workflow("To FTP".into())), "-task ran the workflow: {got:?}");
}

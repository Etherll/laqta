//! Integration tests run the real `laqta-nmh` binary against a fake running Laqta bound at
//! `$XDG_RUNTIME_DIR/laqta.sock`. Messages use ShareX's `NativeMessagingAction` numbers
//! (None=0, UploadImage=1, UploadVideo=2, UploadAudio=3, UploadText=4, ShortenURL=5).
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use laqta_ipc::{Claim, Request};

fn exchange(name: &str, json: &str) -> (Vec<Request>, String) {
    let dir: PathBuf = std::env::temp_dir().join(format!("laqta-audit-nmh-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let Claim::Primary(server) = laqta_ipc::claim_at(dir.join("laqta.sock").to_str().unwrap()).unwrap() else { panic!("not bound") };
    let seen = Arc::new(Mutex::new(Vec::new()));
    let s = seen.clone();
    server.serve_requests(move |r| {
        s.lock().unwrap().push(r);
        Ok(())
    });
    let mut child = Command::new(env!("CARGO_BIN_EXE_laqta-nmh"))
        .arg("chrome-extension://nlkoigbdolhchiicbonbihbphgamnaoc/")
        .env_clear()
        .env("XDG_RUNTIME_DIR", &dir)
        .env("HOME", &dir)
        // Guards in case the fake is not found and the host starts `laqta -silent`.
        .env("LAQTA_NO_GPUI", "1")
        .env("DBUS_SESSION_BUS_ADDRESS", "unix:path=/nonexistent")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(&(json.len() as u32).to_le_bytes()).unwrap();
    stdin.write_all(json.as_bytes()).unwrap();
    drop(stdin);
    let out = child.wait_with_output().unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let reply = String::from_utf8_lossy(out.stdout.get(4..).unwrap_or_default()).into_owned();
    let got = seen.lock().unwrap().clone();
    (got, reply)
}

#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_nmh_upload_text_action_4() {
    let (got, reply) = exchange("text", r#"{"Action":4,"URL":"","Text":"hello"}"#);
    assert_eq!(got, vec![Request::UploadText("hello".into())], "reply: {reply}");
}

#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_nmh_shorten_url_action_5() {
    let (got, reply) = exchange("shorten", r#"{"Action":5,"URL":"https://example.invalid/","Text":""}"#);
    assert_eq!(got, vec![Request::ShortenUrl("https://example.invalid/".into())], "reply: {reply}");
}

#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_nmh_upload_audio_action_3_is_shortened() {
    let (got, reply) = exchange("audio", r#"{"Action":3,"URL":"https://example.invalid/a.mp3","Text":""}"#);
    assert_eq!(got, vec![Request::UploadUrl("https://example.invalid/a.mp3".into())], "reply: {reply}");
}

#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_nmh_upload_video_action_2() {
    let (got, reply) = exchange("video", r#"{"Action":2,"URL":"https://example.invalid/v.mp4","Text":""}"#);
    assert_eq!(got, vec![Request::UploadUrl("https://example.invalid/v.mp4".into())], "reply: {reply}");
}

/// Selected text longer than the IPC line limit (4096 bytes) cannot be forwarded. Uses Laqta's own
/// number for text (2) so only the size is under test.
#[test]
#[ignore = "known issue, not fixed yet"]
fn audit_bug_nmh_long_text_exceeds_ipc_line_limit() {
    let text = "a".repeat(5000);
    let (got, reply) = exchange("long", &format!(r#"{{"Action":2,"URL":"","Text":"{text}"}}"#));
    assert_eq!(got.len(), 1, "a 5000-byte text upload did not reach Laqta; reply: {reply}");
}

/// Control: action 1 (UploadImage) with a URL.
#[test]
fn nmh_upload_image_action_1() {
    let (got, _) = exchange("image", r#"{"Action":1,"URL":"https://example.invalid/i.png","Text":""}"#);
    assert_eq!(got, vec![Request::UploadUrl("https://example.invalid/i.png".into())]);
}

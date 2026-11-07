//! Integration tests for laqta-http. Every request goes to a server on 127.0.0.1 that the test
//! starts; nothing leaves the machine.

mod support;

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::Duration;

use futures_lite::future::block_on;
use laqta_http::{
    Body, HttpError, Method, Part, Request, fetch_verified, loopback_once, run, runtime_alive,
    send, send_with_progress, set_release_after_for_tests,
};
use sha2::{Digest, Sha256};
use support::{Canned, Server};

/// The runtime is shared by the whole process, and the release test asserts on it, so the tests
/// take turns.
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

type Reports = Arc<Mutex<Vec<(u64, Option<u64>)>>>;
type ProgressFn = Arc<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// A progress callback that records every report.
fn recorder() -> (Reports, ProgressFn) {
    let reports: Reports = Arc::default();
    let sink = Arc::clone(&reports);
    let callback = Arc::new(move |sent: u64, total: Option<u64>| {
        sink.lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push((sent, total));
    });
    (reports, callback)
}

fn no_progress() -> ProgressFn {
    Arc::new(|_, _| {})
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn sha256_hex(data: &[u8]) -> String {
    hex(Sha256::digest(data).as_slice())
}

fn part_file(dest: &std::path::Path) -> std::path::PathBuf {
    let mut name = dest.as_os_str().to_owned();
    name.push(".part");
    name.into()
}

#[test]
fn get_sends_query_and_headers() {
    let _serial = serial();
    let server = Server::start(vec![
        Canned::ok("hello").header("Content-Type", "text/plain"),
    ]);
    let mut request = Request::new(Method::Get, server.url("/items"));
    request.query = vec![("q".into(), "hello".into()), ("n".into(), "2".into())];
    request.headers = vec![("X-Token".into(), "abc".into())];

    let response = block_on(send(request)).expect("GET succeeds");
    let seen = server.finish();

    assert_eq!(response.status, 200);
    assert_eq!(response.text(), "hello");
    assert_eq!(response.header("content-type"), Some("text/plain"));
    assert!(
        response.url.ends_with("/items?q=hello&n=2"),
        "final url {}",
        response.url
    );
    assert_eq!(seen[0].method, "GET");
    assert_eq!(seen[0].target, "/items?q=hello&n=2");
    assert_eq!(seen[0].header("x-token"), Some("abc"));
    assert!(
        seen[0]
            .header("user-agent")
            .is_some_and(|agent| agent.starts_with("Laqta/"))
    );
}

#[test]
fn post_form_encodes_pairs() {
    let _serial = serial();
    let server = Server::start(vec![Canned::ok("ok")]);
    let mut request = Request::new(Method::Post, server.url("/form"));
    request.body = Body::Form(vec![
        ("name".into(), "Laqta".into()),
        ("kind".into(), "a b&c".into()),
    ]);

    let response = block_on(send(request)).expect("POST succeeds");
    let seen = server.finish();

    assert_eq!(response.text(), "ok");
    assert_eq!(seen[0].method, "POST");
    assert_eq!(
        seen[0].header("content-type"),
        Some("application/x-www-form-urlencoded")
    );
    assert_eq!(
        String::from_utf8_lossy(&seen[0].body),
        "name=Laqta&kind=a+b%26c"
    );
}

#[test]
fn post_multipart_sends_parts_and_boundary() {
    let _serial = serial();
    let server = Server::start(vec![Canned::ok("ok")]);
    let mut request = Request::new(Method::Post, server.url("/upload"));
    request.body = Body::Multipart(vec![
        Part::Text {
            name: "title".into(),
            value: "shot".into(),
        },
        Part::File {
            name: "file".into(),
            file_name: "a.png".into(),
            mime: "image/png".into(),
            data: vec![0, 1, 2, 255],
        },
    ]);

    block_on(send(request)).expect("multipart POST succeeds");
    let seen = server.finish();

    let content_type = seen[0].header("content-type").expect("content type");
    assert!(
        content_type.starts_with("multipart/form-data; boundary="),
        "{content_type}"
    );
    let boundary = content_type
        .split("boundary=")
        .nth(1)
        .expect("boundary value");
    let body = &seen[0].body;
    assert!(contains(body, format!("--{boundary}").as_bytes()));
    assert!(contains(body, b"name=\"title\""));
    assert!(contains(body, b"shot"));
    assert!(contains(body, b"name=\"file\"; filename=\"a.png\""));
    assert!(contains(body, &[0, 1, 2, 255]));
}

#[test]
fn post_bytes_reports_progress_to_the_total() {
    let _serial = serial();
    let server = Server::start(vec![Canned::ok("ok")]);
    let data = vec![7u8; 300_000];
    let mut request = Request::new(Method::Post, server.url("/blob"));
    request.body = Body::Bytes {
        data: data.clone(),
        content_type: "application/octet-stream".into(),
    };
    let (reports, progress) = recorder();

    let response = block_on(send_with_progress(request, progress)).expect("upload succeeds");
    let seen = server.finish();

    assert_eq!(response.status, 200);
    assert_eq!(seen[0].method, "POST");
    assert_eq!(seen[0].body, data);
    assert_eq!(seen[0].header("content-length"), Some("300000"));
    let reports = reports
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    assert!(
        reports.len() > 1,
        "expected several reports, got {reports:?}"
    );
    assert!(
        reports.windows(2).all(|pair| pair[0].0 <= pair[1].0),
        "{reports:?}"
    );
    assert_eq!(reports.last(), Some(&(300_000, Some(300_000))));
}

#[test]
fn non_success_status_is_a_response() {
    let _serial = serial();
    let server = Server::start(vec![Canned::new(500, "boom")]);

    let response = block_on(send(Request::new(Method::Get, server.url("/fail"))))
        .expect("a 500 is a response");
    server.finish();

    assert_eq!(response.status, 500);
    assert_eq!(response.text(), "boom");
}

#[test]
fn unreachable_port_is_a_network_error() {
    let _serial = serial();
    let port = {
        let listener = TcpListener::bind("127.0.0.1:0").expect("reserve a port");
        listener.local_addr().expect("port").port()
    };

    let err = block_on(send(Request::new(
        Method::Get,
        format!("http://127.0.0.1:{port}/"),
    )))
    .expect_err("nothing listens on that port");

    assert!(matches!(err, HttpError::Network(_)), "{err:?}");
}

#[test]
fn invalid_urls_are_invalid_requests() {
    let _serial = serial();
    for url in ["not a url", "ftp://example.test/file"] {
        let err =
            block_on(send(Request::new(Method::Get, url))).expect_err("rejected before sending");
        assert!(matches!(err, HttpError::Invalid(_)), "{url}: {err:?}");
    }
}

#[test]
fn fetch_verified_writes_the_file_and_removes_the_part() {
    let _serial = serial();
    let body = b"verified model bytes ".repeat(1000);
    let server = Server::start(vec![Canned::ok(body.clone())]);
    let dir = tempfile::tempdir().expect("temp dir");
    let dest = dir.path().join("model.bin");

    let result = block_on(fetch_verified(
        &server.url("/model.bin"),
        &dest,
        &sha256_hex(&body),
        no_progress(),
        Arc::new(AtomicBool::new(false)),
    ));
    server.finish();

    result.expect("the download verifies");
    assert_eq!(std::fs::read(&dest).expect("downloaded file"), body);
    assert!(!part_file(&dest).exists());
}

#[test]
fn fetch_verified_rejects_a_checksum_mismatch() {
    let _serial = serial();
    let body = b"tampered bytes".to_vec();
    let server = Server::start(vec![Canned::ok(body.clone())]);
    let dir = tempfile::tempdir().expect("temp dir");
    let dest = dir.path().join("model.bin");
    let expected = "0".repeat(64);

    let err = block_on(fetch_verified(
        &server.url("/model.bin"),
        &dest,
        &expected,
        no_progress(),
        Arc::new(AtomicBool::new(false)),
    ))
    .expect_err("the digest does not match");
    server.finish();

    assert_eq!(
        err,
        HttpError::Checksum {
            expected,
            actual: sha256_hex(&body)
        }
    );
    assert!(!dest.exists());
    assert!(!part_file(&dest).exists());
}

#[test]
fn fetch_verified_stops_when_cancelled() {
    let _serial = serial();
    let body = vec![1u8; 512 * 1024];
    let server = Server::start(vec![Canned::ok(body.clone())]);
    let dir = tempfile::tempdir().expect("temp dir");
    let dest = dir.path().join("model.bin");
    let cancel = Arc::new(AtomicBool::new(false));
    // Cancelling from the first report makes the outcome independent of how reqwest chunks the body.
    let flag = Arc::clone(&cancel);
    let progress: ProgressFn = Arc::new(move |_, _| flag.store(true, Ordering::Relaxed));

    let err = block_on(fetch_verified(
        &server.url("/model.bin"),
        &dest,
        &sha256_hex(&body),
        progress,
        Arc::clone(&cancel),
    ))
    .expect_err("the download is cancelled");
    let _ = server.finish();

    assert_eq!(err, HttpError::Cancelled);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(!dest.exists());
    assert!(!part_file(&dest).exists());
}

#[test]
fn run_computes_its_output_on_the_runtime_thread() {
    let _serial = serial();
    // The future must be Send and 'static so SFTP and the loopback listener can hand it over.
    fn assert_send_static<T: Send + 'static>(_: &T) {}
    let future = run(async { std::thread::current().name().map(str::to_owned) });
    assert_send_static(&future);

    let thread_name = block_on(future);

    assert_eq!(thread_name.as_deref(), Some("laqta-http"));
}

#[test]
fn loopback_once_receives_the_redirect() {
    let _serial = serial();
    let (port, redirect) =
        block_on(loopback_once("<p>done</p>".to_owned())).expect("loopback listener starts");
    let browser = std::thread::spawn(move || {
        let mut stream = TcpStream::connect(("127.0.0.1", port)).expect("connect to the listener");
        stream
            .write_all(b"GET /cb?code=abc HTTP/1.1\r\nHost: 127.0.0.1\r\n\r\n")
            .expect("send the redirect");
        let mut reply = String::new();
        stream.read_to_string(&mut reply).expect("read the reply");
        reply
    });

    let target = block_on(redirect).expect("the redirect is received");
    let reply = browser.join().expect("browser thread");

    assert_eq!(target, "/cb?code=abc");
    assert!(reply.starts_with("HTTP/1.1 200 OK"), "{reply}");
    assert!(reply.ends_with("<p>done</p>"), "{reply}");
}

#[test]
fn runtime_is_released_when_idle_and_restarts_on_demand() {
    let _serial = serial();
    set_release_after_for_tests(Duration::from_millis(200));
    let server = Server::start(vec![Canned::ok("one"), Canned::ok("two")]);
    let url = server.url("/");

    let first = block_on(send(Request::new(Method::Get, url.clone()))).expect("first request");
    assert_eq!(first.text(), "one");
    std::thread::sleep(Duration::from_millis(600));
    let released = !runtime_alive();

    let second = block_on(send(Request::new(Method::Get, url)));
    server.finish();
    set_release_after_for_tests(Duration::from_secs(60));

    assert!(
        released,
        "the runtime should stop after 200 ms without a job"
    );
    assert_eq!(second.expect("request after release").text(), "two");
}

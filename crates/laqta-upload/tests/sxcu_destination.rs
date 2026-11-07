//! The `.sxcu` Destination and Shortener against a local test server: the request each bundled host
//! sends (method, form part or body, query, headers), the URL read from the reply, and the failure
//! paths. Nothing leaves 127.0.0.1.
#![allow(clippy::unwrap_used)]

mod support;

use std::collections::VecDeque;
use std::net::TcpListener;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use futures_lite::future::block_on;
use laqta_ports::fakes::{CountingRng, FakePrompt};
use laqta_ports::{
    BoxFuture, Destination, PortError, ProgressCallback, Prompt, Shortener, UploadData, UploadError, UploadKind, UploadRequest,
    UploadResult,
};
use laqta_upload::catalog::catalog;
use laqta_upload::sxcu::CustomUploader;
use laqta_upload::sxcu_destination::{SxcuDestination, SxcuShortener};
use support::{Canned, Server};

/// The bundled entry `stem`, with its request URL pointed at `server` at `path`.
fn uploader_at(stem: &str, server: &Server, path: &str) -> CustomUploader {
    let mut uploader = catalog_entry(stem);
    uploader.request_url = server.url(path);
    uploader
}

fn catalog_entry(stem: &str) -> CustomUploader {
    let id = format!("sxcu:{stem}");
    let (_, uploader) = catalog().into_iter().find(|(known, _)| *known == id).unwrap_or_else(|| panic!("no catalog entry {id}"));
    uploader
}

fn destination_for(stem: &str, uploader: CustomUploader, prompt: Arc<dyn Prompt>) -> SxcuDestination {
    SxcuDestination::new(format!("sxcu:{stem}"), uploader, prompt, Arc::new(CountingRng::default()))
}

fn fake_prompt() -> Arc<dyn Prompt> {
    Arc::new(FakePrompt::default())
}

fn upload(destination: &dyn Destination, request: UploadRequest) -> Result<UploadResult, UploadError> {
    let progress: ProgressCallback = Arc::new(|_: u64, _: Option<u64>| {});
    block_on(destination.upload(request, progress))
}

fn image(data: &[u8]) -> UploadRequest {
    UploadRequest {
        file_name: "shot.png".into(),
        kind: UploadKind::Image,
        data: UploadData::Bytes(data.to_vec()),
        mime: "image/png".into(),
    }
}

fn text(body: &str) -> UploadRequest {
    UploadRequest {
        file_name: "paste.txt".into(),
        kind: UploadKind::Text,
        data: UploadData::Bytes(body.as_bytes().to_vec()),
        mime: "text/plain".into(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

/// The fields of a form-encoded body or query, decoded.
fn fields(bytes: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(bytes).into_owned().collect()
}

fn has_field(form: &[(String, String)], key: &str, value: &str) -> bool {
    form.iter().any(|(k, v)| k == key && v == value)
}

/// Answers text questions from a script, then with `answer`, and counts them. A scripted `Err` is a
/// question the user cancelled.
struct Asker {
    script: Mutex<VecDeque<Result<String, PortError>>>,
    answer: String,
    asked: AtomicUsize,
}

impl Asker {
    fn answering(answer: &str) -> Self {
        Asker { script: Mutex::new(VecDeque::new()), answer: answer.to_string(), asked: AtomicUsize::new(0) }
    }

    fn asked(&self) -> usize {
        self.asked.load(Ordering::SeqCst)
    }
}

impl Prompt for Asker {
    fn ask_text(&self, _title: &str, _default: &str) -> BoxFuture<'_, Result<String, PortError>> {
        self.asked.fetch_add(1, Ordering::SeqCst);
        let scripted = self.script.lock().unwrap().pop_front();
        let answer = scripted.unwrap_or_else(|| Ok(self.answer.clone()));
        Box::pin(async move { answer })
    }

    fn choose(&self, _title: &str, _options: &[String]) -> BoxFuture<'_, Result<usize, PortError>> {
        Box::pin(async { Ok(0) })
    }

    fn show_text(&self, _title: &str, _text: &str) -> BoxFuture<'_, Result<(), PortError>> {
        Box::pin(async { Ok(()) })
    }

    fn confirm(&self, _title: &str, _text: &str) -> BoxFuture<'_, Result<bool, PortError>> {
        Box::pin(async { Ok(true) })
    }
}

#[test]
fn vgy_me_sends_the_file_as_multipart_and_reads_the_image_url() {
    let server = Server::start(vec![Canned::ok(r#"{"image":"https://vgy.me/abc.png"}"#)]);
    let destination = destination_for("vgy-me", uploader_at("vgy-me", &server, "/upload"), fake_prompt());

    let result = upload(&destination, image(b"PNG")).unwrap();
    let seen = server.finish();

    assert_eq!(result.url.as_deref(), Some("https://vgy.me/abc.png"));
    assert_eq!(result.response, r#"{"image":"https://vgy.me/abc.png"}"#);
    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/upload");
    assert!(request.header("content-type").unwrap().starts_with("multipart/form-data"));
    assert!(contains(&request.body, b"name=\"file\"; filename=\"shot.png\""));
    assert!(contains(&request.body, b"PNG"));
}

#[test]
fn pomf_clone_uses_the_files_array_name_and_reads_files_0_url() {
    let reply = r#"{"success":true,"files":[{"url":"https://a.pomf.cat/x.png"}]}"#;
    let server = Server::start(vec![Canned::ok(reply)]);
    let destination = destination_for("pomf", uploader_at("pomf", &server, "/upload.php"), fake_prompt());

    let result = upload(&destination, image(b"PNG")).unwrap();
    let seen = server.finish();

    assert_eq!(result.url.as_deref(), Some("https://a.pomf.cat/x.png"));
    assert!(contains(&seen[0].body, b"name=\"files[]\"; filename=\"shot.png\""));
}

#[test]
fn pastebin_posts_a_form_and_reads_the_plain_url() {
    let server = Server::start(vec![Canned::ok("https://pastebin.com/AbC123\n")]);
    let destination = destination_for("pastebin", uploader_at("pastebin", &server, "/api/api_post.php"), fake_prompt());

    let result = upload(&destination, text("hello & welcome")).unwrap();
    let seen = server.finish();

    let request = &seen[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.header("content-type"), Some("application/x-www-form-urlencoded"));
    let form = fields(&request.body);
    assert!(has_field(&form, "api_option", "paste"), "{form:?}");
    assert!(has_field(&form, "api_paste_code", "hello & welcome"), "{form:?}");
    assert_eq!(result.url.as_deref(), Some("https://pastebin.com/AbC123"));
}

#[test]
fn pastebin_reports_an_error_reply_as_no_url() {
    let server = Server::start(vec![Canned::ok("Bad API request, invalid api_dev_key")]);
    let destination = destination_for("pastebin", uploader_at("pastebin", &server, "/api/api_post.php"), fake_prompt());

    let result = upload(&destination, text("hi")).unwrap();
    server.finish();

    assert_eq!(result.url, None);
    assert_eq!(result.response, "Bad API request, invalid api_dev_key");
}

#[test]
fn paste_ee_sends_the_text_as_escaped_json() {
    let server = Server::start(vec![Canned::ok(r#"{"link":"https://paste.ee/p/xyz"}"#)]);
    let destination = destination_for("paste-ee", uploader_at("paste-ee", &server, "/v1/pastes"), fake_prompt());
    let body = "say \"hi\"\nline two {braces} \\ back";

    let result = upload(&destination, text(body)).unwrap();
    let seen = server.finish();

    assert_eq!(seen[0].header("content-type"), Some("application/json"));
    let sent: serde_json::Value = serde_json::from_slice(&seen[0].body).unwrap();
    assert_eq!(sent["sections"][0]["contents"], body);
    assert_eq!(result.url.as_deref(), Some("https://paste.ee/p/xyz"));
}

#[test]
fn hastebin_asks_once_and_sends_the_text_as_the_body() {
    let server = Server::start(vec![Canned::ok(r#"{"key":"abc"}"#), Canned::ok(r#"{"key":"def"}"#)]);
    let base = server.url("");
    let asker = Arc::new(Asker::answering(&base));
    let destination = SxcuDestination::new("sxcu:hastebin", catalog_entry("hastebin"), asker.clone(), Arc::new(CountingRng::default()));

    let first = upload(&destination, text("one")).unwrap();
    let second = upload(&destination, text("two")).unwrap();
    let seen = server.finish();

    // The URL and the response template both name `Hastebin URL`; the answer is remembered.
    assert_eq!(asker.asked(), 1);
    assert_eq!(first.url, Some(format!("{base}/abc")));
    assert_eq!(second.url, Some(format!("{base}/def")));
    assert_eq!(seen[0].target, "/documents");
    assert_eq!(seen[0].body, b"one".to_vec());
    assert_eq!(seen[0].header("content-type"), Some("text/plain"));
    assert_eq!(seen[1].body, b"two".to_vec());
}

#[test]
fn a_cancelled_question_cancels_the_upload_and_is_asked_again() {
    let server = Server::start(vec![Canned::ok(r#"{"key":"abc"}"#)]);
    let base = server.url("");
    let asker = Arc::new(Asker {
        script: Mutex::new(VecDeque::from([Err(PortError::Cancelled)])),
        answer: base.clone(),
        asked: AtomicUsize::new(0),
    });
    let destination = SxcuDestination::new("sxcu:hastebin", catalog_entry("hastebin"), asker.clone(), Arc::new(CountingRng::default()));

    let cancelled = upload(&destination, text("one"));
    let answered = upload(&destination, text("two")).unwrap();
    let seen = server.finish();

    assert_eq!(cancelled, Err(UploadError::Cancelled));
    assert_eq!(asker.asked(), 2, "the cancelled question was not remembered");
    assert_eq!(answered.url, Some(format!("{base}/abc")));
    assert_eq!(seen.len(), 1, "nothing was sent for the cancelled upload");
}

#[test]
fn is_gd_shortens_through_query_parameters_and_reads_the_short_url() {
    let server = Server::start(vec![Canned::ok("https://is.gd/abc12\n")]);
    let shortener =
        SxcuShortener::new("sxcu:is-gd", uploader_at("is-gd", &server, "/create.php"), fake_prompt(), Arc::new(CountingRng::default()));

    let short = block_on(shortener.shorten("https://example.com/a?b=c&d=e f")).unwrap();
    let seen = server.finish();

    assert_eq!(short, "https://is.gd/abc12");
    let request = &seen[0];
    assert_eq!(request.method, "GET");
    let (path, query) = request.target.split_once('?').unwrap();
    assert_eq!(path, "/create.php");
    assert_eq!(
        fields(query.as_bytes()),
        vec![("format".to_string(), "simple".to_string()), ("url".to_string(), "https://example.com/a?b=c&d=e f".to_string()),]
    );
}

#[test]
fn a_shortener_reply_without_a_short_url_is_a_bad_response() {
    let server = Server::start(vec![Canned::ok("Error: invalid URL")]);
    let shortener =
        SxcuShortener::new("sxcu:is-gd", uploader_at("is-gd", &server, "/create.php"), fake_prompt(), Arc::new(CountingRng::default()));

    let result = block_on(shortener.shorten("https://example.com/"));
    server.finish();

    assert!(matches!(result, Err(UploadError::BadResponse(_))), "{result:?}");
}

#[test]
fn a_500_is_a_server_error_with_the_reply_as_its_body() {
    let server = Server::start(vec![Canned::new(500, "boom")]);
    let destination = destination_for("vgy-me", uploader_at("vgy-me", &server, "/upload"), fake_prompt());

    let error = upload(&destination, image(b"PNG")).unwrap_err();
    server.finish();

    assert_eq!(error, UploadError::Server { status: 500, body: "boom".to_string() });
    assert!(error.is_retryable());
}

#[test]
fn a_refused_connection_is_a_network_error() {
    let port = TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port();
    let mut uploader = catalog_entry("vgy-me");
    uploader.request_url = format!("http://127.0.0.1:{port}/upload");
    let destination = destination_for("vgy-me", uploader, fake_prompt());

    let error = upload(&destination, image(b"PNG")).unwrap_err();

    assert!(matches!(error, UploadError::Network(_)), "{error:?}");
}

#[test]
fn a_file_on_disk_is_read_and_sent() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, b"DISKDATA").unwrap();
    let server = Server::start(vec![Canned::ok(r#"{"image":"https://vgy.me/d.png"}"#)]);
    let destination = destination_for("vgy-me", uploader_at("vgy-me", &server, "/upload"), fake_prompt());
    let request =
        UploadRequest { file_name: "shot.png".into(), kind: UploadKind::Image, data: UploadData::Path(path), mime: "image/png".into() };

    upload(&destination, request).unwrap();
    let seen = server.finish();

    assert!(contains(&seen[0].body, b"DISKDATA"));
}

#[test]
fn a_missing_file_is_a_bad_response_and_nothing_is_sent() {
    let destination = destination_for("vgy-me", catalog_entry("vgy-me"), fake_prompt());
    let request = UploadRequest {
        file_name: "gone.png".into(),
        kind: UploadKind::Image,
        data: UploadData::Path("/nonexistent/laqta-test/gone.png".into()),
        mime: "image/png".into(),
    };

    let error = upload(&destination, request).unwrap_err();

    assert!(matches!(error, UploadError::BadResponse(_)), "{error:?}");
}

#[test]
fn puush_reads_the_url_from_its_comma_separated_reply() {
    let server = Server::start(vec![Canned::ok("0,https://puush.me/i/abc.png,1,2"), Canned::ok("-1")]);
    let destination = destination_for("puush", uploader_at("puush", &server, "/api/up"), fake_prompt());

    let first = upload(&destination, image(b"PNG")).unwrap();
    let second = upload(&destination, image(b"PNG")).unwrap();
    let seen = server.finish();

    assert_eq!(first.url.as_deref(), Some("https://puush.me/i/abc.png"));
    assert_eq!(second.url, None);
    assert_eq!(second.response, "-1");
    assert!(contains(&seen[0].body, b"name=\"k\""));
    assert!(contains(&seen[0].body, b"name=\"f\"; filename=\"shot.png\""));
}

#[test]
fn a_result_keeps_the_raw_reply() {
    let server = Server::start(vec![Canned::ok(r#"{"image":"https://vgy.me/q.png"}"#).header("X-Trace", "1")]);
    let destination = destination_for("vgy-me", uploader_at("vgy-me", &server, "/upload"), fake_prompt());

    let result: UploadResult = upload(&destination, image(b"PNG")).unwrap();
    server.finish();

    assert_eq!(result.response, r#"{"image":"https://vgy.me/q.png"}"#);
    assert_eq!(result.thumbnail_url, None);
    assert_eq!(result.deletion_url, None);
}

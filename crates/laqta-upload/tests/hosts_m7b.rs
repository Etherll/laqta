//! The Flickr, Box, YouTube, Google Cloud Storage, Azure Blob and Backblaze B2 hosts against
//! local servers with canned replies: request shapes, signatures and parsed results. No network.
#![allow(clippy::unwrap_used)]

mod support_oauth;

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_lite::future::block_on;
use laqta_ports::fakes::{CountingRng, FakeSecretStore, FakeShell};
use laqta_ports::{Destination, PortError, ProgressCallback, SecretStore, Shell, UploadData, UploadError, UploadKind, UploadRequest};
use laqta_upload::hosts::{azure_blob, b2, box_com, flickr, gcs, s3, youtube};
use laqta_upload::oauth::OAuth;
use percent_encoding::percent_decode_str;
use serde_json::{Value, json};
use sha1::{Digest, Sha1};
use support_oauth::sign_in::{NOW, config, make_oauth, stored};
use support_oauth::{Canned, Recorded, Server};

fn store(entries: &[(&str, &str)]) -> Arc<dyn SecretStore> {
    let secrets = FakeSecretStore::default();
    for (key, value) in entries {
        secrets.set(key, value).unwrap();
    }
    Arc::new(secrets)
}

fn progress() -> ProgressCallback {
    Arc::new(|_, _| {})
}

fn now() -> Arc<dyn Fn() -> i64 + Send + Sync> {
    Arc::new(|| NOW)
}

fn png(name: &str, bytes: &[u8]) -> UploadRequest {
    UploadRequest {
        file_name: name.to_owned(),
        kind: UploadKind::Image,
        data: UploadData::Bytes(bytes.to_vec()),
        mime: "image/png".to_owned(),
    }
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// A local address with nothing listening on it: connecting is refused.
fn closed_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    format!("http://{}", listener.local_addr().unwrap())
}

/// A sign-in whose store already holds `token` under `key`. It never refreshes: the token does not
/// expire, and the token endpoint is unreachable.
fn signed_in(key: &str, token: &str) -> Arc<OAuth> {
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(key, &stored(token, None, None)).unwrap();
    make_oauth(config(closed_url()), secrets, Arc::new(FakeShell::default()), key)
}

fn query_pairs_of(target: &str) -> Vec<(String, String)> {
    url::Url::parse(&format!("http://localhost{target}"))
        .unwrap()
        .query_pairs()
        .map(|(name, value)| (name.into_owned(), value.into_owned()))
        .collect()
}

/// Checks an OAuth 1.0a Authorization header against RFC 5849: the signature must be the one the
/// request's method, URL, oauth parameters and covered (query or form) parameters give. Returns the
/// oauth parameters, decoded.
fn check_oauth(
    request: &Recorded,
    url: &str,
    covered: &[(String, String)],
    consumer_secret: &str,
    token_secret: &str,
) -> Vec<(String, String)> {
    let header = request.header("authorization").unwrap();
    let fields = header.strip_prefix("OAuth ").unwrap();
    let mut params: Vec<(String, String)> = fields
        .split(", ")
        .map(|field| {
            let (name, value) = field.split_once('=').unwrap();
            let name = percent_decode_str(name).decode_utf8_lossy().into_owned();
            let value = percent_decode_str(value.trim_matches('"')).decode_utf8_lossy().into_owned();
            (name, value)
        })
        .collect();
    let position = params.iter().position(|(name, _)| name == "oauth_signature").unwrap();
    let (_, signature) = params.remove(position);
    let mut all = params.clone();
    all.extend(covered.iter().cloned());
    let base = laqta_upload::hosts::flickr::base_string(&request.method, url, &all);
    assert_eq!(laqta_upload::hosts::flickr::signature(&base, consumer_secret, token_secret), signature, "base string: {base}");
    params
}

fn value_of<'a>(params: &'a [(String, String)], name: &str) -> Option<&'a str> {
    params.iter().find(|(key, _)| key == name).map(|(_, value)| value.as_str())
}

/// A server that answers `replies` in order, like `support_oauth::Server`, and also hands each
/// request to the test as it arrives. The browser stand-in needs the request before the reply.
fn recording_server(replies: Vec<Canned>) -> (String, Receiver<Recorded>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        for reply in replies {
            let Ok((mut stream, _)) = listener.accept() else {
                return;
            };
            let recorded = read_request(&mut stream);
            let _ = sender.send(recorded);
            let mut head = format!("HTTP/1.1 {} OK\r\n", reply.status);
            for (name, value) in &reply.headers {
                head.push_str(&format!("{name}: {value}\r\n"));
            }
            head.push_str(&format!("Content-Length: {}\r\nConnection: close\r\n\r\n", reply.body.len()));
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&reply.body);
        }
    });
    (base, receiver)
}

fn read_request(stream: &mut TcpStream) -> Recorded {
    let mut reader = BufReader::new(stream.try_clone().unwrap());
    let mut line = String::new();
    reader.read_line(&mut line).unwrap();
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or_default().to_owned();
    let target = parts.next().unwrap_or_default().to_owned();
    let mut headers = Vec::new();
    let mut length = 0usize;
    loop {
        let mut field = String::new();
        reader.read_line(&mut field).unwrap();
        let field = field.trim_end();
        if field.is_empty() {
            break;
        }
        if let Some((name, value)) = field.split_once(':') {
            let (name, value) = (name.trim().to_ascii_lowercase(), value.trim().to_owned());
            if name == "content-length" {
                length = value.parse().unwrap();
            }
            headers.push((name, value));
        }
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body).unwrap();
    Recorded { method, target, headers, body }
}

/// Plays the browser for the Flickr sign-in. When the sign-in URL opens, it reads the request token
/// request (the first request the recording server got), and answers the loopback redirect on the
/// callback that request named.
struct Browser {
    requests: Mutex<Receiver<Recorded>>,
    opened: Mutex<Vec<String>>,
}

impl Shell for Browser {
    fn open_path(&self, _path: &Path) -> Result<(), PortError> {
        Ok(())
    }

    fn open_url(&self, url: &str) -> Result<(), PortError> {
        self.opened.lock().unwrap().push(url.to_owned());
        let token_request = self.requests.lock().unwrap().recv().unwrap();
        let header = token_request.header("authorization").unwrap().to_owned();
        let start = header.find("oauth_callback=\"").unwrap() + "oauth_callback=\"".len();
        let end = header[start..].find('"').unwrap() + start;
        let callback = percent_decode_str(&header[start..end]).decode_utf8_lossy().into_owned();
        let token =
            query_pairs_of(&format!("?{}", url.split_once('?').unwrap().1)).into_iter().find(|(name, _)| name == "oauth_token").unwrap().1;
        thread::spawn(move || {
            let port = url::Url::parse(&callback).unwrap().port().unwrap();
            let mut stream = TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(
                stream,
                "GET /callback?oauth_token={token}&oauth_verifier=ver-1 HTTP/1.1\r\nHost: 127.0.0.1\r\nConnection: close\r\n\r\n"
            )
            .unwrap();
            let mut reply = String::new();
            let _ = stream.read_to_string(&mut reply);
        });
        Ok(())
    }
}

fn flickr_endpoints(base: &str) -> flickr::FlickrEndpoints {
    flickr::FlickrEndpoints {
        request_token: format!("{base}/oauth/request_token"),
        authorize: format!("{base}/oauth/authorize"),
        access_token: format!("{base}/oauth/access_token"),
        upload: format!("{base}/upload/"),
        rest: format!("{base}/rest/"),
    }
}

#[test]
fn flickr_signs_in_through_the_browser_then_uploads_and_reads_the_page_url() {
    let (base, requests) = recording_server(vec![
        Canned::ok("oauth_token=rt-tok&oauth_token_secret=rt-sec&oauth_callback_confirmed=true"),
        Canned::ok("fullname=Laqta&oauth_token=at-tok&oauth_token_secret=at-sec&user_nsid=1%40N01&username=laqta"),
        Canned::ok(r#"<?xml version="1.0" encoding="utf-8"?><rsp stat="ok"><photoid>5550</photoid></rsp>"#),
        Canned::ok(json!({"stat": "ok", "photo": {"urls": {"url": [{"type": "photopage", "_content": "https://www.flickr.com/photos/me/5550/"}]}}}).to_string()),
    ]);
    let browser = Arc::new(Browser { requests: Mutex::new(requests), opened: Mutex::new(Vec::new()) });
    let secrets = store(&[(flickr::API_SECRET_KEY, "api-secret")]);
    let config = flickr::FlickrConfig { api_key: "api-key".to_owned(), public: false, endpoints: flickr_endpoints(&base) };
    let destination = flickr::Flickr::new(config, secrets.clone(), browser.clone(), Arc::new(CountingRng::default()), now());

    let result = block_on(destination.upload(png("a.png", b"PNG-DATA"), progress())).unwrap();

    let opened = browser.opened.lock().unwrap().clone();
    assert_eq!(opened.len(), 1);
    assert!(opened[0].starts_with(&format!("{base}/oauth/authorize?oauth_token=rt-tok")), "{}", opened[0]);
    assert!(opened[0].contains("perms=write"), "{}", opened[0]);

    let requests = browser.requests.lock().unwrap();
    let access = requests.recv().unwrap();
    let upload = requests.recv().unwrap();
    let info = requests.recv().unwrap();
    assert_eq!((access.method.as_str(), access.target.as_str()), ("POST", "/oauth/access_token"));
    let access_params = check_oauth(&access, &format!("{base}/oauth/access_token"), &[], "api-secret", "rt-sec");
    assert_eq!(value_of(&access_params, "oauth_verifier"), Some("ver-1"));
    assert_eq!(value_of(&access_params, "oauth_token"), Some("rt-tok"));

    assert_eq!((upload.method.as_str(), upload.target.as_str()), ("POST", "/upload/"));
    let upload_params = check_oauth(&upload, &format!("{base}/upload/"), &[], "api-secret", "at-sec");
    assert_eq!(value_of(&upload_params, "oauth_token"), Some("at-tok"));
    assert_eq!(value_of(&upload_params, "oauth_signature_method"), Some("HMAC-SHA1"));
    assert!(upload.header("authorization").unwrap().starts_with("OAuth "));
    assert!(contains(&upload.body, b"name=\"is_public\"\r\n\r\n0"));
    assert!(contains(&upload.body, b"name=\"photo\"; filename=\"a.png\""));
    assert!(contains(&upload.body, b"PNG-DATA"));

    assert_eq!((info.method.as_str(), info.target.split('?').next().unwrap()), ("GET", "/rest/"));
    let covered = query_pairs_of(&info.target);
    assert_eq!(value_of(&covered, "method"), Some("flickr.photos.getInfo"));
    assert_eq!(value_of(&covered, "photo_id"), Some("5550"));
    check_oauth(&info, &format!("{base}/rest/"), &covered, "api-secret", "at-sec");

    assert_eq!(result.url.as_deref(), Some("https://www.flickr.com/photos/me/5550/"));
    assert!(secrets.get(flickr::TOKEN_KEY).unwrap().unwrap().contains("at-tok"));
}

#[test]
fn flickr_with_a_stored_token_uploads_without_the_browser() {
    let server = Server::start(vec![
        Canned::ok(r#"<rsp stat="ok"><photoid>77</photoid></rsp>"#),
        Canned::ok(
            json!({"stat": "ok", "photo": {"urls": {"url": [{"type": "photopage", "_content": "https://www.flickr.com/photos/me/77/"}]}}})
                .to_string(),
        ),
    ]);
    let base = server.url("");
    let secrets =
        store(&[(flickr::API_SECRET_KEY, "api-secret"), (flickr::TOKEN_KEY, r#"{"oauth_token":"tok","oauth_token_secret":"tsec"}"#)]);
    let shell = Arc::new(FakeShell::default());
    let config = flickr::FlickrConfig { api_key: "api-key".to_owned(), public: true, endpoints: flickr_endpoints(&base) };
    let destination = flickr::Flickr::new(config, secrets, shell.clone(), Arc::new(CountingRng::default()), now());

    let result = block_on(destination.upload(png("shot.png", b"BYTES"), progress())).unwrap();

    let recorded = server.finish();
    assert!(shell.urls.lock().unwrap().is_empty(), "a stored token does not open the browser");
    let upload = &recorded[0];
    let params = check_oauth(upload, &format!("{base}/upload/"), &[], "api-secret", "tsec");
    assert_eq!(value_of(&params, "oauth_token"), Some("tok"));
    assert!(contains(&upload.body, b"name=\"is_public\"\r\n\r\n1"));
    let info = &recorded[1];
    check_oauth(info, &format!("{base}/rest/"), &query_pairs_of(&info.target), "api-secret", "tsec");
    assert_eq!(result.url.as_deref(), Some("https://www.flickr.com/photos/me/77/"));
}

#[test]
fn box_uploads_into_the_folder_and_shares_an_open_link() {
    let server = Server::start(vec![
        Canned::new(201, json!({"total_count": 1, "entries": [{"type": "file", "id": "123", "name": "a.png"}]}).to_string()),
        Canned::ok(json!({"id": "123", "shared_link": {"url": "https://app.box.com/s/abc"}}).to_string()),
    ]);
    let destination = box_com::BoxCom::new(signed_in("laqta/box", "box-tok"), server.url(""), server.url(""), "0".to_owned(), true);

    let result = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    let upload = &recorded[0];
    assert_eq!((upload.method.as_str(), upload.target.as_str()), ("POST", "/api/2.0/files/content"));
    assert_eq!(upload.header("authorization"), Some("Bearer box-tok"));
    assert!(upload.header("content-type").unwrap().starts_with("multipart/form-data"));
    assert!(contains(&upload.body, br#"{"name":"a.png","parent":{"id":"0"}}"#));
    assert!(contains(&upload.body, b"PNG"));
    let share = &recorded[1];
    assert_eq!((share.method.as_str(), share.target.as_str()), ("PUT", "/2.0/files/123?fields=shared_link"));
    assert_eq!(json_of(&share.body), json!({"shared_link": {"access": "open"}}));
    assert_eq!(result.url.as_deref(), Some("https://app.box.com/s/abc"));
    assert_eq!(destination.id(), "box");
}

#[test]
fn box_without_a_share_links_the_file_page_and_makes_one_request() {
    let server = Server::start(vec![Canned::new(201, json!({"entries": [{"id": "9"}]}).to_string())]);
    let destination = box_com::BoxCom::new(signed_in("laqta/box", "box-tok"), server.url(""), server.url(""), "42".to_owned(), false);

    let result = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    assert_eq!(recorded.len(), 1);
    assert!(contains(&recorded[0].body, br#"{"name":"a.png","parent":{"id":"42"}}"#));
    assert_eq!(result.url.as_deref(), Some("https://app.box.com/file/9"));
}

#[test]
fn youtube_opens_a_resumable_session_then_puts_the_video_and_returns_the_short_link() {
    let upload = Server::start(vec![Canned::ok(json!({"id": "vid123"}).to_string())]);
    let session = Server::start(vec![Canned::new(200, "").header("Location", &upload.url("/upload/session?upload_id=up-1"))]);
    let destination = youtube::YouTube::new(signed_in("laqta/youtube", "yt-tok"), session.url(""), "unlisted");
    let video = vec![7u8; 16];
    let request = UploadRequest {
        file_name: "clip.mp4".to_owned(),
        kind: UploadKind::File,
        data: UploadData::Bytes(video.clone()),
        mime: "video/mp4".to_owned(),
    };

    let result = block_on(destination.upload(request, progress())).unwrap();

    let start = &session.finish()[0];
    assert_eq!(start.method, "POST");
    let target = url::Url::parse(&format!("http://localhost{}", start.target)).unwrap();
    assert_eq!(target.path(), "/upload/youtube/v3/videos");
    let query: HashMap<String, String> = target.query_pairs().into_owned().collect();
    assert_eq!(query.get("uploadType").map(String::as_str), Some("resumable"));
    assert_eq!(query.get("part").map(String::as_str), Some("snippet,status"));
    assert_eq!(start.header("authorization"), Some("Bearer yt-tok"));
    assert_eq!(start.header("x-upload-content-type"), Some("video/mp4"));
    assert_eq!(start.header("x-upload-content-length"), Some("16"));
    let metadata = json_of(&start.body);
    assert_eq!(metadata["snippet"]["title"], "clip");
    assert_eq!(metadata["status"]["privacyStatus"], "unlisted");

    let put = &upload.finish()[0];
    assert_eq!((put.method.as_str(), put.target.as_str()), ("PUT", "/upload/session?upload_id=up-1"));
    assert_eq!(put.header("authorization"), Some("Bearer yt-tok"));
    assert_eq!(put.header("content-type"), Some("video/mp4"));
    assert_eq!(put.body, video);
    assert_eq!(result.url.as_deref(), Some("https://youtu.be/vid123"));
}

#[test]
fn youtube_refuses_a_file_that_is_not_a_video_before_sending_anything() {
    let destination = youtube::YouTube::new(signed_in("laqta/youtube", "yt-tok"), closed_url(), "private");

    let error = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Unsupported(_)), "{error:?}");
}

#[test]
fn youtube_privacy_values_outside_the_three_fall_back_to_unlisted() {
    assert_eq!(youtube::privacy_status("public"), "public");
    assert_eq!(youtube::privacy_status(" private "), "private");
    assert_eq!(youtube::privacy_status(""), "unlisted");
    assert_eq!(youtube::privacy_status("everyone"), "unlisted");
}

#[test]
fn gcs_uploads_with_its_hmac_key_in_path_style_and_region_auto() {
    let server = Server::start(vec![Canned::new(200, "")]);
    let base = server.url("");
    let secrets = store(&[(gcs::GCS_SECRET_KEY, "gcs-secret")]);
    let config = gcs::GcsConfig {
        endpoint: base.clone(),
        bucket: "bucket".to_owned(),
        path_prefix: "shots/".to_owned(),
        access_key_id: "GOOGAKID".to_owned(),
        public_url_base: None,
    };
    let destination = gcs::GoogleCloudStorage::new(&config, secrets);

    let result = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    let put = &recorded[0];
    assert_eq!(put.method, "PUT");
    assert!(put.target.starts_with("/bucket/shots/a.png?"), "{}", put.target);
    assert!(put.target.contains("X-Amz-Credential=GOOGAKID%2F"), "{}", put.target);
    assert!(put.target.contains("%2Fauto%2Fs3%2Faws4_request"), "{}", put.target);
    assert!(put.target.contains("X-Amz-Signature="), "{}", put.target);
    assert!(put.header("x-amz-acl").is_none(), "uniform bucket access rejects ACL headers");
    assert_eq!(put.body, b"PNG");
    assert_eq!(result.url.as_deref(), Some(format!("{base}/bucket/shots/a.png").as_str()));
    assert_eq!(destination.id(), "google-cloud-storage");
}

#[test]
fn gcs_never_uses_the_amazon_s3_secret() {
    let secrets = store(&[(s3::SECRET_KEY, "amazon-secret")]);
    let config = gcs::GcsConfig {
        endpoint: closed_url(),
        bucket: "bucket".to_owned(),
        path_prefix: String::new(),
        access_key_id: "GOOGAKID".to_owned(),
        public_url_base: None,
    };
    let destination = gcs::GoogleCloudStorage::new(&config, secrets);

    let error = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

fn azure_config(blob_base: String) -> azure_blob::AzureConfig {
    azure_blob::AzureConfig {
        account: "acct".to_owned(),
        container: "container".to_owned(),
        path_prefix: "shots/".to_owned(),
        public_url_base: None,
        blob_base,
    }
}

#[test]
fn azure_shared_key_signs_the_put_over_the_headers_and_resource_as_sent() {
    let server = Server::start(vec![Canned::new(201, "")]);
    let base = server.url("");
    let key = STANDARD.encode(b"0123456789abcdef");
    let secrets = store(&[(azure_blob::ACCOUNT_KEY, key.as_str())]);
    let destination = azure_blob::AzureBlob::new(azure_config(base.clone()), secrets, now());

    let result = block_on(destination.upload(png("a b.png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    let put = &recorded[0];
    assert_eq!((put.method.as_str(), put.target.as_str()), ("PUT", "/container/shots/a%20b.png"));
    assert_eq!(put.header("x-ms-version"), Some("2023-11-03"));
    assert_eq!(put.header("x-ms-blob-type"), Some("BlockBlob"));
    assert_eq!(put.header("x-ms-date"), Some("Mon, 12 Jan 1970 13:46:40 GMT"));
    assert_eq!(put.header("content-type"), Some("image/png"));
    assert_eq!(put.body, b"PNG");
    let to_sign = azure_blob::string_to_sign(
        "PUT",
        3,
        "image/png",
        &[("x-ms-blob-type", "BlockBlob"), ("x-ms-date", "Mon, 12 Jan 1970 13:46:40 GMT"), ("x-ms-version", "2023-11-03")],
        "acct",
        "/container/shots/a%20b.png",
    );
    let expected = format!("SharedKey acct:{}", azure_blob::sign_shared_key(&key, &to_sign).unwrap());
    assert_eq!(put.header("authorization"), Some(expected.as_str()));
    assert_eq!(result.url.as_deref(), Some(format!("{base}/container/shots/a%20b.png").as_str()));
}

#[test]
fn azure_sas_token_is_appended_to_the_url_and_no_key_is_sent() {
    let server = Server::start(vec![Canned::new(201, "")]);
    let base = server.url("");
    let secrets = store(&[(azure_blob::SAS_KEY, "?sv=2023-11-03&sig=abc%3D")]);
    let destination = azure_blob::AzureBlob::new(azure_config(base), secrets, now());

    block_on(destination.upload(png("a b.png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    let put = &recorded[0];
    assert_eq!(put.target, "/container/shots/a%20b.png?sv=2023-11-03&sig=abc%3D");
    assert!(put.header("authorization").is_none());
    assert_eq!(put.header("x-ms-version"), Some("2023-11-03"));
}

#[test]
fn azure_without_a_sas_or_a_key_fails_before_sending() {
    let destination = azure_blob::AzureBlob::new(azure_config(closed_url()), store(&[]), now());

    let error = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

#[test]
fn azure_shared_key_hmac_matches_the_rfc_4231_test_case_1() {
    // RFC 4231 test case 1: key = 20 bytes of 0x0b, data = "Hi There", HMAC-SHA-256.
    let key = STANDARD.encode([0x0bu8; 20]);
    assert_eq!(azure_blob::sign_shared_key(&key, "Hi There").unwrap(), "sDRMYdjbOFNcqK/OrwvxK4gdwgDJgz2nJuk3bC4yz/c=");
}

#[test]
fn b2_authorizes_finds_the_bucket_gets_an_upload_url_and_uploads() {
    // Three servers, started so that each reply can name the next one: the upload host, the API
    // host that hands out upload URLs, and the authorization endpoint whose reply names the API host.
    let upload = Server::start(vec![Canned::new(200, json!({"fileId": "f1", "fileName": "shots/a b.png"}).to_string())]);
    let upload_base = upload.url("");
    let api = Server::start(vec![
        Canned::ok(
            json!({"buckets": [{"bucketId": "other", "bucketName": "other"}, {"bucketId": "bid-1", "bucketName": "bkt"}]}).to_string(),
        ),
        Canned::ok(json!({"bucketId": "bid-1", "uploadUrl": upload.url("/upload/one"), "authorizationToken": "up-tok"}).to_string()),
    ]);
    let api_base = api.url("");
    let auth = Server::start(vec![Canned::ok(
        json!({"accountId": "acc", "authorizationToken": "auth-tok", "apiUrl": api_base, "downloadUrl": upload_base}).to_string(),
    )]);
    let config = b2::B2Config {
        api_base: auth.url(""),
        key_id: "keyid".to_owned(),
        bucket: "bkt".to_owned(),
        path_prefix: "shots/".to_owned(),
        public_url_base: None,
    };
    let destination = b2::BackblazeB2::new(config, store(&[(b2::SECRET_KEY, "appkey")]));

    let result = block_on(destination.upload(png("a b.png", b"PNG"), progress())).unwrap();

    let authorize = &auth.finish()[0];
    assert_eq!((authorize.method.as_str(), authorize.target.as_str()), ("GET", "/b2api/v2/b2_authorize_account"));
    assert_eq!(authorize.header("authorization"), Some(format!("Basic {}", STANDARD.encode("keyid:appkey")).as_str()));

    let calls = api.finish();
    let list = &calls[0];
    assert_eq!((list.method.as_str(), list.target.as_str()), ("POST", "/b2api/v2/b2_list_buckets"));
    assert_eq!(list.header("authorization"), Some("auth-tok"));
    assert_eq!(json_of(&list.body), json!({"accountId": "acc", "bucketName": "bkt"}));
    let target = &calls[1];
    assert_eq!((target.method.as_str(), target.target.as_str()), ("POST", "/b2api/v2/b2_get_upload_url"));
    assert_eq!(target.header("authorization"), Some("auth-tok"));
    assert_eq!(json_of(&target.body), json!({"bucketId": "bid-1"}));

    let put = &upload.finish()[0];
    assert_eq!((put.method.as_str(), put.target.as_str()), ("POST", "/upload/one"));
    assert_eq!(put.header("authorization"), Some("up-tok"));
    assert_eq!(put.header("x-bz-file-name"), Some("shots/a%20b.png"));
    let sha1: String = Sha1::digest(b"PNG").iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(put.header("x-bz-content-sha1"), Some(sha1.as_str()));
    assert_eq!(put.body, b"PNG");
    assert_eq!(result.url.as_deref(), Some(format!("{upload_base}/file/bkt/shots/a%20b.png").as_str()));
}

#[test]
fn b2_without_an_application_key_fails_before_authorizing() {
    let config = b2::B2Config {
        api_base: closed_url(),
        key_id: "keyid".to_owned(),
        bucket: "bkt".to_owned(),
        path_prefix: String::new(),
        public_url_base: Some("https://files.example.com/".to_owned()),
    };
    let destination = b2::BackblazeB2::new(config, store(&[]));

    let error = block_on(destination.upload(png("a.png", b"PNG"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

#[test]
fn b2_object_names_keep_slashes_and_encode_each_segment() {
    assert_eq!(b2::encode_path("shots/Laqta 1#2.png"), "shots/Laqta%201%232.png");
    assert_eq!(azure_blob::encode_path("a b/c.png"), "a%20b/c.png");
}

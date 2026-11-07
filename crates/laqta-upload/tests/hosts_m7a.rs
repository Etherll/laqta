//! The M7 HO-a hosts (Gist, PrivateBin, Pushbullet, Nextcloud, SendSpace, MediaFire, SMTP) against
//! the local test server with canned replies, plus the pure pieces: the PrivateBin envelope, the
//! MediaFire signature and the SMTP message. No network.
#![allow(clippy::unwrap_used)]

#[path = "support/mod.rs"]
mod support;

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use futures_lite::future::block_on;
use laqta_domain::hosts::{HostAccount, HostKind};
use laqta_ports::fakes::{CountingRng, FakeSecretStore, FakeShell};
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadKind, UploadRequest};
use laqta_upload::hosts::{gist, mediafire, nextcloud, privatebin, pushbullet, sendspace, smtp};
use laqta_upload::native::{HostDeps, native_hosts};
use serde_json::json;
use support::{Canned, Server};

fn store(entries: &[(&str, &str)]) -> Arc<dyn SecretStore> {
    let secrets: HashMap<String, String> = entries.iter().map(|(key, value)| (key.to_string(), value.to_string())).collect();
    Arc::new(FakeSecretStore { secrets: Mutex::new(secrets) })
}

fn progress() -> ProgressCallback {
    Arc::new(|_sent, _total| {})
}

fn text_request(name: &str, text: &str) -> UploadRequest {
    UploadRequest {
        file_name: name.to_owned(),
        kind: UploadKind::Text,
        data: UploadData::Bytes(text.as_bytes().to_vec()),
        mime: "text/plain".to_owned(),
    }
}

fn file_request(name: &str, mime: &str, bytes: &[u8]) -> UploadRequest {
    UploadRequest { file_name: name.to_owned(), kind: UploadKind::File, data: UploadData::Bytes(bytes.to_vec()), mime: mime.to_owned() }
}

/// The body as a JSON value.
fn body_json(body: &[u8]) -> serde_json::Value {
    serde_json::from_slice(body).unwrap()
}

/// The name/value pairs of a form-encoded body.
fn form(body: &[u8]) -> Vec<(String, String)> {
    url::form_urlencoded::parse(body).into_owned().collect()
}

/// The name/value pairs of a request target's query string.
fn query(target: &str) -> Vec<(String, String)> {
    let (_, query) = target.split_once('?').unwrap_or((target, ""));
    url::form_urlencoded::parse(query.as_bytes()).into_owned().collect()
}

fn pair(name: &str, value: &str) -> (String, String) {
    (name.to_owned(), value.to_owned())
}

fn text_of(body: &[u8]) -> String {
    String::from_utf8_lossy(body).into_owned()
}

// ---- GitHub Gist ----

#[test]
fn gist_creates_a_public_gist_with_the_text_as_its_one_file() {
    let server = Server::start(vec![Canned::new(201, r#"{"html_url":"https://gist.github.com/alice/abc123"}"#)]);
    let config = gist::GistConfig { api_base: server.url(""), public: true };
    let destination = gist::Gist::new(config, store(&[(gist::SECRET_KEY, "ghp_token")]));

    let result = block_on(destination.upload(text_request("notes.txt", "hello"), progress())).unwrap();

    let recorded = server.finish();
    let request = &recorded[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/gists");
    assert_eq!(request.header("authorization"), Some("Bearer ghp_token"));
    assert_eq!(request.header("accept"), Some("application/vnd.github+json"));
    assert_eq!(body_json(&request.body), json!({"public": true, "files": {"notes.txt": {"content": "hello"}}}));
    assert_eq!(result.url.as_deref(), Some("https://gist.github.com/alice/abc123"));
}

#[test]
fn gist_takes_text_uploads_only_and_sends_nothing_otherwise() {
    let server = Server::start(Vec::new());
    let config = gist::GistConfig { api_base: server.url(""), public: false };
    let destination = gist::Gist::new(config, store(&[(gist::SECRET_KEY, "ghp_token")]));

    let error = block_on(destination.upload(file_request("a.png", "image/png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Unsupported(_)), "{error:?}");
    assert!(server.finish().is_empty());
}

#[test]
fn gist_a_rejected_token_is_an_auth_error() {
    let server = Server::start(vec![Canned::new(401, r#"{"message":"Bad credentials"}"#)]);
    let config = gist::GistConfig { api_base: server.url(""), public: false };
    let destination = gist::Gist::new(config, store(&[(gist::SECRET_KEY, "bad")]));

    let error = block_on(destination.upload(text_request("a.txt", "x"), progress())).unwrap_err();

    server.finish();
    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
    assert!(!error.is_retryable());
}

// ---- PrivateBin ----

#[test]
fn privatebin_seal_and_open_round_trip_and_need_the_password_and_the_adata() {
    let sealed = privatebin::seal("secret text", "pw", false, &CountingRng::default()).unwrap();

    assert_eq!(privatebin::open(&sealed.key, &sealed.adata, &sealed.ct, "pw").unwrap(), "secret text");
    assert!(privatebin::open(&sealed.key, &sealed.adata, &sealed.ct, "wrong").is_err());

    let mut tampered = sealed.adata.clone();
    tampered[3] = json!(1);
    assert!(privatebin::open(&sealed.key, &tampered, &sealed.ct, "pw").is_err(), "the burn flag is authenticated");
}

#[test]
fn privatebin_adata_has_the_v2_shape_with_16_byte_iv_and_salt() {
    let sealed = privatebin::seal("x", "", true, &CountingRng::default()).unwrap();

    let params = sealed.adata[0].as_array().unwrap();
    assert_eq!(params.len(), 8);
    assert_eq!(params[2], json!(100_000));
    assert_eq!(&params[3..], &[json!(256), json!(128), json!("aes"), json!("gcm"), json!("zlib")]);
    assert_eq!(STANDARD.decode(params[0].as_str().unwrap()).unwrap().len(), 16, "IV");
    assert_eq!(STANDARD.decode(params[1].as_str().unwrap()).unwrap().len(), 16, "salt");
    assert_eq!(sealed.adata[1], json!("plaintext"));
    assert_eq!(sealed.adata[2], json!(0));
    assert_eq!(sealed.adata[3], json!(1), "burn after reading");
    // The key is 32 bytes and its base58 form is what the fragment carries.
    assert_eq!(sealed.key.len(), 32);
}

#[test]
fn privatebin_posts_the_envelope_and_the_url_fragment_opens_it() {
    let server =
        Server::start(vec![Canned::new(200, r#"{"status":0,"id":"0f1e2d3c4b5a6978","url":"/?0f1e2d3c4b5a6978","deletetoken":"t"}"#)]);
    let base = server.url("");
    let config = privatebin::PrivateBinConfig { server: base.clone(), expire: "1week".to_owned(), burn_after_reading: true };
    let destination = privatebin::PrivateBin::new(config, store(&[(privatebin::PASSWORD_KEY, "pw")]), Arc::new(CountingRng::default()));

    let result = block_on(destination.upload(text_request("paste.txt", "hello paste"), progress())).unwrap();

    let recorded = server.finish();
    let request = &recorded[0];
    assert_eq!(request.method, "POST");
    assert_eq!(request.target, "/");
    assert_eq!(request.header("x-requested-with"), Some("JSONHttpRequest"));
    let body = body_json(&request.body);
    assert_eq!(body["v"], 2);
    assert_eq!(body["meta"], json!({"expire": "1week"}));
    assert_eq!(body["adata"][3], 1);
    assert!(!text_of(&request.body).contains("hello paste"), "the server never sees the text");

    let url = result.url.unwrap();
    let prefix = format!("{base}/?0f1e2d3c4b5a6978#");
    let fragment = url.strip_prefix(&prefix).unwrap_or_else(|| panic!("{url} does not start with {prefix}"));
    let key = bs58::decode(fragment).into_vec().unwrap();
    assert_eq!(key.len(), 32);
    let opened = privatebin::open(&key, &body["adata"], body["ct"].as_str().unwrap(), "pw").unwrap();
    assert_eq!(opened, "hello paste");
}

#[test]
fn privatebin_a_failed_status_is_a_bad_response_with_the_message() {
    let server = Server::start(vec![Canned::new(200, r#"{"status":1,"message":"Paste is too big"}"#)]);
    let config = privatebin::PrivateBinConfig { server: server.url(""), expire: "1week".to_owned(), burn_after_reading: false };
    let destination = privatebin::PrivateBin::new(config, store(&[]), Arc::new(CountingRng::default()));

    let error = block_on(destination.upload(text_request("p.txt", "x"), progress())).unwrap_err();

    server.finish();
    let UploadError::BadResponse(message) = &error else { panic!("{error:?}") };
    assert!(message.contains("Paste is too big"), "{message}");
}

// ---- Pushbullet ----

#[test]
fn pushbullet_requests_an_upload_then_posts_the_file_last_then_pushes_it() {
    let upload_server = Server::start(vec![Canned::new(204, "")]);
    let upload_url = upload_server.url("/upload/abc");
    let api = Server::start(vec![
        Canned::ok(
            json!({
                "file_url": "https://dl.pushbulletusercontent.com/x/f.png",
                "upload_url": upload_url,
                "data": {"acl": "public-read", "key": "x/f.png"}
            })
            .to_string(),
        ),
        Canned::ok(r#"{"iden":"push1"}"#),
    ]);
    let config = pushbullet::PushbulletConfig { api_base: api.url(""), device_iden: Some("dev1".to_owned()) };
    let destination = pushbullet::Pushbullet::new(config, store(&[(pushbullet::SECRET_KEY, "o.token")]));

    let result = block_on(destination.upload(file_request("f.png", "image/png", b"PNGBYTES"), progress())).unwrap();

    let requests = api.finish();
    let uploads = upload_server.finish();
    assert_eq!(requests[0].target, "/v2/upload-request");
    assert_eq!(requests[0].header("access-token"), Some("o.token"));
    assert_eq!(body_json(&requests[0].body), json!({"file_name": "f.png", "file_type": "image/png"}));

    let upload = &uploads[0];
    assert_eq!(upload.method, "POST");
    assert_eq!(upload.target, "/upload/abc");
    assert_eq!(upload.header("access-token"), None, "the upload is not sent to the API");
    let body = text_of(&upload.body);
    assert!(body.contains("name=\"acl\"") && body.contains("public-read"));
    assert!(body.contains("name=\"file\"") && body.contains("filename=\"f.png\""));
    assert!(body.contains("PNGBYTES"));
    assert!(body.find("name=\"file\"").unwrap() > body.find("name=\"key\"").unwrap(), "the file is the last part");

    assert_eq!(requests[1].target, "/v2/pushes");
    assert_eq!(requests[1].header("access-token"), Some("o.token"));
    assert_eq!(
        body_json(&requests[1].body),
        json!({
            "type": "file",
            "file_name": "f.png",
            "file_type": "image/png",
            "file_url": "https://dl.pushbulletusercontent.com/x/f.png",
            "device_iden": "dev1"
        })
    );
    assert_eq!(result.url.as_deref(), Some("https://dl.pushbulletusercontent.com/x/f.png"));
}

// ---- Nextcloud / ownCloud ----

#[test]
fn nextcloud_puts_the_file_then_shares_it_with_a_download_link() {
    let server = Server::start(vec![
        Canned::new(201, ""),
        Canned::ok(
            r#"{"ocs":{"meta":{"status":"ok","statuscode":100,"message":"OK"},"data":{"url":"https://cloud.example.com/s/AbC123"}}}"#,
        ),
    ]);
    let config = nextcloud::NextcloudConfig {
        server: server.url(""),
        username: "alice".to_owned(),
        directory: "/Laqta".to_owned(),
        share: true,
        direct_link: true,
    };
    let destination = nextcloud::Nextcloud::new(config, store(&[(nextcloud::SECRET_KEY, "app-pass")]));

    let result = block_on(destination.upload(file_request("Laqta 1.png", "image/png", b"PNG"), progress())).unwrap();

    let recorded = server.finish();
    let put = &recorded[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.target, "/remote.php/dav/files/alice/Laqta/Laqta%201.png");
    let basic = format!("Basic {}", STANDARD.encode("alice:app-pass"));
    assert_eq!(put.header("authorization"), Some(basic.as_str()));
    assert_eq!(put.header("ocs-apirequest"), Some("true"));
    assert_eq!(put.body, b"PNG".to_vec());

    let share = &recorded[1];
    assert_eq!(share.method, "POST");
    assert_eq!(share.target, "/ocs/v1.php/apps/files_sharing/api/v1/shares?format=json");
    assert_eq!(query(&share.target), vec![pair("format", "json")]);
    assert_eq!(share.header("ocs-apirequest"), Some("true"));
    assert_eq!(share.header("authorization"), Some(basic.as_str()));
    let fields = form(&share.body);
    assert!(fields.contains(&pair("shareType", "3")), "{fields:?}");
    assert!(fields.contains(&pair("path", "/Laqta/Laqta 1.png")), "{fields:?}");
    assert!(fields.contains(&pair("permissions", "1")), "{fields:?}");

    assert_eq!(result.url.as_deref(), Some("https://cloud.example.com/s/AbC123/download"));
}

#[test]
fn nextcloud_without_a_share_returns_the_webdav_url() {
    let server = Server::start(vec![Canned::new(204, "")]);
    let base = server.url("");
    let config = nextcloud::NextcloudConfig {
        server: base.clone(),
        username: "alice".to_owned(),
        directory: "Laqta".to_owned(),
        share: false,
        direct_link: false,
    };
    let destination = nextcloud::Nextcloud::new(config, store(&[(nextcloud::SECRET_KEY, "app-pass")]));

    let result = block_on(destination.upload(file_request("a.png", "image/png", b"x"), progress())).unwrap();

    let recorded = server.finish();
    assert_eq!(recorded.len(), 1, "no OCS call without a share");
    assert_eq!(result.url, Some(format!("{base}/remote.php/dav/files/alice/Laqta/a.png")));
}

#[test]
fn nextcloud_a_missing_folder_says_so() {
    let server = Server::start(vec![Canned::new(409, "")]);
    let config = nextcloud::NextcloudConfig {
        server: server.url(""),
        username: "alice".to_owned(),
        directory: "/Missing".to_owned(),
        share: true,
        direct_link: false,
    };
    let destination = nextcloud::Nextcloud::new(config, store(&[(nextcloud::SECRET_KEY, "app-pass")]));

    let error = block_on(destination.upload(file_request("a.png", "image/png", b"x"), progress())).unwrap_err();

    server.finish();
    let UploadError::BadResponse(message) = &error else { panic!("{error:?}") };
    assert!(message.contains("does not exist"), "{message}");
}

#[test]
fn nextcloud_a_folder_that_leaves_the_user_files_is_refused() {
    let server = Server::start(Vec::new());
    let config = nextcloud::NextcloudConfig {
        server: server.url(""),
        username: "alice".to_owned(),
        directory: "/../other".to_owned(),
        share: true,
        direct_link: false,
    };
    let destination = nextcloud::Nextcloud::new(config, store(&[(nextcloud::SECRET_KEY, "app-pass")]));

    let error = block_on(destination.upload(file_request("a.png", "image/png", b"x"), progress())).unwrap_err();

    assert!(matches!(error, UploadError::Unsupported(_)), "{error:?}");
    assert!(server.finish().is_empty());
}

// ---- SendSpace ----

#[test]
fn sendspace_gets_the_upload_info_posts_the_file_and_returns_the_download_link() {
    let upload_server = Server::start(vec![Canned::ok(
        r#"<?xml version="1.0"?><response><status>ok</status><download_url>https://www.sendspace.com/file/xyz</download_url><delete_url>https://www.sendspace.com/delete/xyz</delete_url></response>"#,
    )]);
    let info = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><result method="anonymous.uploadGetInfo" status="ok"><upload url="{}" progress_url="http://progress.example.com/p" max_file_size="300000000" upload_identifier="p42" extra_info="xyz"/></result>"#,
        upload_server.url("/upload.php")
    );
    let api = Server::start(vec![Canned::ok(info)]);
    let config = sendspace::SendSpaceConfig { api_base: api.url("/rest/"), username: String::new() };
    let destination = sendspace::SendSpace::new(config, store(&[(sendspace::API_KEY_KEY, "KEY")]));

    let result = block_on(destination.upload(file_request("a.bin", "application/octet-stream", b"DATA"), progress())).unwrap();

    let requests = api.finish();
    let info_request = &requests[0];
    assert_eq!(info_request.method, "GET");
    assert!(info_request.target.starts_with("/rest/?"), "{}", info_request.target);
    let fields = query(&info_request.target);
    assert!(fields.contains(&pair("method", "anonymous.uploadGetInfo")), "{fields:?}");
    assert!(fields.contains(&pair("api_key", "KEY")), "{fields:?}");
    assert!(fields.contains(&pair("api_version", "1.0")), "{fields:?}");
    assert!(fields.contains(&pair("app_version", "1.0")), "{fields:?}");

    let uploads = upload_server.finish();
    let upload = &uploads[0];
    assert_eq!(upload.method, "POST");
    assert_eq!(upload.target, "/upload.php");
    let body = text_of(&upload.body);
    assert!(body.contains("name=\"UPLOAD_IDENTIFIER\"") && body.contains("p42"), "{body}");
    assert!(body.contains("name=\"MAX_FILE_SIZE\"") && body.contains("300000000"), "{body}");
    assert!(body.contains("name=\"extra_info\"") && body.contains("xyz"), "{body}");
    assert!(body.contains("name=\"userfile\"") && body.contains("filename=\"a.bin\""), "{body}");
    assert!(body.contains("DATA"));

    assert_eq!(result.url.as_deref(), Some("https://www.sendspace.com/file/xyz"));
}

#[test]
fn sendspace_an_error_reply_is_a_bad_response() {
    let api = Server::start(vec![Canned::ok(
        r#"<?xml version="1.0"?><result method="anonymous.uploadGetInfo" status="fail"><error code="2" text="Invalid API key"/></result>"#,
    )]);
    let config = sendspace::SendSpaceConfig { api_base: api.url("/rest/"), username: String::new() };
    let destination = sendspace::SendSpace::new(config, store(&[(sendspace::API_KEY_KEY, "bad")]));

    let error = block_on(destination.upload(file_request("a.bin", "application/octet-stream", b"D"), progress())).unwrap_err();

    api.finish();
    let UploadError::BadResponse(message) = &error else { panic!("{error:?}") };
    assert!(message.contains("Invalid API key"), "{message}");
}

#[test]
fn sendspace_signs_in_then_gets_the_upload_info() {
    let upload_server = Server::start(vec![Canned::ok(
        r#"<?xml version="1.0"?><response><status>ok</status><download_url>https://www.sendspace.com/file/u1</download_url></response>"#,
    )]);
    let info = format!(
        r#"<result method="upload.getInfo" status="ok"><upload url="{}" max_file_size="1" upload_identifier="i" extra_info="e"/></result>"#,
        upload_server.url("/up")
    );
    let api = Server::start(vec![
        Canned::ok(r#"<result method="auth.createToken" status="ok"><token>TOK</token></result>"#),
        Canned::ok(r#"<result method="auth.login" status="ok"><session_key>SESS</session_key><user_name>alice</user_name></result>"#),
        Canned::ok(info),
    ]);
    let config = sendspace::SendSpaceConfig { api_base: api.url("/rest/"), username: "alice".to_owned() };
    let destination = sendspace::SendSpace::new(config, store(&[(sendspace::API_KEY_KEY, "KEY"), (sendspace::PASSWORD_KEY, "pw")]));

    let result = block_on(destination.upload(file_request("a.bin", "application/octet-stream", b"D"), progress())).unwrap();

    let requests = api.finish();
    let login = text_of(&requests[1].body);
    assert!(login.contains("auth.login") && login.contains(&sendspace::tokened_password("TOK", "pw")), "{login}");
    assert!(query(&requests[2].target).contains(&pair("session_key", "SESS")));
    upload_server.finish();
    assert_eq!(result.url.as_deref(), Some("https://www.sendspace.com/file/u1"));
    assert_eq!(sendspace::tokened_password("t", "").len(), 32);
}

// ---- MediaFire ----

#[test]
fn mediafire_signature_is_the_sha1_of_the_concatenated_inputs() {
    // `printf 'abcd' | sha1sum`
    assert_eq!(mediafire::signature("a", "b", "c", "d"), "81fe8bfe87576c3ecb22426f8e57847382917acf");
    // `printf 'me@example.comhunter212345APIKEY' | sha1sum`
    assert_eq!(mediafire::signature("me@example.com", "hunter2", "12345", "APIKEY"), "81dc50f1e0c6647539cd84f3491948f23d577527");
}

#[test]
fn mediafire_signs_in_uploads_and_polls_for_the_quickkey() {
    let api = Server::start(vec![
        Canned::ok(r#"{"response":{"result":"Success","session_token":"ST","secret_key":298,"time":"123.4","new_key":"no"}}"#),
        Canned::ok(r#"{"response":{"result":"Success","doupload":{"result":"0","key":"UK"},"new_key":"yes"}}"#),
        Canned::ok(r#"{"response":{"result":"Success","doupload":{"status":"17"}}}"#),
        Canned::ok(r#"{"response":{"result":"Success","doupload":{"status":"99","quickkey":"qk1"}}}"#),
    ]);
    let config = mediafire::MediaFireConfig {
        api_base: api.url("/api/"),
        app_id: "APP".into(),
        email: "me@example.com".into(),
        folder: String::new(),
        poll_interval: std::time::Duration::from_millis(1),
    };
    let destination = mediafire::MediaFire::new(config, store(&[(mediafire::API_KEY_KEY, "AK"), (mediafire::PASSWORD_KEY, "pw")]));

    let result = block_on(destination.upload(file_request("a.bin", "application/octet-stream", b"DATA"), progress())).unwrap();

    let requests = api.finish();
    assert!(requests[0].target.ends_with("user/get_session_token.php"));
    let login = text_of(&requests[0].body);
    assert!(login.contains(&mediafire::signature("me@example.com", "pw", "APP", "AK")), "{login}");
    let up = query(&requests[1].target);
    let unsigned: Vec<(String, String)> = up.iter().filter(|(k, _)| k != "signature").cloned().collect();
    let sig = up.iter().find(|(k, _)| k == "signature").map(|(_, v)| v.clone()).unwrap();
    assert_eq!(sig, mediafire::call_signature(298, "123.4", "upload/simple.php", &unsigned));
    assert!(text_of(&requests[1].body).contains("name=\"Filedata\""));
    // The upload reply said new_key = yes, so the polls are signed with the next key.
    let poll = text_of(&requests[2].body);
    assert!(
        poll.contains(&mediafire::call_signature(
            mediafire::next_key(298),
            "123.4",
            "upload/poll_upload.php",
            &[pair("session_token", "ST"), pair("key", "UK"), pair("filename", "a.bin"), pair("response_format", "json"),]
        )),
        "{poll}"
    );
    assert_eq!(result.url.as_deref(), Some("https://www.mediafire.com/file/qk1"));
}

// ---- SMTP ----

fn smtp_config(to: &str) -> smtp::SmtpConfig {
    smtp::SmtpConfig {
        host: "smtp.example.com".to_owned(),
        port: 587,
        tls: smtp::SmtpTls::StartTls,
        username: "alice".to_owned(),
        from: "Laqta <laqta@example.com>".to_owned(),
        to: to.to_owned(),
        subject: "Laqta: %filename%".to_owned(),
    }
}

#[test]
fn smtp_message_has_the_subject_the_recipients_and_the_file_as_attachment() {
    // Non-ASCII bytes, so lettre base64-encodes the attachment.
    let bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    let message = smtp::build_message(&smtp_config("ops@example.com, lead@example.com"), "shot.png", "image/png", bytes.clone()).unwrap();

    let text = text_of(&message.formatted());
    assert!(text.contains("Subject: Laqta: shot.png"), "{text}");
    assert!(text.contains("From: Laqta <laqta@example.com>"), "{text}");
    assert!(text.contains("ops@example.com") && text.contains("lead@example.com"), "{text}");
    assert!(text.contains("Content-Type: image/png"), "{text}");
    assert!(text.contains("filename=\"shot.png\""), "{text}");
    assert!(text.contains("Content-Transfer-Encoding: base64"), "{text}");
    assert!(text.contains(&STANDARD.encode(&bytes)), "the attachment carries the bytes: {text}");
}

#[test]
fn smtp_a_missing_mime_type_falls_back_to_octet_stream() {
    let message = smtp::build_message(&smtp_config("ops@example.com"), "data.bin", "", b"D".to_vec()).unwrap();

    let text = text_of(&message.formatted());
    assert!(text.contains("Content-Type: application/octet-stream"), "{text}");
}

#[test]
fn smtp_refuses_a_bad_recipient_or_none_before_sending() {
    let bad = smtp::build_message(&smtp_config("not an address"), "a.png", "image/png", vec![1]);
    assert!(matches!(bad, Err(UploadError::Unsupported(_))), "{bad:?}");

    let none = smtp::build_message(&smtp_config(" , "), "a.png", "image/png", vec![1]);
    assert!(matches!(none, Err(UploadError::Unsupported(_))), "{none:?}");
}

// ---- The native host list ----

#[test]
fn native_hosts_build_a_destination_for_each_ho_a_account() {
    let kinds = [
        HostKind::Gist,
        HostKind::PrivateBin,
        HostKind::Pushbullet,
        HostKind::Nextcloud,
        HostKind::SendSpace,
        HostKind::MediaFire,
        HostKind::Smtp,
    ];
    let accounts: Vec<HostAccount> = kinds.iter().map(|kind| HostAccount::new(*kind)).collect();
    let deps = HostDeps {
        secrets: store(&[]),
        shell: Arc::new(FakeShell::default()),
        csprng: Arc::new(CountingRng::default()),
        now_unix: Arc::new(|| 0),
    };

    let hosts = native_hosts(&accounts, &deps);

    let ids: Vec<&str> = hosts.destinations.iter().map(|destination| destination.id()).collect();
    for kind in kinds {
        assert!(ids.contains(&kind.id()), "{kind:?} is not built: {ids:?}");
    }
}

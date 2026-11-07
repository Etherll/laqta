//! Black-box tests of the native hosts against a local HTTP server: request shapes, parsed results
//! and status mapping, with and without a sign-in.
#![allow(clippy::unwrap_used)]

mod support_oauth;

use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_ports::fakes::{FakeSecretStore, FakeShell};
use laqta_ports::{Destination, ProgressCallback, SecretStore, Shortener, UploadData, UploadError, UploadKind, UploadRequest};
use laqta_upload::hosts::bitly::Bitly;
use laqta_upload::hosts::dropbox::Dropbox;
use laqta_upload::hosts::google_drive::GoogleDrive;
use laqta_upload::hosts::imgur::Imgur;
use laqta_upload::hosts::onedrive::OneDrive;
use laqta_upload::oauth::OAuth;
use serde_json::{Value, json};
use support_oauth::sign_in::{NOW, config, make_oauth, stored, token_reply};
use support_oauth::{Canned, Server};

fn request(name: &str, data: &[u8], mime: &str) -> UploadRequest {
    UploadRequest { file_name: name.to_owned(), kind: UploadKind::Image, data: UploadData::Bytes(data.to_vec()), mime: mime.to_owned() }
}

fn progress() -> ProgressCallback {
    Arc::new(|_, _| {})
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|window| window == needle)
}

fn json_of(bytes: &[u8]) -> Value {
    serde_json::from_slice(bytes).unwrap()
}

/// A sign-in whose store already holds `token` under `key`. Its token endpoint is `token_url`, which
/// a test reaches only when the token has expired.
fn signed_in(key: &str, token: &str, token_url: &str) -> (Arc<OAuth>, Arc<FakeSecretStore>, Arc<FakeShell>) {
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set(key, token).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(token_url.to_owned()), secrets.clone(), shell.clone(), key);
    (oauth, secrets, shell)
}

#[test]
fn imgur_anonymous_upload_uses_the_client_id_and_parses_the_links() {
    let reply = json!({"data": {"link": "https://i.imgur.com/AbC.png", "deletehash": "hash1"}, "success": true, "status": 200});
    let server = Server::start(vec![Canned::ok(reply.to_string())]);
    let host = Imgur::new("test-id", None, server.url(""));

    let result = block_on(host.upload(request("shot.png", b"PNG-DATA", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    let upload = &recorded[0];
    assert_eq!((upload.method.as_str(), upload.target.as_str()), ("POST", "/3/image"));
    assert_eq!(upload.header("authorization"), Some("Client-ID test-id"));
    assert!(upload.header("content-type").unwrap().starts_with("multipart/form-data"));
    assert!(contains(&upload.body, b"name=\"image\""));
    assert!(contains(&upload.body, b"filename=\"shot.png\""));
    assert!(contains(&upload.body, b"PNG-DATA"));
    assert_eq!(result.url.as_deref(), Some("https://i.imgur.com/AbC.png"));
    assert_eq!(result.thumbnail_url.as_deref(), Some("https://i.imgur.com/AbCm.png"));
    assert_eq!(result.deletion_url.as_deref(), Some("https://imgur.com/delete/hash1"));
    assert_eq!(host.id(), "imgur");
}

#[test]
fn imgur_reads_a_file_from_disk() {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), b"FROM-DISK").unwrap();
    let server = Server::start(vec![Canned::ok(json!({"data": {"link": "https://i.imgur.com/d.png"}}).to_string())]);
    let host = Imgur::new("test-id", None, server.url(""));
    let upload = UploadRequest {
        file_name: "disk.png".to_owned(),
        kind: UploadKind::File,
        data: UploadData::Path(file.path().to_path_buf()),
        mime: "image/png".to_owned(),
    };

    block_on(host.upload(upload, progress())).unwrap();
    let recorded = server.finish();

    assert!(contains(&recorded[0].body, b"FROM-DISK"));
}

#[test]
fn imgur_with_a_signed_in_account_uploads_as_that_account() {
    let server = Server::start(vec![Canned::ok(json!({"data": {"link": "https://i.imgur.com/Zz9.gif"}}).to_string())]);
    let (oauth, _secrets, shell) = signed_in("laqta/imgur", &stored("imgur-token", None, None), "unused");
    let host = Imgur::new("test-id", Some(oauth), server.url(""));

    let result = block_on(host.upload(request("clip.gif", b"GIF", "image/gif"), progress())).unwrap();
    let recorded = server.finish();

    assert_eq!(recorded[0].header("authorization"), Some("Bearer imgur-token"));
    assert_eq!(result.thumbnail_url.as_deref(), Some("https://i.imgur.com/Zz9m.gif"));
    assert_eq!(result.deletion_url, None);
    assert!(shell.urls.lock().unwrap().is_empty(), "a signed-in upload does not open the browser");
}

#[test]
fn imgur_without_a_sign_in_stays_anonymous_and_never_opens_the_browser() {
    let server = Server::start(vec![Canned::ok(json!({"data": {"link": "https://i.imgur.com/n.png"}}).to_string())]);
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config("http://127.0.0.1:9/token".into()), Arc::new(FakeSecretStore::default()), shell.clone(), "laqta/imgur");
    let host = Imgur::new("test-id", Some(oauth), server.url(""));

    block_on(host.upload(request("n.png", b"x", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    assert_eq!(recorded[0].header("authorization"), Some("Client-ID test-id"));
    assert!(shell.urls.lock().unwrap().is_empty());
}

#[test]
fn a_401_is_an_auth_error_and_drops_the_signed_in_token() {
    let server = Server::start(vec![Canned::new(401, r#"{"data":{"error":"expired"}}"#)]);
    let (oauth, secrets, _shell) = signed_in("laqta/imgur", &stored("imgur-token", None, None), "unused");
    let host = Imgur::new("test-id", Some(oauth.clone()), server.url(""));

    let error = block_on(host.upload(request("a.png", b"x", "image/png"), progress())).unwrap_err();
    server.finish();

    match &error {
        UploadError::Auth(message) => assert!(message.contains("401"), "{message}"),
        other => panic!("expected Auth, got {other:?}"),
    }
    assert!(secrets.get("laqta/imgur").unwrap().is_none(), "the stored token is dropped");
    assert!(!oauth.is_signed_in(), "the session token is dropped");
}

#[test]
fn rate_limits_and_server_errors_are_retryable_server_errors() {
    for status in [429_u16, 500, 503] {
        let server = Server::start(vec![Canned::new(status, "later")]);
        let host = Imgur::new("test-id", None, server.url(""));

        let error = block_on(host.upload(request("a.png", b"x", "image/png"), progress())).unwrap_err();
        server.finish();

        assert!(matches!(&error, UploadError::Server { status: got, body } if *got == status && body == "later"), "{error:?}");
        assert!(error.is_retryable(), "{error:?}");
    }
}

#[test]
fn other_client_errors_are_bad_responses_and_are_not_retried() {
    let server = Server::start(vec![Canned::new(404, "no such endpoint")]);
    let host = Imgur::new("test-id", None, server.url(""));

    let error = block_on(host.upload(request("a.png", b"x", "image/png"), progress())).unwrap_err();
    server.finish();

    assert!(matches!(&error, UploadError::BadResponse(message) if message.contains("no such endpoint")), "{error:?}");
    assert!(!error.is_retryable());
}

#[test]
fn dropbox_uploads_to_the_laqta_folder_then_shares_the_file() {
    let server = Server::start(vec![
        Canned::ok(json!({"name": "Laqta_1.png", "path_display": "/Laqta/Laqta_1.png", "id": "id:1"}).to_string()),
        Canned::ok(json!({"url": "https://www.dropbox.com/s/abc/Laqta_1.png?dl=0"}).to_string()),
    ]);
    let (oauth, _secrets, _shell) = signed_in("laqta/dropbox", &stored("db-token", None, None), "unused");
    let host = Dropbox::new(oauth, server.url(""), server.url(""), false);

    let result = block_on(host.upload(request("Laqta_1.png", b"PNG", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    let upload = &recorded[0];
    assert_eq!((upload.method.as_str(), upload.target.as_str()), ("POST", "/2/files/upload"));
    assert_eq!(upload.header("authorization"), Some("Bearer db-token"));
    assert_eq!(upload.header("content-type"), Some("application/octet-stream"));
    assert_eq!(upload.body, b"PNG".to_vec());
    let arg = upload.header("dropbox-api-arg").unwrap();
    assert_eq!(json_of(arg.as_bytes()), json!({"path": "/Laqta/Laqta_1.png", "mode": "add", "autorename": true}));

    let share = &recorded[1];
    assert_eq!((share.method.as_str(), share.target.as_str()), ("POST", "/2/sharing/create_shared_link_with_settings"));
    assert_eq!(share.header("authorization"), Some("Bearer db-token"));
    assert_eq!(json_of(&share.body), json!({"path": "/Laqta/Laqta_1.png"}));

    assert_eq!(result.url.as_deref(), Some("https://www.dropbox.com/s/abc/Laqta_1.png?dl=0"));
    assert_eq!(result.thumbnail_url, None);
    assert_eq!(result.deletion_url, None);
}

#[test]
fn dropbox_direct_links_replace_dl_zero_with_raw_one() {
    let cases = [
        ("https://www.dropbox.com/s/abc/shot.png?dl=0", "https://www.dropbox.com/s/abc/shot.png?raw=1"),
        ("https://www.dropbox.com/scl/fi/xyz/shot.png?rlkey=k1&dl=0", "https://www.dropbox.com/scl/fi/xyz/shot.png?rlkey=k1&raw=1"),
    ];
    for (share_url, expected) in cases {
        let server = Server::start(vec![
            Canned::ok(json!({"path_display": "/Laqta/shot.png"}).to_string()),
            Canned::ok(json!({"url": share_url}).to_string()),
        ]);
        let (oauth, _secrets, _shell) = signed_in("laqta/dropbox", &stored("db-token", None, None), "unused");
        let host = Dropbox::new(oauth, server.url(""), server.url(""), true);

        let result = block_on(host.upload(request("shot.png", b"x", "image/png"), progress())).unwrap();
        server.finish();

        assert_eq!(result.url.as_deref(), Some(expected));
    }
}

#[test]
fn dropbox_escapes_non_ascii_names_in_the_api_arg_header() {
    let server = Server::start(vec![
        Canned::ok(json!({"path_display": "/Laqta/café 🎉.png"}).to_string()),
        Canned::ok(json!({"url": "https://www.dropbox.com/s/x/y?dl=0"}).to_string()),
    ]);
    let (oauth, _secrets, _shell) = signed_in("laqta/dropbox", &stored("db-token", None, None), "unused");
    let host = Dropbox::new(oauth, server.url(""), server.url(""), false);

    block_on(host.upload(request("café 🎉.png", b"x", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    let arg = recorded[0].header("dropbox-api-arg").unwrap();
    assert!(arg.is_ascii(), "{arg}");
    assert_eq!(json_of(arg.as_bytes())["path"], "/Laqta/café 🎉.png");
}

#[test]
fn dropbox_refreshes_an_expired_token_before_it_uploads() {
    let server = Server::start(vec![
        Canned::ok(token_reply("fresh-token", None, Some(3600))),
        Canned::ok(json!({"path_display": "/Laqta/a.png"}).to_string()),
        Canned::ok(json!({"url": "https://www.dropbox.com/s/a/a.png?dl=0"}).to_string()),
    ]);
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set("laqta/dropbox", &stored("old-token", Some("refresh"), Some(NOW - 5))).unwrap();
    let shell = Arc::new(FakeShell::default());
    let oauth = make_oauth(config(server.url("/oauth2/token")), secrets, shell.clone(), "laqta/dropbox");
    let host = Dropbox::new(oauth, server.url(""), server.url(""), false);

    block_on(host.upload(request("a.png", b"x", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    assert_eq!(recorded[0].target, "/oauth2/token");
    assert_eq!(recorded[1].header("authorization"), Some("Bearer fresh-token"));
    assert!(shell.urls.lock().unwrap().is_empty());
}

#[test]
fn google_drive_sends_one_related_multipart_and_opens_a_public_link() {
    let server = Server::start(vec![
        Canned::ok(json!({"id": "file123", "name": "shot.png"}).to_string()),
        Canned::ok(json!({"id": "perm1", "role": "reader", "type": "anyone"}).to_string()),
    ]);
    let (oauth, _secrets, _shell) = signed_in("laqta/google-drive", &stored("g-token", None, None), "unused");
    let host = GoogleDrive::new(oauth, server.url(""), true);

    let result = block_on(host.upload(request("shot.png", b"PNG-DATA", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    let upload = &recorded[0];
    assert_eq!((upload.method.as_str(), upload.target.as_str()), ("POST", "/upload/drive/v3/files?uploadType=multipart"));
    assert_eq!(upload.header("authorization"), Some("Bearer g-token"));
    let content_type = upload.header("content-type").unwrap();
    let boundary = content_type.strip_prefix("multipart/related; boundary=").expect("a related boundary");
    let expected = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{{\"name\":\"shot.png\"}}\r\n--{boundary}\r\nContent-Type: image/png\r\n\r\nPNG-DATA\r\n--{boundary}--\r\n"
    );
    assert_eq!(upload.body, expected.as_bytes());

    let permission = &recorded[1];
    assert_eq!((permission.method.as_str(), permission.target.as_str()), ("POST", "/drive/v3/files/file123/permissions"));
    assert_eq!(json_of(&permission.body), json!({"role": "reader", "type": "anyone"}));

    assert_eq!(result.url.as_deref(), Some("https://drive.google.com/file/d/file123/view"));
}

#[test]
fn google_drive_private_upload_makes_no_permission_call() {
    let server = Server::start(vec![Canned::ok(json!({"id": "f9"}).to_string())]);
    let (oauth, _secrets, _shell) = signed_in("laqta/google-drive", &stored("g-token", None, None), "unused");
    let host = GoogleDrive::new(oauth, server.url(""), false);

    let result = block_on(host.upload(request("p.png", b"x", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    assert_eq!(recorded.len(), 1);
    assert_eq!(result.url.as_deref(), Some("https://drive.google.com/file/d/f9/view"));
}

#[test]
fn onedrive_puts_the_file_under_laqta_and_creates_a_view_link() {
    let server = Server::start(vec![
        Canned::new(201, json!({"id": "01ABC"}).to_string()),
        Canned::new(201, json!({"link": {"webUrl": "https://1drv.ms/u/s!abc", "type": "view", "scope": "anonymous"}}).to_string()),
    ]);
    let (oauth, _secrets, _shell) = signed_in("laqta/onedrive", &stored("od-token", None, None), "unused");
    let host = OneDrive::new(oauth, server.url(""));

    let result = block_on(host.upload(request("My shot #1.png", b"PNG", "image/png"), progress())).unwrap();
    let recorded = server.finish();

    let put = &recorded[0];
    assert_eq!(put.method, "PUT");
    assert_eq!(put.target, "/v1.0/me/drive/root:/Laqta/My%20shot%20%231.png:/content");
    assert_eq!(put.header("authorization"), Some("Bearer od-token"));
    assert_eq!(put.header("content-type"), Some("image/png"));
    assert_eq!(put.body, b"PNG".to_vec());

    let link = &recorded[1];
    assert_eq!((link.method.as_str(), link.target.as_str()), ("POST", "/v1.0/me/drive/items/01ABC/createLink"));
    assert_eq!(json_of(&link.body), json!({"type": "view", "scope": "anonymous"}));

    assert_eq!(result.url.as_deref(), Some("https://1drv.ms/u/s!abc"));
}

#[test]
fn bitly_shortens_with_the_stored_token() {
    let server = Server::start(vec![Canned::new(201, json!({"link": "https://bit.ly/3abc"}).to_string())]);
    let secrets = Arc::new(FakeSecretStore::default());
    secrets.set("laqta/bitly", "bt-token\n").unwrap();
    let host = Bitly::new(None, secrets, server.url(""));

    let short = block_on(host.shorten("https://example.test/long")).unwrap();
    let recorded = server.finish();

    assert_eq!(short, "https://bit.ly/3abc");
    assert_eq!((recorded[0].method.as_str(), recorded[0].target.as_str()), ("POST", "/v4/shorten"));
    assert_eq!(recorded[0].header("authorization"), Some("Bearer bt-token"));
    assert_eq!(json_of(&recorded[0].body), json!({"long_url": "https://example.test/long"}));
    assert_eq!(host.id(), "bitly");
}

#[test]
fn bitly_with_a_sign_in_uses_its_access_token() {
    let server = Server::start(vec![Canned::ok(json!({"link": "https://bit.ly/9"}).to_string())]);
    let (oauth, secrets, _shell) = signed_in("laqta/bitly", &stored("bitly-oauth", None, None), "unused");
    let host = Bitly::new(Some(oauth), secrets, server.url(""));

    assert_eq!(block_on(host.shorten("https://example.test/")).unwrap(), "https://bit.ly/9");
    assert_eq!(server.finish()[0].header("authorization"), Some("Bearer bitly-oauth"));
}

#[test]
fn bitly_without_a_token_is_an_auth_error_and_sends_nothing() {
    let server = Server::start(Vec::new());
    let host = Bitly::new(None, Arc::new(FakeSecretStore::default()), server.url(""));

    let error = block_on(host.shorten("https://example.test/")).unwrap_err();

    assert!(server.finish().is_empty());
    assert!(matches!(error, UploadError::Auth(_)), "{error:?}");
}

//! MediaFire over its REST API: sign in with
//! `user/get_session_token.php` (token version 2, signed with the SHA-1 of e-mail, password,
//! application id and API key), upload with `upload/simple.php` (multipart, the file as `Filedata`),
//! then poll `upload/poll_upload.php` once a second until `doupload.status` is 99 and read the
//! `quickkey`. Every call after sign-in carries `signature = md5((secret_key % 256) + time + path?query)`;
//! a reply with `new_key = "yes"` advances the key (`key * 16807 % 2147483647`).

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use laqta_http::{Body, Method, Part, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use md5::Md5;
use sha1::{Digest, Sha1};

use crate::hosts::{call, data_bytes, port_error, upload_call};

pub const API_BASE: &str = "https://www.mediafire.com/api/";
pub const API_KEY_KEY: &str = "laqta/mediafire/api_key";
pub const PASSWORD_KEY: &str = "laqta/mediafire/password";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MediaFireConfig {
    pub api_base: String,
    pub app_id: String,
    pub email: String,
    /// The folder path on MediaFire; empty = the root.
    pub folder: String,
    /// How long to wait between status polls (1 s in production).
    pub poll_interval: Duration,
}

#[derive(Clone)]
pub struct MediaFire {
    config: MediaFireConfig,
    secrets: Arc<dyn SecretStore>,
}

impl MediaFire {
    pub fn new(config: MediaFireConfig, secrets: Arc<dyn SecretStore>) -> Self {
        MediaFire { config, secrets }
    }
}

impl fmt::Debug for MediaFire {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("MediaFire").field("config", &self.config).finish_non_exhaustive()
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The sign-in signature: the SHA-1 of e-mail, password, application id and API key, concatenated
/// with no separators, as lowercase hex.
pub fn signature(email: &str, password: &str, app_id: &str, api_key: &str) -> String {
    hex(&Sha1::digest(format!("{email}{password}{app_id}{api_key}").as_bytes()))
}

/// The request signature for a call after sign-in. `args` are in request order, unescaped.
pub fn call_signature(secret_key: i64, time: &str, path: &str, args: &[(String, String)]) -> String {
    let query = args.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("&");
    let s = format!("{}{time}/api/{path}?{query}", secret_key % 256);
    hex(&Md5::digest(s.as_bytes()))
}

/// The next secret key after a reply says `new_key = "yes"`.
pub fn next_key(key: i64) -> i64 {
    (key * 16807) % 2_147_483_647
}

struct Session {
    token: String,
    time: String,
    key: i64,
}

fn response(body: &str) -> Result<serde_json::Value, UploadError> {
    let v: serde_json::Value = serde_json::from_str(body).map_err(|e| UploadError::BadResponse(format!("MediaFire: {e}")))?;
    let r = v.get("response").cloned().ok_or_else(|| UploadError::BadResponse("MediaFire: no response object".into()))?;
    if r.get("result").and_then(|x| x.as_str()) != Some("Success") {
        let msg = r.get("message").and_then(|x| x.as_str()).unwrap_or("the request failed");
        return Err(UploadError::BadResponse(format!("MediaFire: {msg}")));
    }
    Ok(r)
}

impl Session {
    /// Signs `args`, and advances the key when the reply asks for it.
    fn signed(&self, path: &str, mut args: Vec<(String, String)>) -> Vec<(String, String)> {
        let sig = call_signature(self.key, &self.time, path, &args);
        args.push(("signature".into(), sig));
        args
    }
    fn after(&mut self, r: &serde_json::Value) {
        if r.get("new_key").and_then(|x| x.as_str()) == Some("yes") {
            self.key = next_key(self.key);
        }
    }
}

fn form(args: &[(String, String)]) -> Body {
    Body::Multipart(args.iter().map(|(k, v)| Part::Text { name: k.clone(), value: v.clone() }).collect())
}

async fn transfer(
    config: MediaFireConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let secret = |key: &str| -> Result<String, UploadError> {
        secrets
            .get(key)
            .map_err(port_error)?
            .filter(|v| !v.is_empty())
            .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {key}")))
    };
    let (api_key, password) = (secret(API_KEY_KEY)?, secret(PASSWORD_KEY)?);
    if config.email.is_empty() || config.app_id.is_empty() {
        return Err(UploadError::Auth("MediaFire needs an e-mail and an application ID".into()));
    }
    // Sign in.
    let mut login = Request::new(Method::Post, format!("{}user/get_session_token.php", config.api_base));
    login.body = form(&[
        ("email".into(), config.email.clone()),
        ("password".into(), password.clone()),
        ("application_id".into(), config.app_id.clone()),
        ("token_version".into(), "2".into()),
        ("response_format".into(), "json".into()),
        ("signature".into(), signature(&config.email, &password, &config.app_id, &api_key)),
    ]);
    let r = response(&call(login, None).await?.text())?;
    let mut session = Session {
        token: r.get("session_token").and_then(|x| x.as_str()).unwrap_or_default().to_owned(),
        time: r.get("time").and_then(|x| x.as_str()).unwrap_or_default().to_owned(),
        key: r.get("secret_key").and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok()))).unwrap_or(-1),
    };
    if session.token.is_empty() || session.time.is_empty() || session.key < 0 {
        return Err(UploadError::BadResponse("MediaFire: the sign-in reply is incomplete".into()));
    }
    // Upload.
    let UploadRequest { file_name, data, mime, .. } = request;
    let args = session.signed(
        "upload/simple.php",
        vec![
            ("session_token".into(), session.token.clone()),
            ("path".into(), config.folder.clone()),
            ("response_format".into(), "json".into()),
        ],
    );
    let mut up = Request::new(Method::Post, format!("{}upload/simple.php", config.api_base));
    up.query = args;
    up.body = Body::Multipart(vec![Part::File { name: "Filedata".into(), file_name: file_name.clone(), mime, data: data_bytes(data)? }]);
    let r = response(&upload_call(up, progress, None).await?.text())?;
    session.after(&r);
    let key = r
        .pointer("/doupload/key")
        .and_then(|x| x.as_str())
        .filter(|k| !k.is_empty())
        .ok_or_else(|| UploadError::BadResponse("MediaFire: no upload key".into()))?
        .to_owned();
    // Poll.
    for _ in 0..120 {
        let args = session.signed(
            "upload/poll_upload.php",
            vec![
                ("session_token".into(), session.token.clone()),
                ("key".into(), key.clone()),
                ("filename".into(), file_name.clone()),
                ("response_format".into(), "json".into()),
            ],
        );
        let mut poll = Request::new(Method::Post, format!("{}upload/poll_upload.php", config.api_base));
        poll.body = form(&args);
        let text = call(poll, None).await?.text();
        let r = response(&text)?;
        session.after(&r);
        let d = r.get("doupload").cloned().unwrap_or_default();
        if d.get("fileerror").is_some_and(|e| !e.is_null() && e.as_str() != Some("")) {
            let msg = d.get("description").and_then(|x| x.as_str()).unwrap_or("the upload failed");
            return Err(UploadError::BadResponse(format!("MediaFire: {msg}")));
        }
        let status = d.get("status").and_then(|x| x.as_i64().or_else(|| x.as_str().and_then(|s| s.parse().ok())));
        if status == Some(99) {
            let quick =
                d.get("quickkey").and_then(|x| x.as_str()).ok_or_else(|| UploadError::BadResponse("MediaFire: no quickkey".into()))?;
            return Ok(UploadResult {
                url: Some(format!("https://www.mediafire.com/file/{quick}")),
                response: text,
                ..UploadResult::default()
            });
        }
        laqta_http::sleep(config.poll_interval).await;
    }
    Err(UploadError::BadResponse("MediaFire: the upload did not finish in two minutes".into()))
}

impl Destination for MediaFire {
    fn id(&self) -> &str {
        "mediafire"
    }

    fn name(&self) -> &str {
        "MediaFire"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signatures_are_lowercase_hex_digests() {
        assert_eq!(signature("a", "b", "c", "d"), hex(&Sha1::digest(b"abcd")));
        // md5("42" + "123" + "/api/upload/simple.php?x=1&y=2"), key 298 % 256 = 42.
        let s = call_signature(298, "123", "upload/simple.php", &[("x".into(), "1".into()), ("y".into(), "2".into())]);
        assert_eq!(s, hex(&Md5::digest(b"42123/api/upload/simple.php?x=1&y=2")));
        assert_eq!(next_key(1), 16807);
        assert_eq!(next_key(2_147_483_646), (2_147_483_646i64 * 16807) % 2_147_483_647);
    }
}

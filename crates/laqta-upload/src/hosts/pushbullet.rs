//! Pushbullet file push. `POST /v2/upload-request` gives a file URL and an upload URL with its form
//! fields. The file is posted there as multipart, the file part last, and a `file` push then shares
//! it with the account's devices. The URL is the file's download link. The access token is read
//! from `SecretStore` under `laqta/pushbullet/access_token`.

use std::collections::BTreeMap;
use std::fmt;
use std::sync::Arc;

use laqta_http::{Body, Method, Part, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use serde::Deserialize;
use serde_json::Value;

use crate::hosts::{call, data_bytes, json, json_body, port_error, upload_call};

/// Where the access token lives in `SecretStore`.
pub const SECRET_KEY: &str = "laqta/pushbullet/access_token";
/// The production API root.
pub const PUSHBULLET_API: &str = "https://api.pushbullet.com";

/// One Pushbullet destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PushbulletConfig {
    /// The API root, `PUSHBULLET_API` in production.
    pub api_base: String,
    /// The device to push to. `None` pushes to every device.
    pub device_iden: Option<String>,
}

/// The Pushbullet destination.
#[derive(Clone)]
pub struct Pushbullet {
    config: PushbulletConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Pushbullet {
    pub fn new(config: PushbulletConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Pushbullet { config, secrets }
    }
}

impl fmt::Debug for Pushbullet {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Pushbullet").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Pushbullet {
    fn id(&self) -> &str {
        "pushbullet"
    }

    fn name(&self) -> &str {
        "Pushbullet"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: PushbulletConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let token = secret(secrets.as_ref(), SECRET_KEY)?;
    let api = config.api_base.trim_end_matches('/');
    let UploadRequest { file_name, data, mime, .. } = request;
    let data = data_bytes(data)?;

    let mut ask = Request::new(Method::Post, format!("{api}/v2/upload-request"));
    ask.headers = vec![access_token(&token)];
    ask.body = json_body(&serde_json::json!({ "file_name": file_name, "file_type": mime }));
    let reply: UploadRequestReply = json(&call(ask, None).await?)?;

    let mut parts: Vec<Part> = reply.data.iter().map(|(name, value)| Part::Text { name: name.clone(), value: field_text(value) }).collect();
    // The file goes last: the storage's form rules read the fields before it.
    parts.push(Part::File { name: "file".to_owned(), file_name: file_name.clone(), mime: mime.clone(), data });
    let mut upload = Request::new(Method::Post, reply.upload_url);
    upload.body = Body::Multipart(parts);
    upload_call(upload, progress, None).await?;

    let mut push = serde_json::json!({ "type": "file", "file_name": file_name, "file_type": mime, "file_url": reply.file_url });
    if let Some(device) = config.device_iden.filter(|device| !device.is_empty()) {
        push["device_iden"] = Value::String(device);
    }
    let mut http = Request::new(Method::Post, format!("{api}/v2/pushes"));
    http.headers = vec![access_token(&token)];
    http.body = json_body(&push);
    let response = call(http, None).await?;
    Ok(UploadResult { url: Some(reply.file_url), response: response.text(), ..UploadResult::default() })
}

#[derive(Deserialize)]
struct UploadRequestReply {
    file_url: String,
    upload_url: String,
    #[serde(default)]
    data: BTreeMap<String, Value>,
}

/// Pushbullet's own header for the token (not `Authorization`).
fn access_token(token: &str) -> (String, String) {
    ("Access-Token".to_owned(), token.to_owned())
}

/// A form field's value as text: strings as they are, anything else as its JSON.
fn field_text(value: &Value) -> String {
    value.as_str().map_or_else(|| value.to_string(), str::to_owned)
}

fn secret(store: &dyn SecretStore, key: &str) -> Result<String, UploadError> {
    store
        .get(key)
        .map_err(port_error)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {key}")))
}

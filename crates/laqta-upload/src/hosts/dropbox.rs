//! Dropbox: the file goes to `/Laqta/<name>`, then gets a shared link (ADR 014). It needs a sign-in.

use std::fmt::Write as _;
use std::sync::Arc;

use laqta_http::{Body, Method, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{bearer, call, data_bytes, json, json_body, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production API root for RPC calls.
pub const DROPBOX_API: &str = "https://api.dropboxapi.com";
/// The production root for content uploads.
pub const DROPBOX_CONTENT: &str = "https://content.dropboxapi.com";
/// Placeholder until the app is registered with Dropbox. The real app key comes from settings.
pub const DROPBOX_CLIENT_ID: &str = "laqta-dropbox-app-key";
/// The `SecretStore` key for the token.
pub const SECRET_KEY: &str = "laqta/dropbox";

/// Dropbox's endpoints. `token_access_type=offline` asks for a refresh token.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://www.dropbox.com/oauth2/authorize".to_owned(),
        token_url: "https://api.dropboxapi.com/oauth2/token".to_owned(),
        client_id: DROPBOX_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: Vec::new(),
        extra_auth_params: vec![("token_access_type".to_owned(), "offline".to_owned())],
    }
}

#[derive(Debug)]
pub struct Dropbox {
    oauth: Arc<OAuth>,
    api_base: String,
    content_base: String,
    direct_link: bool,
}

#[derive(Deserialize)]
struct Uploaded {
    path_display: String,
}

#[derive(Deserialize)]
struct Shared {
    url: String,
}

impl Dropbox {
    /// `direct_link` turns the shared page link into a direct-download link.
    pub fn new(oauth: Arc<OAuth>, api_base: String, content_base: String, direct_link: bool) -> Self {
        Dropbox { oauth, api_base, content_base, direct_link }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, .. } = request;
        let data = data_bytes(data)?;
        let authorization = bearer(&self.oauth.access_token().await?);
        let arg = json!({"path": format!("/Laqta/{file_name}"), "mode": "add", "autorename": true});
        let mut upload = Request::new(Method::Post, format!("{}/2/files/upload", self.content_base));
        upload.headers.push(authorization.clone());
        upload.headers.push(("Dropbox-API-Arg".to_owned(), api_arg(&arg)));
        upload.body = Body::Bytes { data, content_type: "application/octet-stream".to_owned() };
        let response = upload_call(upload, progress, Some(self.oauth.as_ref())).await?;
        let uploaded: Uploaded = json(&response)?;

        let mut share = Request::new(Method::Post, format!("{}/2/sharing/create_shared_link_with_settings", self.api_base));
        share.headers.push(authorization);
        share.body = json_body(&json!({"path": uploaded.path_display}));
        let shared_response = call(share, Some(self.oauth.as_ref())).await?;
        let shared: Shared = json(&shared_response)?;
        let url = if self.direct_link { direct_link(&shared.url) } else { shared.url };
        Ok(UploadResult { url: Some(url), thumbnail_url: None, deletion_url: None, response: shared_response.text() })
    }
}

/// The `Dropbox-API-Arg` header. Header values are ASCII, so every other character is escaped as
/// `\uXXXX`, and characters outside the BMP as a UTF-16 surrogate pair.
fn api_arg(value: &serde_json::Value) -> String {
    let mut escaped = String::new();
    for c in value.to_string().chars() {
        if c.is_ascii() {
            escaped.push(c);
        } else {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                let _ = write!(escaped, "\\u{unit:04x}");
            }
        }
    }
    escaped
}

/// The direct-download form of a shared link: `dl=0` is replaced by `raw=1`, and other parameters
/// stay as they are.
fn direct_link(share: &str) -> String {
    let Some((base, query)) = share.split_once('?') else {
        return format!("{share}?raw=1");
    };
    let mut params: Vec<&str> =
        query.split('&').filter(|param| !param.is_empty() && !param.starts_with("dl=") && !param.starts_with("raw=")).collect();
    params.push("raw=1");
    format!("{base}?{}", params.join("&"))
}

impl Destination for Dropbox {
    fn id(&self) -> &str {
        "dropbox"
    }

    fn name(&self) -> &str {
        "Dropbox"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

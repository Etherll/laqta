//! Box: one multipart upload into a folder, then an open shared link (ADR 014). It needs a sign-in
//! (OAuth2 through `oauth.rs`, with the client secret the app registration gives).

use std::sync::Arc;

use laqta_http::{Body, Method, Part, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{PATH_SEGMENT, bearer, call, data_bytes, json, json_body, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production API root (shared links, file metadata).
pub const BOX_API: &str = "https://api.box.com";
/// The production root for file uploads.
pub const BOX_UPLOAD: &str = "https://upload.box.com";
/// Placeholder until the app is registered with Box. The real client id comes from settings.
pub const BOX_CLIENT_ID: &str = "laqta-box-client-id";
/// The `SecretStore` key for the token.
pub const SECRET_KEY: &str = "laqta/box";

/// Box's endpoints. Box takes its scopes from the app settings, so none are asked for here.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://account.box.com/api/oauth2/authorize".to_owned(),
        token_url: "https://api.box.com/oauth2/token".to_owned(),
        client_id: BOX_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: Vec::new(),
        extra_auth_params: Vec::new(),
    }
}

#[derive(Debug)]
pub struct BoxCom {
    oauth: Arc<OAuth>,
    api_base: String,
    upload_base: String,
    folder_id: String,
    share: bool,
}

#[derive(Deserialize)]
struct Uploaded {
    entries: Vec<Entry>,
}

#[derive(Deserialize)]
struct Entry {
    id: String,
}

#[derive(Deserialize)]
struct Shared {
    shared_link: Option<SharedLink>,
}

#[derive(Deserialize)]
struct SharedLink {
    url: String,
}

impl BoxCom {
    /// `folder_id` is the folder the file goes to ("0" is the root). `share` creates an open link.
    pub fn new(oauth: Arc<OAuth>, api_base: String, upload_base: String, folder_id: String, share: bool) -> Self {
        BoxCom { oauth, api_base, upload_base, folder_id, share }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        let data = data_bytes(data)?;
        let authorization = bearer(&self.oauth.access_token().await?);
        let attributes = json!({"name": file_name, "parent": {"id": self.folder_id}}).to_string();

        let mut upload = Request::new(Method::Post, format!("{}/api/2.0/files/content", self.upload_base));
        upload.headers.push(authorization.clone());
        upload.body = Body::Multipart(vec![
            Part::Text { name: "attributes".to_owned(), value: attributes },
            Part::File { name: "file".to_owned(), file_name, mime, data },
        ]);
        let response = upload_call(upload, progress, Some(self.oauth.as_ref())).await?;
        let uploaded: Uploaded = json(&response)?;
        let id = uploaded
            .entries
            .first()
            .map(|entry| utf8_encode(&entry.id))
            .ok_or_else(|| UploadError::BadResponse("Box reply has no uploaded file".to_owned()))?;

        let url = if self.share {
            let mut share = Request::new(Method::Put, format!("{}/2.0/files/{id}", self.api_base));
            share.query = vec![("fields".to_owned(), "shared_link".to_owned())];
            share.headers.push(authorization);
            share.body = json_body(&json!({"shared_link": {"access": "open"}}));
            let reply = call(share, Some(self.oauth.as_ref())).await?;
            let shared: Shared = json(&reply)?;
            shared.shared_link.map(|link| link.url).ok_or_else(|| UploadError::BadResponse("Box reply has no shared link".to_owned()))?
        } else {
            format!("https://app.box.com/file/{id}")
        };
        Ok(UploadResult { url: Some(url), thumbnail_url: None, deletion_url: None, response: response.text() })
    }
}

fn utf8_encode(value: &str) -> String {
    percent_encoding::utf8_percent_encode(value, PATH_SEGMENT).to_string()
}

impl Destination for BoxCom {
    fn id(&self) -> &str {
        "box"
    }

    fn name(&self) -> &str {
        "Box"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

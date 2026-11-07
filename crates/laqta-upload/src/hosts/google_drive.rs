//! Google Drive: one `multipart/related` upload, then an "anyone with the link" permission when the
//! file is public (ADR 014). It needs a sign-in.

use std::sync::Arc;

use laqta_http::{Body, Method, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{PATH_SEGMENT, bearer, boundary_for, call, data_bytes, json, json_body, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production root for both the upload and the Drive API.
pub const GOOGLE_API: &str = "https://www.googleapis.com";
/// Placeholder until the app is registered with Google. The real client id comes from settings.
pub const GOOGLE_DRIVE_CLIENT_ID: &str = "laqta-google-drive-client-id";
/// The `SecretStore` key for the token.
pub const SECRET_KEY: &str = "laqta/google-drive";

/// Google's endpoints. `drive.file` limits access to the files the app creates. `access_type=offline`
/// with `prompt=consent` asks for a refresh token.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://accounts.google.com/o/oauth2/v2/auth".to_owned(),
        token_url: "https://oauth2.googleapis.com/token".to_owned(),
        client_id: GOOGLE_DRIVE_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: vec!["https://www.googleapis.com/auth/drive.file".to_owned()],
        extra_auth_params: vec![("access_type".to_owned(), "offline".to_owned()), ("prompt".to_owned(), "consent".to_owned())],
    }
}

#[derive(Debug)]
pub struct GoogleDrive {
    oauth: Arc<OAuth>,
    base_url: String,
    public: bool,
}

#[derive(Deserialize)]
struct Created {
    id: String,
}

impl GoogleDrive {
    pub fn new(oauth: Arc<OAuth>, base_url: String, public: bool) -> Self {
        GoogleDrive { oauth, base_url, public }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        let data = data_bytes(data)?;
        let authorization = bearer(&self.oauth.access_token().await?);
        let metadata = json!({"name": file_name}).to_string();
        let boundary = boundary_for(&[metadata.as_bytes(), &data]);
        let body = related_body(&boundary, &metadata, &mime, &data);

        let mut upload = Request::new(Method::Post, format!("{}/upload/drive/v3/files", self.base_url));
        upload.query = vec![("uploadType".to_owned(), "multipart".to_owned())];
        upload.headers.push(authorization.clone());
        upload.body = Body::Bytes { data: body, content_type: format!("multipart/related; boundary={boundary}") };
        let response = upload_call(upload, progress, Some(self.oauth.as_ref())).await?;
        let created: Created = json(&response)?;
        let id = utf8_percent_encode(&created.id, PATH_SEGMENT);

        if self.public {
            let mut share = Request::new(Method::Post, format!("{}/drive/v3/files/{id}/permissions", self.base_url));
            share.headers.push(authorization);
            share.body = json_body(&json!({"role": "reader", "type": "anyone"}));
            call(share, Some(self.oauth.as_ref())).await?;
        }
        Ok(UploadResult {
            url: Some(format!("https://drive.google.com/file/d/{id}/view")),
            thumbnail_url: None,
            deletion_url: None,
            response: response.text(),
        })
    }
}

/// The `multipart/related` body: the metadata JSON part, then the media part. Parts are separated
/// by CRLF and the body ends with the closing delimiter.
fn related_body(boundary: &str, metadata: &str, mime: &str, data: &[u8]) -> Vec<u8> {
    let head = format!(
        "--{boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n{metadata}\r\n--{boundary}\r\nContent-Type: {mime}\r\n\r\n"
    );
    let tail = format!("\r\n--{boundary}--\r\n");
    let mut body = Vec::with_capacity(head.len() + data.len() + tail.len());
    body.extend_from_slice(head.as_bytes());
    body.extend_from_slice(data);
    body.extend_from_slice(tail.as_bytes());
    body
}

impl Destination for GoogleDrive {
    fn id(&self) -> &str {
        "google-drive"
    }

    fn name(&self) -> &str {
        "Google Drive"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

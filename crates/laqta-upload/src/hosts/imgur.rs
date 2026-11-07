//! Imgur (`POST /3/image`). An anonymous upload uses the app's Client-ID. When the account is
//! signed in, the upload goes to that account instead (ADR 014).

use std::sync::Arc;

use laqta_http::{Body, Method, Part, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use serde::Deserialize;

use crate::hosts::{bearer, data_bytes, json, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production API root.
pub const IMGUR_API: &str = "https://api.imgur.com";
/// Placeholder until the app is registered with Imgur. The real value comes from settings.
pub const IMGUR_CLIENT_ID: &str = "laqta-imgur-client-id";
/// The `SecretStore` key for the signed-in account's token.
pub const SECRET_KEY: &str = "laqta/imgur";

/// Imgur's endpoints for a signed-in account. The client secret is not set (see the report).
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://api.imgur.com/oauth2/authorize".to_owned(),
        token_url: "https://api.imgur.com/oauth2/token".to_owned(),
        client_id: IMGUR_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: Vec::new(),
        extra_auth_params: Vec::new(),
    }
}

#[derive(Debug)]
pub struct Imgur {
    client_id: String,
    oauth: Option<Arc<OAuth>>,
    base_url: String,
}

#[derive(Deserialize)]
struct Envelope {
    data: Image,
}

#[derive(Deserialize)]
struct Image {
    link: String,
    deletehash: Option<String>,
}

impl Imgur {
    /// `oauth` is the account's sign-in. It is used only while it holds a token, so an anonymous
    /// upload never opens the browser.
    pub fn new(client_id: impl Into<String>, oauth: Option<Arc<OAuth>>, base_url: String) -> Self {
        Imgur { client_id: client_id.into(), oauth, base_url }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        let data = data_bytes(data)?;
        let (authorization, account) = match self.oauth.as_deref().filter(|oauth| oauth.is_signed_in()) {
            Some(oauth) => (bearer(&oauth.access_token().await?), Some(oauth)),
            None => (("Authorization".to_owned(), format!("Client-ID {}", self.client_id)), None),
        };
        let mut upload = Request::new(Method::Post, format!("{}/3/image", self.base_url));
        upload.headers.push(authorization);
        upload.body = Body::Multipart(vec![Part::File { name: "image".to_owned(), file_name, mime, data }]);
        let response = upload_call(upload, progress, account).await?;
        let envelope: Envelope = json(&response)?;
        let image = envelope.data;
        let thumbnail_url = thumbnail(&image.link);
        Ok(UploadResult {
            url: Some(image.link),
            thumbnail_url,
            deletion_url: image.deletehash.map(|hash| format!("https://imgur.com/delete/{hash}")),
            response: response.text(),
        })
    }
}

/// The small square thumbnail: `m` goes before the extension of the last path segment, so
/// `https://i.imgur.com/AbC.png` becomes `https://i.imgur.com/AbCm.png`.
fn thumbnail(link: &str) -> Option<String> {
    let name_start = link.rfind('/')? + 1;
    let dot = name_start + link[name_start..].rfind('.')?;
    Some(format!("{}m{}", &link[..dot], &link[dot..]))
}

impl Destination for Imgur {
    fn id(&self) -> &str {
        "imgur"
    }

    fn name(&self) -> &str {
        "Imgur"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

//! OneDrive (Microsoft Graph): the file goes to `/Laqta/<name>`, then gets an anonymous view link
//! (ADR 014). It needs a sign-in.

use std::sync::Arc;

use laqta_http::{Body, Method, Request};
use laqta_ports::{BoxFuture, Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{PATH_SEGMENT, bearer, call, data_bytes, json, json_body, upload_call};
use crate::oauth::{OAuth, OAuthConfig};

/// The production Microsoft Graph root.
pub const ONEDRIVE_API: &str = "https://graph.microsoft.com";
/// Placeholder until the app is registered with Microsoft. The real client id comes from settings.
pub const ONEDRIVE_CLIENT_ID: &str = "laqta-onedrive-client-id";
/// The `SecretStore` key for the token.
pub const SECRET_KEY: &str = "laqta/onedrive";

/// Microsoft's endpoints. `offline_access` asks for a refresh token.
pub fn oauth_config() -> OAuthConfig {
    OAuthConfig {
        auth_url: "https://login.microsoftonline.com/common/oauth2/v2.0/authorize".to_owned(),
        token_url: "https://login.microsoftonline.com/common/oauth2/v2.0/token".to_owned(),
        client_id: ONEDRIVE_CLIENT_ID.to_owned(),
        client_secret: None,
        scopes: vec!["Files.ReadWrite".to_owned(), "offline_access".to_owned()],
        extra_auth_params: Vec::new(),
    }
}

#[derive(Debug)]
pub struct OneDrive {
    oauth: Arc<OAuth>,
    base_url: String,
}

#[derive(Deserialize)]
struct Item {
    id: String,
}

#[derive(Deserialize)]
struct Created {
    link: Link,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Link {
    web_url: String,
}

impl OneDrive {
    pub fn new(oauth: Arc<OAuth>, base_url: String) -> Self {
        OneDrive { oauth, base_url }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        let data = data_bytes(data)?;
        let authorization = bearer(&self.oauth.access_token().await?);
        let name = utf8_percent_encode(&file_name, PATH_SEGMENT);

        let mut upload = Request::new(Method::Put, format!("{}/v1.0/me/drive/root:/Laqta/{name}:/content", self.base_url));
        upload.headers.push(authorization.clone());
        upload.body = Body::Bytes { data, content_type: mime };
        let response = upload_call(upload, progress, Some(self.oauth.as_ref())).await?;
        let item: Item = json(&response)?;
        let id = utf8_percent_encode(&item.id, PATH_SEGMENT);

        let mut link = Request::new(Method::Post, format!("{}/v1.0/me/drive/items/{id}/createLink", self.base_url));
        link.headers.push(authorization);
        link.body = json_body(&json!({"type": "view", "scope": "anonymous"}));
        let created_response = call(link, Some(self.oauth.as_ref())).await?;
        let created: Created = json(&created_response)?;
        Ok(UploadResult { url: Some(created.link.web_url), thumbnail_url: None, deletion_url: None, response: created_response.text() })
    }
}

impl Destination for OneDrive {
    fn id(&self) -> &str {
        "onedrive"
    }

    fn name(&self) -> &str {
        "OneDrive"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

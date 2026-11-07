//! GitHub Gist (`POST /gists`). Text uploads only: the upload becomes the one file of a new gist,
//! and the gist's page is the URL. The token is a personal access token with the `gist` scope,
//! read from `SecretStore` under `laqta/gist/token`.

use std::fmt;
use std::sync::Arc;

use laqta_http::{Method, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadKind, UploadRequest, UploadResult};
use serde::Deserialize;

use crate::hosts::{bearer, call, data_bytes, json, json_body, port_error};

/// Where the token lives in `SecretStore`.
pub const SECRET_KEY: &str = "laqta/gist/token";
/// The production API root.
pub const GITHUB_API: &str = "https://api.github.com";

/// One Gist destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GistConfig {
    /// The API root, `GITHUB_API` in production.
    pub api_base: String,
    /// A public gist (listed and searchable) rather than a secret one.
    pub public: bool,
}

/// The GitHub Gist destination.
#[derive(Clone)]
pub struct Gist {
    config: GistConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Gist {
    pub fn new(config: GistConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Gist { config, secrets }
    }
}

impl fmt::Debug for Gist {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gist").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Gist {
    fn id(&self) -> &str {
        "gist"
    }

    fn name(&self) -> &str {
        "GitHub Gist"
    }

    fn upload(&self, request: UploadRequest, _progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request))
    }
}

async fn transfer(config: GistConfig, secrets: Arc<dyn SecretStore>, request: UploadRequest) -> Result<UploadResult, UploadError> {
    if request.kind != UploadKind::Text {
        return Err(UploadError::Unsupported("a GitHub gist takes text uploads only".to_owned()));
    }
    let name = request.file_name;
    let text = String::from_utf8(data_bytes(request.data)?).map_err(|_| UploadError::Unsupported("the text is not UTF-8".to_owned()))?;
    let token = secret(secrets.as_ref(), SECRET_KEY)?;

    let mut files = serde_json::Map::new();
    files.insert(name, serde_json::json!({ "content": text }));
    let body = serde_json::json!({ "public": config.public, "files": files });

    let mut http = Request::new(Method::Post, format!("{}/gists", config.api_base.trim_end_matches('/')));
    http.headers = vec![bearer(&token), ("Accept".to_owned(), "application/vnd.github+json".to_owned())];
    http.body = json_body(&body);
    let response = call(http, None).await?;
    let created: Created = json(&response)?;
    Ok(UploadResult { url: Some(created.html_url), response: response.text(), ..UploadResult::default() })
}

#[derive(Deserialize)]
struct Created {
    html_url: String,
}

fn secret(store: &dyn SecretStore, key: &str) -> Result<String, UploadError> {
    store
        .get(key)
        .map_err(port_error)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {key}")))
}

//! Bitly (`POST /v4/shorten`). The token is the signed-in account's when an OAuth sign-in is given,
//! and otherwise the token kept under `laqta/bitly` (ADR 014).

use std::fmt;
use std::sync::Arc;

use laqta_http::{Method, Request};
use laqta_ports::{BoxFuture, PortError, SecretStore, Shortener, UploadError};
use serde::Deserialize;
use serde_json::json;

use crate::hosts::{bearer, call, json, json_body, port_error};
use crate::oauth::{OAuth, Token};

/// The production API root.
pub const BITLY_API: &str = "https://api-ssl.bitly.com";
/// The `SecretStore` key for a pasted token. The entry may also hold `Token` JSON from a sign-in.
pub const SECRET_KEY: &str = "laqta/bitly";

pub struct Bitly {
    oauth: Option<Arc<OAuth>>,
    secrets: Arc<dyn SecretStore>,
    base_url: String,
}

impl fmt::Debug for Bitly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Bitly").field("base_url", &self.base_url).finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Shortened {
    link: String,
}

impl Bitly {
    pub fn new(oauth: Option<Arc<OAuth>>, secrets: Arc<dyn SecretStore>, base_url: String) -> Self {
        Bitly { oauth, secrets, base_url }
    }

    /// The bearer header from the sign-in, else from the stored entry (JSON with an access token,
    /// or the bare token).
    async fn authorization(&self) -> Result<(String, String), UploadError> {
        if let Some(oauth) = self.oauth.as_deref() {
            return Ok(bearer(&oauth.access_token().await?));
        }
        let stored = match self.secrets.get(SECRET_KEY) {
            Ok(Some(stored)) => stored,
            Ok(None) => return Err(UploadError::Auth("no Bitly token is stored".into())),
            Err(PortError::Unsupported(message)) => return Err(UploadError::Unsupported(message)),
            Err(error) => return Err(port_error(error)),
        };
        let token = match serde_json::from_str::<Token>(&stored) {
            Ok(token) => token.access_token,
            Err(_) => stored.trim().to_owned(),
        };
        Ok(bearer(&token))
    }

    async fn run(&self, long_url: &str) -> Result<String, UploadError> {
        let authorization = self.authorization().await?;
        let mut request = Request::new(Method::Post, format!("{}/v4/shorten", self.base_url));
        request.headers.push(authorization);
        request.body = json_body(&json!({"long_url": long_url}));
        let response = call(request, self.oauth.as_deref()).await?;
        let shortened: Shortened = json(&response)?;
        Ok(shortened.link)
    }
}

impl Shortener for Bitly {
    fn id(&self) -> &str {
        "bitly"
    }

    fn shorten(&self, url: &str) -> BoxFuture<'_, Result<String, UploadError>> {
        let url = url.to_owned();
        Box::pin(async move { self.run(&url).await })
    }
}

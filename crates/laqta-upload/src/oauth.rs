//! OAuth2 sign-in for the native hosts (ADR 014, ARCHITECTURE §6): the authorization-code flow with
//! PKCE (RFC 7636) over a loopback redirect on 127.0.0.1. The browser opens through `Shell`, the
//! listener runs on the HTTP runtime, and the token lives in `SecretStore` as JSON, or only in
//! memory when the platform has no keyring.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use laqta_http::{Body, Method, Request};
use laqta_ports::{Rng, SecretStore, Shell, UploadError};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::hosts::{http_error, port_error, status_error};

/// Seconds before expiry at which a token already counts as expired, so a request does not start
/// with a token that dies in flight.
const EXPIRY_SKEW: i64 = 60;

/// One provider's endpoints and this app's registration with it.
#[derive(Clone)]
pub struct OAuthConfig {
    pub auth_url: String,
    pub token_url: String,
    pub client_id: String,
    pub client_secret: Option<String>,
    pub scopes: Vec<String>,
    pub extra_auth_params: Vec<(String, String)>,
}

impl fmt::Debug for OAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuthConfig")
            .field("auth_url", &self.auth_url)
            .field("token_url", &self.token_url)
            .field("client_id", &self.client_id)
            .field("client_secret", &self.client_secret.as_ref().map(|_| "<redacted>"))
            .field("scopes", &self.scopes)
            .field("extra_auth_params", &self.extra_auth_params)
            .finish()
    }
}

/// A token as the token endpoint gave it. Its JSON is what `SecretStore` holds.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Token {
    pub access_token: String,
    pub refresh_token: Option<String>,
    /// Unix seconds. `None` means the token does not expire on its own.
    pub expires_at_unix: Option<i64>,
}

impl Token {
    fn expired(&self, now: i64) -> bool {
        self.expires_at_unix.is_some_and(|at| now.saturating_add(EXPIRY_SKEW) >= at)
    }
}

/// The token endpoint's reply. `refresh_token` is optional on refresh, so the old one is kept then.
#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// One sign-in to one provider. It hands out access tokens: from the session, from the store
/// (refreshed when expired), or through the browser when neither has one.
pub struct OAuth {
    config: OAuthConfig,
    secrets: Arc<dyn SecretStore>,
    shell: Arc<dyn Shell>,
    rng: Arc<dyn Rng>,
    /// The `SecretStore` key that holds the token JSON, e.g. `laqta/dropbox`.
    key: String,
    now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    session: Mutex<Option<Token>>,
}

impl fmt::Debug for OAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OAuth").field("key", &self.key).field("config", &self.config).finish_non_exhaustive()
    }
}

impl OAuth {
    pub fn new(
        config: OAuthConfig,
        secrets: Arc<dyn SecretStore>,
        shell: Arc<dyn Shell>,
        rng: Arc<dyn Rng>,
        key: impl Into<String>,
        now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        OAuth { config, secrets, shell, rng, key: key.into(), now_unix, session: Mutex::new(None) }
    }

    /// A valid access token. Uses the session's token, then the stored one (refreshed if it has
    /// expired), and otherwise runs the browser sign-in.
    pub async fn access_token(&self) -> Result<String, UploadError> {
        if let Some(token) = self.known_token() {
            if !token.expired((self.now_unix)()) {
                return Ok(token.access_token);
            }
            if let Some(refresh) = token.refresh_token.as_deref() {
                match self.refresh(refresh).await {
                    Ok(fresh) => {
                        self.remember(&fresh);
                        return Ok(fresh.access_token);
                    }
                    // The provider rejected the refresh token, so the sign-in must run again.
                    Err(UploadError::Auth(_)) => {
                        let _ = self.sign_out();
                    }
                    Err(error) => return Err(error),
                }
            }
        }
        let token = self.sign_in().await?;
        self.remember(&token);
        Ok(token.access_token)
    }

    /// Whether a token is held, without starting a sign-in.
    pub fn is_signed_in(&self) -> bool {
        self.known_token().is_some()
    }

    /// Forgets the token: the session's copy and the stored one. With no keyring there is no
    /// stored copy, so only the session is cleared.
    pub fn sign_out(&self) -> Result<(), UploadError> {
        *self.session() = None;
        match self.secrets.delete(&self.key) {
            Ok(()) | Err(laqta_ports::PortError::Unsupported(_)) => Ok(()),
            Err(error) => Err(port_error(error)),
        }
    }

    /// The session's token, else the stored one (which then fills the session).
    fn known_token(&self) -> Option<Token> {
        let cached: Option<Token> = self.session().clone();
        if cached.is_some() {
            return cached;
        }
        let stored = match self.secrets.get(&self.key) {
            Ok(Some(json)) => serde_json::from_str::<Token>(&json).ok(),
            _ => None,
        }?;
        *self.session() = Some(stored.clone());
        Some(stored)
    }

    /// Keeps the token for this session, and in the store when there is one.
    fn remember(&self, token: &Token) {
        *self.session() = Some(token.clone());
        let Ok(json) = serde_json::to_string(token) else {
            return;
        };
        match self.secrets.set(&self.key, &json) {
            Ok(()) | Err(laqta_ports::PortError::Unsupported(_)) => {}
            Err(error) => tracing::warn!(key = %self.key, %error, "the token is kept for this session only"),
        }
    }

    fn session(&self) -> MutexGuard<'_, Option<Token>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    async fn refresh(&self, refresh_token: &str) -> Result<Token, UploadError> {
        let form = vec![("grant_type".to_owned(), "refresh_token".to_owned()), ("refresh_token".to_owned(), refresh_token.to_owned())];
        self.token_request(form, Some(refresh_token)).await
    }

    /// The interactive flow: a loopback listener, the browser, and the code exchange.
    async fn sign_in(&self) -> Result<Token, UploadError> {
        let verifier = random_token(&*self.rng, 32);
        let state = random_token(&*self.rng, 16);
        // An empty reply page: the copy the browser shows after sign-in belongs to the UI.
        let (port, redirect) = laqta_http::loopback_once(String::new()).await.map_err(http_error)?;
        let redirect_uri = format!("http://127.0.0.1:{port}/callback");
        let auth_url = authorize_url(&self.config, &redirect_uri, &state, &pkce_challenge(&verifier))?;
        self.shell.open_url(&auth_url).map_err(port_error)?;
        let target = redirect.await.map_err(http_error)?;
        let code = authorized_code(&target, &state)?;
        let form = vec![
            ("grant_type".to_owned(), "authorization_code".to_owned()),
            ("code".to_owned(), code),
            ("redirect_uri".to_owned(), redirect_uri),
            ("code_verifier".to_owned(), verifier),
        ];
        self.token_request(form, None).await
    }

    /// Posts a form to the token endpoint. A reply without a refresh token keeps `fallback_refresh`.
    async fn token_request(&self, mut form: Vec<(String, String)>, fallback_refresh: Option<&str>) -> Result<Token, UploadError> {
        form.push(("client_id".to_owned(), self.config.client_id.clone()));
        if let Some(secret) = &self.config.client_secret {
            form.push(("client_secret".to_owned(), secret.clone()));
        }
        let mut request = Request::new(Method::Post, self.config.token_url.clone());
        request.headers.push(("Accept".to_owned(), "application/json".to_owned()));
        request.body = Body::Form(form);
        let response = laqta_http::send(request).await.map_err(http_error)?;
        if !(200..300).contains(&response.status) {
            // The token endpoint answers a rejected code or refresh token with 400.
            return Err(match response.status {
                400 | 401 | 403 => UploadError::Auth(response.text()),
                status => status_error(status, response.text()),
            });
        }
        // The body holds tokens, so a parse error does not quote it.
        let parsed: TokenResponse =
            serde_json::from_slice(&response.body).map_err(|error| UploadError::BadResponse(format!("token response: {error}")))?;
        let now = (self.now_unix)();
        Ok(Token {
            access_token: parsed.access_token,
            refresh_token: parsed.refresh_token.or_else(|| fallback_refresh.map(str::to_owned)),
            expires_at_unix: parsed.expires_in.map(|seconds| now.saturating_add(seconds)),
        })
    }
}

/// The S256 code challenge for a verifier (RFC 7636 §4.2).
pub fn pkce_challenge(verifier: &str) -> String {
    URL_SAFE_NO_PAD.encode(Sha256::digest(verifier.as_bytes()))
}

/// `bytes` random bytes from the injected generator, as unpadded URL-safe base64. Thirty-two bytes
/// make the 43-character verifier RFC 7636 asks for.
fn random_token(rng: &dyn Rng, bytes: usize) -> String {
    let mut buf = Vec::with_capacity(bytes + 8);
    while buf.len() < bytes {
        buf.extend_from_slice(&rng.next_u64().to_le_bytes());
    }
    buf.truncate(bytes);
    URL_SAFE_NO_PAD.encode(buf)
}

fn authorize_url(config: &OAuthConfig, redirect_uri: &str, state: &str, challenge: &str) -> Result<String, UploadError> {
    let mut url =
        url::Url::parse(&config.auth_url).map_err(|error| UploadError::Unsupported(format!("auth URL {}: {error}", config.auth_url)))?;
    {
        let mut query = url.query_pairs_mut();
        query.append_pair("response_type", "code").append_pair("client_id", &config.client_id).append_pair("redirect_uri", redirect_uri);
        if !config.scopes.is_empty() {
            query.append_pair("scope", &config.scopes.join(" "));
        }
        query.append_pair("state", state).append_pair("code_challenge", challenge).append_pair("code_challenge_method", "S256");
        for (key, value) in &config.extra_auth_params {
            query.append_pair(key, value);
        }
    }
    Ok(url.to_string())
}

/// The authorization code in the redirect target (`/callback?code=…&state=…`). The state is checked
/// first, so a forged redirect cannot supply an error or a code.
fn authorized_code(target: &str, expected_state: &str) -> Result<String, UploadError> {
    let url = url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|error| UploadError::BadResponse(format!("redirect target: {error}")))?;
    let (mut code, mut state, mut error, mut description) = (None, None, None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "code" => code = Some(value.into_owned()),
            "state" => state = Some(value.into_owned()),
            "error" => error = Some(value.into_owned()),
            "error_description" => description = Some(value.into_owned()),
            _ => {}
        }
    }
    if state.as_deref() != Some(expected_state) {
        return Err(UploadError::Auth("the sign-in redirect does not carry this sign-in's state".into()));
    }
    if let Some(error) = error {
        return Err(UploadError::Auth(description.unwrap_or(error)));
    }
    code.ok_or_else(|| UploadError::BadResponse("the sign-in redirect has no code".into()))
}

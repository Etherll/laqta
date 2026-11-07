//! Flickr (M7 D12): OAuth 1.0a with HMAC-SHA1 (RFC 5849). The first upload signs the user in through
//! the browser, with a loopback callback as the request token's `oauth_callback`. The access token
//! then lives in `SecretStore`. The upload is a multipart `POST` to `up.flickr.com` whose OAuth
//! parameters travel in the Authorization header (multipart fields are not in the base string), and
//! the page URL comes from `flickr.photos.getInfo`.

use std::fmt;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use laqta_http::{Body, Method, Part, Request};
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use laqta_ports::{BoxFuture, PortError, Rng, Shell};
use percent_encoding::utf8_percent_encode;
use serde::{Deserialize, Serialize};
use sha1::Sha1;

use crate::hosts::{PATH_SEGMENT, call, data_bytes, http_error, json, port_error, upload_call};

/// Where the access token lives in `SecretStore`.
pub const TOKEN_KEY: &str = "laqta/flickr/access_token";
/// Where the API secret lives in `SecretStore` (the `api_secret` field of the account).
pub const API_SECRET_KEY: &str = "laqta/flickr/api_secret";

/// Flickr's OAuth 1.0a endpoints, the upload and the REST API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlickrEndpoints {
    pub request_token: String,
    pub authorize: String,
    pub access_token: String,
    pub upload: String,
    pub rest: String,
}

impl FlickrEndpoints {
    pub fn production() -> Self {
        FlickrEndpoints {
            request_token: "https://www.flickr.com/services/oauth/request_token".to_owned(),
            authorize: "https://www.flickr.com/services/oauth/authorize".to_owned(),
            access_token: "https://www.flickr.com/services/oauth/access_token".to_owned(),
            upload: "https://up.flickr.com/services/upload/".to_owned(),
            rest: "https://api.flickr.com/services/rest/".to_owned(),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FlickrConfig {
    /// The API key (the consumer key of OAuth 1.0a).
    pub api_key: String,
    /// Uploads are public (`is_public=1`).
    pub public: bool,
    pub endpoints: FlickrEndpoints,
}

/// The access token: what Flickr's `access_token` reply gives. Its JSON is what `SecretStore` holds.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Token {
    pub oauth_token: String,
    pub oauth_token_secret: String,
}

impl fmt::Debug for Token {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Token").field("oauth_token", &self.oauth_token).field("oauth_token_secret", &"<redacted>").finish()
    }
}

/// The Flickr destination.
pub struct Flickr {
    config: FlickrConfig,
    secrets: Arc<dyn SecretStore>,
    shell: Arc<dyn Shell>,
    rng: Arc<dyn Rng>,
    now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    /// The token for this session, kept when the platform has no keyring.
    session: Mutex<Option<Token>>,
}

impl Flickr {
    /// `rng` must be cryptographically secure: it makes the nonces.
    pub fn new(
        config: FlickrConfig,
        secrets: Arc<dyn SecretStore>,
        shell: Arc<dyn Shell>,
        rng: Arc<dyn Rng>,
        now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    ) -> Self {
        Flickr { config, secrets, shell, rng, now_unix, session: Mutex::new(None) }
    }

    fn session(&self) -> MutexGuard<'_, Option<Token>> {
        self.session.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn now(&self) -> i64 {
        (self.now_unix)()
    }

    fn nonce(&self) -> String {
        format!("{:016x}{:016x}", self.rng.next_u64(), self.rng.next_u64())
    }

    fn consumer_secret(&self) -> Result<String, UploadError> {
        self.secrets
            .get(API_SECRET_KEY)
            .map_err(port_error)?
            .filter(|secret| !secret.is_empty())
            .ok_or_else(|| UploadError::Auth(format!("no Flickr API secret is stored under {API_SECRET_KEY}")))
    }

    /// The `oauth_*` parameters of one request, without the signature. `extra` adds
    /// `oauth_callback` or `oauth_verifier`.
    fn oauth_params(&self, token: Option<&str>, extra: &[(&str, String)]) -> Vec<(String, String)> {
        let mut params = vec![
            ("oauth_consumer_key".to_owned(), self.config.api_key.clone()),
            ("oauth_nonce".to_owned(), self.nonce()),
            ("oauth_signature_method".to_owned(), "HMAC-SHA1".to_owned()),
            ("oauth_timestamp".to_owned(), self.now().to_string()),
            ("oauth_version".to_owned(), "1.0".to_owned()),
        ];
        if let Some(token) = token {
            params.push(("oauth_token".to_owned(), token.to_owned()));
        }
        params.extend(extra.iter().map(|(name, value)| ((*name).to_owned(), value.clone())));
        params
    }

    /// The stored token, else the session's (kept there when the store is unavailable).
    fn stored_token(&self) -> Option<Token> {
        if let Some(token) = self.session().clone() {
            return Some(token);
        }
        let stored: Token = match self.secrets.get(TOKEN_KEY) {
            Ok(Some(text)) => serde_json::from_str(&text).ok()?,
            _ => return None,
        };
        *self.session() = Some(stored.clone());
        Some(stored)
    }

    fn remember(&self, token: &Token) {
        *self.session() = Some(token.clone());
        let Ok(text) = serde_json::to_string(token) else {
            return;
        };
        match self.secrets.set(TOKEN_KEY, &text) {
            Ok(()) | Err(PortError::Unsupported(_)) => {}
            Err(error) => tracing::warn!(%error, "the Flickr token is kept for this session only"),
        }
    }

    /// Drops the token after Flickr rejects it, so the next upload signs in again.
    fn forget(&self) {
        *self.session() = None;
        let _ = self.secrets.delete(TOKEN_KEY);
    }

    /// The three-legged sign-in: request token, the browser, then the access token.
    async fn sign_in(&self, consumer_secret: &str) -> Result<Token, UploadError> {
        let endpoints = &self.config.endpoints;
        let (port, redirect) = laqta_http::loopback_once(String::new()).await.map_err(http_error)?;
        let callback = format!("http://127.0.0.1:{port}/callback");

        let oauth = self.oauth_params(None, &[("oauth_callback", callback)]);
        let signed = sign("POST", &endpoints.request_token, &[], oauth, consumer_secret, "");
        let reply = call(post_oauth(&endpoints.request_token, &signed), None).await?;
        let token = form_field(&reply.body, "oauth_token")?;
        let token_secret = form_field(&reply.body, "oauth_token_secret")?;

        let mut authorize =
            url::Url::parse(&endpoints.authorize).map_err(|error| UploadError::Unsupported(format!("Flickr authorize URL: {error}")))?;
        authorize.query_pairs_mut().append_pair("oauth_token", &token).append_pair("perms", "write");
        self.shell.open_url(authorize.as_str()).map_err(port_error)?;

        let target = redirect.await.map_err(http_error)?;
        let (returned_token, verifier) = callback_parameters(&target)?;
        if returned_token != token {
            return Err(UploadError::Auth("the Flickr redirect does not carry this sign-in's token".to_owned()));
        }

        let oauth = self.oauth_params(Some(&token), &[("oauth_verifier", verifier)]);
        let signed = sign("POST", &endpoints.access_token, &[], oauth, consumer_secret, &token_secret);
        let reply = call(post_oauth(&endpoints.access_token, &signed), None).await?;
        Ok(Token {
            oauth_token: form_field(&reply.body, "oauth_token")?,
            oauth_token_secret: form_field(&reply.body, "oauth_token_secret")?,
        })
    }

    /// `flickr.photos.getInfo` for the page URL. A failure here leaves the URL out: the photo is
    /// already up, so the upload itself must still succeed.
    async fn page_url(&self, photo_id: &str, consumer_secret: &str, token: &Token) -> Option<String> {
        let covered = vec![
            ("method".to_owned(), "flickr.photos.getInfo".to_owned()),
            ("api_key".to_owned(), self.config.api_key.clone()),
            ("photo_id".to_owned(), photo_id.to_owned()),
            ("format".to_owned(), "json".to_owned()),
            ("nojsoncallback".to_owned(), "1".to_owned()),
        ];
        let oauth = self.oauth_params(Some(&token.oauth_token), &[]);
        let signed = sign("GET", &self.config.endpoints.rest, &covered, oauth, consumer_secret, &token.oauth_token_secret);
        let query = covered.iter().map(|(name, value)| format!("{}={}", enc(name), enc(value))).collect::<Vec<_>>().join("&");
        let mut request = Request::new(Method::Get, format!("{}?{query}", self.config.endpoints.rest));
        request.headers.push(("Authorization".to_owned(), authorization_header(&signed)));
        let info = match call(request, None).await {
            Ok(reply) => json::<PhotoInfo>(&reply),
            Err(error) => Err(error),
        };
        match info {
            Ok(info) => info.photo.urls.url.into_iter().find(|url| url.kind == "photopage").map(|url| url.content),
            Err(error) => {
                tracing::warn!(%error, "the Flickr page URL is not known");
                None
            }
        }
    }

    async fn run(&self, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
        let UploadRequest { file_name, data, mime, .. } = request;
        let data = data_bytes(data)?;
        let consumer_secret = self.consumer_secret()?;
        let token = match self.stored_token() {
            Some(token) => token,
            None => {
                let token = self.sign_in(&consumer_secret).await?;
                self.remember(&token);
                token
            }
        };

        // Multipart fields are not form-encoded, so they stay out of the base string: only the
        // oauth_* parameters are signed.
        let oauth = self.oauth_params(Some(&token.oauth_token), &[]);
        let signed = sign("POST", &self.config.endpoints.upload, &[], oauth, &consumer_secret, &token.oauth_token_secret);
        let title = file_name.rsplit_once('.').map_or(file_name.clone(), |(stem, _)| stem.to_owned());
        let mut upload = Request::new(Method::Post, self.config.endpoints.upload.clone());
        upload.headers.push(("Authorization".to_owned(), authorization_header(&signed)));
        upload.body = Body::Multipart(vec![
            Part::Text { name: "is_public".to_owned(), value: if self.config.public { "1" } else { "0" }.to_owned() },
            Part::Text { name: "title".to_owned(), value: title },
            Part::File { name: "photo".to_owned(), file_name, mime, data },
        ]);
        let response = match upload_call(upload, progress, None).await {
            Ok(response) => response,
            Err(error) => {
                if matches!(error, UploadError::Auth(_)) {
                    self.forget();
                }
                return Err(error);
            }
        };
        let photo_id = photo_id(&response.body)?;
        let url = self.page_url(&photo_id, &consumer_secret, &token).await;
        Ok(UploadResult { url, thumbnail_url: None, deletion_url: None, response: response.text() })
    }
}

impl fmt::Debug for Flickr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Flickr").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Flickr {
    fn id(&self) -> &str {
        "flickr"
    }

    fn name(&self) -> &str {
        "Flickr"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(self.run(request, progress))
    }
}

#[derive(Deserialize)]
struct PhotoInfo {
    photo: Photo,
}

#[derive(Deserialize)]
struct Photo {
    urls: Urls,
}

#[derive(Deserialize)]
struct Urls {
    url: Vec<PhotoUrl>,
}

#[derive(Deserialize)]
struct PhotoUrl {
    #[serde(rename = "type")]
    kind: String,
    #[serde(rename = "_content")]
    content: String,
}

/// RFC 3986 unreserved characters stay as they are; everything else is percent-encoded (RFC 5849
/// §3.6).
fn enc(value: &str) -> String {
    utf8_percent_encode(value, PATH_SEGMENT).to_string()
}

/// The signature base string (RFC 5849 §3.4.1.1): the method, the base URI (no query) and the
/// normalized parameters, each encoded, joined with `&`. `params` holds every parameter the
/// signature covers: the oauth_* ones and the query or form ones.
pub fn base_string(method: &str, url: &str, params: &[(String, String)]) -> String {
    let mut pairs: Vec<(String, String)> = params.iter().map(|(name, value)| (enc(name), enc(value))).collect();
    pairs.sort();
    let normalized = pairs.iter().map(|(name, value)| format!("{name}={value}")).collect::<Vec<_>>().join("&");
    format!("{}&{}&{}", method.to_ascii_uppercase(), enc(url), enc(&normalized))
}

/// HMAC-SHA1 over the base string (RFC 5849 §3.4.2). The key is the encoded consumer secret, an
/// `&`, and the encoded token secret (empty before the access token exists).
pub fn signature(base: &str, consumer_secret: &str, token_secret: &str) -> String {
    let key = format!("{}&{}", enc(consumer_secret), enc(token_secret));
    let mut mac = Hmac::<Sha1>::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(base.as_bytes());
    STANDARD.encode(mac.finalize().into_bytes())
}

/// The oauth parameters with `oauth_signature` appended. `covered` holds the request's query or
/// form parameters, which the signature covers as well.
fn sign(
    method: &str,
    url: &str,
    covered: &[(String, String)],
    mut oauth: Vec<(String, String)>,
    consumer_secret: &str,
    token_secret: &str,
) -> Vec<(String, String)> {
    let mut all = oauth.clone();
    all.extend(covered.iter().cloned());
    let base = base_string(method, url, &all);
    oauth.push(("oauth_signature".to_owned(), signature(&base, consumer_secret, token_secret)));
    oauth
}

/// The `Authorization` header: `OAuth` and the parameters, each name and value encoded and quoted.
fn authorization_header(params: &[(String, String)]) -> String {
    let fields: Vec<String> = params.iter().map(|(name, value)| format!("{}=\"{}\"", enc(name), enc(value))).collect();
    format!("OAuth {}", fields.join(", "))
}

fn post_oauth(url: &str, oauth: &[(String, String)]) -> Request {
    let mut request = Request::new(Method::Post, url.to_owned());
    request.headers.push(("Authorization".to_owned(), authorization_header(oauth)));
    request
}

/// One field of a form-encoded reply (the OAuth token endpoints).
fn form_field(body: &[u8], name: &str) -> Result<String, UploadError> {
    url::form_urlencoded::parse(body)
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.into_owned())
        .ok_or_else(|| UploadError::BadResponse(format!("the Flickr token reply has no {name}")))
}

/// The token and the verifier in the loopback redirect (`/callback?oauth_token=…&oauth_verifier=…`).
fn callback_parameters(target: &str) -> Result<(String, String), UploadError> {
    let url = url::Url::parse(&format!("http://127.0.0.1{target}"))
        .map_err(|error| UploadError::BadResponse(format!("the Flickr redirect target: {error}")))?;
    let (mut token, mut verifier) = (None, None);
    for (key, value) in url.query_pairs() {
        match key.as_ref() {
            "oauth_token" => token = Some(value.into_owned()),
            "oauth_verifier" => verifier = Some(value.into_owned()),
            _ => {}
        }
    }
    match (token, verifier) {
        (Some(token), Some(verifier)) => Ok((token, verifier)),
        _ => Err(UploadError::Auth("the Flickr sign-in was not approved".to_owned())),
    }
}

/// The photo id of an upload reply. Flickr answers in XML: `<rsp stat="ok"><photoid>…</photoid>`,
/// or `<rsp stat="fail"><err code="…" msg="…"/>`.
fn photo_id(body: &[u8]) -> Result<String, UploadError> {
    let text = std::str::from_utf8(body).map_err(|error| UploadError::BadResponse(format!("Flickr reply: {error}")))?;
    let document = roxmltree::Document::parse(text).map_err(|error| UploadError::BadResponse(format!("Flickr reply: {error}")))?;
    let root = document.root_element();
    if root.attribute("stat") != Some("ok") {
        let error = root.children().find(|node| node.has_tag_name("err"));
        let code = error.and_then(|node| node.attribute("code")).and_then(|code| code.parse::<u32>().ok());
        let message = error.and_then(|node| node.attribute("msg")).unwrap_or("unknown error").to_owned();
        return Err(match code {
            Some(96..=100) => UploadError::Auth(message),
            Some(105) => UploadError::Network(message),
            _ => UploadError::BadResponse(message),
        });
    }
    root.children()
        .find(|node| node.has_tag_name("photoid"))
        .and_then(|node| node.text())
        .map(str::to_owned)
        .ok_or_else(|| UploadError::BadResponse("the Flickr upload reply has no photo id".to_owned()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    /// RFC 5849 §3.4.1.1: the base string of the example request (the parameters in §3.4.1.3.1).
    #[test]
    fn base_string_matches_the_rfc_5849_example() {
        let params: Vec<(String, String)> = [
            ("b5", "=%3D"),
            ("a3", "a"),
            ("c@", ""),
            ("a2", "r b"),
            ("oauth_consumer_key", "9djdj82h48djs9d2"),
            ("oauth_token", "kkk9d7dh3k39sjv7"),
            ("oauth_signature_method", "HMAC-SHA1"),
            ("oauth_timestamp", "137131201"),
            ("oauth_nonce", "7d8f3e4a"),
            ("c2", ""),
            ("a3", "2 q"),
        ]
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
        let expected = "POST&http%3A%2F%2Fexample.com%2Frequest&a2%3Dr%2520b%26a3%3D2%2520q%26a3%3Da%26b5%3D%253D%25253D%26c%2540%3D%26c2%3D%26oauth_consumer_key%3D9djdj82h48djs9d2%26oauth_nonce%3D7d8f3e4a%26oauth_signature_method%3DHMAC-SHA1%26oauth_timestamp%3D137131201%26oauth_token%3Dkkk9d7dh3k39sjv7";
        assert_eq!(base_string("POST", "http://example.com/request", &params), expected);
    }

    /// RFC 5849 §1.2: the temporary credential request. Its signature is in the RFC text.
    #[test]
    fn signature_matches_the_rfc_5849_temporary_credential_request() {
        let params = vec![
            ("oauth_consumer_key".to_owned(), "dpf43f3p2l4k3l03".to_owned()),
            ("oauth_signature_method".to_owned(), "HMAC-SHA1".to_owned()),
            ("oauth_timestamp".to_owned(), "137131200".to_owned()),
            ("oauth_nonce".to_owned(), "wIjqoS".to_owned()),
            ("oauth_callback".to_owned(), "http://printer.example.com/ready".to_owned()),
        ];
        let base = base_string("POST", "https://photos.example.net/initiate", &params);
        assert_eq!(signature(&base, "kd94hf93k423kf44", ""), "74KNZJeDHnMBp0EMJ9ZHt/XKycU=");
    }

    /// RFC 5849 §1.2: the token request, signed with the temporary credentials' secret.
    #[test]
    fn signature_matches_the_rfc_5849_token_request() {
        let params = vec![
            ("oauth_consumer_key".to_owned(), "dpf43f3p2l4k3l03".to_owned()),
            ("oauth_token".to_owned(), "hh5s93j4hdidpola".to_owned()),
            ("oauth_signature_method".to_owned(), "HMAC-SHA1".to_owned()),
            ("oauth_timestamp".to_owned(), "137131201".to_owned()),
            ("oauth_nonce".to_owned(), "walatlh".to_owned()),
            ("oauth_verifier".to_owned(), "hfdp7dh39dks9884".to_owned()),
        ];
        let base = base_string("POST", "https://photos.example.net/token", &params);
        assert_eq!(signature(&base, "kd94hf93k423kf44", "hdhd0244k9j7ao03"), "gKgrFCywp7rO0OXSjdot/IHF7IU=");
    }

    /// The §3.4.1.1 base string, signed with the token secret of that example (`pfkkdhi9sl3r4s00`).
    /// The RFC gives this base string but no signature for it, so the expected value was computed
    /// from the RFC's base string with an independent HMAC-SHA1.
    #[test]
    fn signature_of_the_rfc_5849_base_string_example() {
        let base = "POST&http%3A%2F%2Fexample.com%2Frequest&a2%3Dr%2520b%26a3%3D2%2520q%26a3%3Da%26b5%3D%253D%25253D%26c%2540%3D%26c2%3D%26oauth_consumer_key%3D9djdj82h48djs9d2%26oauth_nonce%3D7d8f3e4a%26oauth_signature_method%3DHMAC-SHA1%26oauth_timestamp%3D137131201%26oauth_token%3Dkkk9d7dh3k39sjv7";
        assert_eq!(signature(base, "kd94hf93k423kf44", "pfkkdhi9sl3r4s00"), "hJiW3ib/H6oWBhS6iCyReahf7B4=");
    }

    #[test]
    fn the_authorization_header_quotes_and_encodes_each_parameter() {
        let params = vec![("oauth_token".to_owned(), "a b".to_owned()), ("oauth_signature".to_owned(), "x/y=".to_owned())];
        assert_eq!(authorization_header(&params), "OAuth oauth_token=\"a%20b\", oauth_signature=\"x%2Fy%3D\"");
    }

    #[test]
    fn a_failed_upload_reply_maps_flickr_error_codes() {
        let bad_key = br#"<?xml version="1.0"?><rsp stat="fail"><err code="100" msg="Invalid API Key"/></rsp>"#;
        assert!(matches!(photo_id(bad_key), Err(UploadError::Auth(_))));
        let busy = br#"<rsp stat="fail"><err code="105" msg="Service Currently Unavailable"/></rsp>"#;
        assert!(matches!(photo_id(busy), Err(UploadError::Network(_))));
        let ok = br#"<?xml version="1.0"?><rsp stat="ok"><photoid>5550</photoid></rsp>"#;
        assert_eq!(photo_id(ok).unwrap(), "5550");
    }
}

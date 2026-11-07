//! Azure Blob Storage (ADR 014): one `PUT` of a block blob. It is authorized either by a SAS token
//! (appended to the URL) or, when there is none, by Shared Key with the account key. Both secrets
//! are read from `SecretStore` under `laqta/azure-blob/…`.

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use hmac::{Hmac, KeyInit, Mac};
use laqta_http::{Body, Method, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use percent_encoding::utf8_percent_encode;
use sha2::Sha256;

use crate::hosts::{PATH_SEGMENT, data_bytes, port_error, upload_call};

/// The storage REST version the requests and the Shared Key signature use.
pub const AZURE_VERSION: &str = "2023-11-03";
/// Where the SAS token lives in `SecretStore`.
pub const SAS_KEY: &str = "laqta/azure-blob/sas_token";
/// Where the account key lives in `SecretStore`.
pub const ACCOUNT_KEY: &str = "laqta/azure-blob/account_key";

/// One Blob destination. `blob_base` is `https://<account>.blob.core.windows.net` in production.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AzureConfig {
    pub account: String,
    pub container: String,
    /// Prepended to the file name to make the blob name, e.g. `shots/`.
    pub path_prefix: String,
    /// Base of the URL shared with users, e.g. a CDN in front of the container.
    pub public_url_base: Option<String>,
    pub blob_base: String,
}

/// The Blob destination.
#[derive(Clone)]
pub struct AzureBlob {
    config: AzureConfig,
    secrets: Arc<dyn SecretStore>,
    now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
}

impl AzureBlob {
    pub fn new(config: AzureConfig, secrets: Arc<dyn SecretStore>, now_unix: Arc<dyn Fn() -> i64 + Send + Sync>) -> Self {
        AzureBlob { config, secrets, now_unix }
    }
}

impl fmt::Debug for AzureBlob {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AzureBlob").field("config", &self.config).finish_non_exhaustive()
    }
}

/// The string that Shared Key signs (Azure "Authorize with Shared Key", blob service): twelve
/// standard header fields, then the canonicalized `x-ms-*` headers, then the canonicalized resource.
/// `ms_headers` are the request's `x-ms-*` headers as sent. `path` is the URL path (`/container/blob`).
/// The Date field stays empty because `x-ms-date` is sent. Pure, so it is tested on every target.
pub fn string_to_sign(
    verb: &str,
    content_length: usize,
    content_type: &str,
    ms_headers: &[(&str, &str)],
    account: &str,
    path: &str,
) -> String {
    let length = if content_length == 0 { String::new() } else { content_length.to_string() };
    let fields = [verb, "", "", length.as_str(), "", content_type, "", "", "", "", "", ""];
    let mut canonical: Vec<(String, String)> = ms_headers
        .iter()
        .map(|(name, value)| (name.to_ascii_lowercase(), value.trim().to_owned()))
        .filter(|(name, _)| name.starts_with("x-ms-"))
        .collect();
    canonical.sort();
    let mut headers = String::new();
    for (name, value) in canonical {
        headers.push_str(&format!("{name}:{value}\n"));
    }
    format!("{}\n{headers}/{account}{path}", fields.join("\n"))
}

/// The Shared Key signature: HMAC-SHA256 with the base64-decoded account key, base64-encoded.
pub fn sign_shared_key(account_key: &str, string_to_sign: &str) -> Result<String, UploadError> {
    let key =
        STANDARD.decode(account_key.trim()).map_err(|error| UploadError::Auth(format!("the Azure account key is not base64: {error}")))?;
    let mut mac = Hmac::<Sha256>::new_from_slice(&key).map_err(|error| UploadError::Auth(format!("the Azure account key: {error}")))?;
    mac.update(string_to_sign.as_bytes());
    Ok(STANDARD.encode(mac.finalize().into_bytes()))
}

/// The HTTP date for `x-ms-date`, RFC 1123 in GMT.
fn http_date(now_unix: i64) -> Result<String, UploadError> {
    let timestamp = jiff::Timestamp::from_second(now_unix).map_err(|error| UploadError::Unsupported(format!("clock: {error}")))?;
    Ok(timestamp.strftime("%a, %d %b %Y %H:%M:%S GMT").to_string())
}

/// The blob name as the URL path takes it: each segment percent-encoded, slashes kept.
pub fn encode_path(name: &str) -> String {
    name.split('/').map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string()).collect::<Vec<_>>().join("/")
}

async fn transfer(
    config: AzureConfig,
    secrets: Arc<dyn SecretStore>,
    now_unix: Arc<dyn Fn() -> i64 + Send + Sync>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let secret = |key: &str| secrets.get(key).map_err(port_error).map(|value| value.filter(|value| !value.trim().is_empty()));
    let sas = secret(SAS_KEY)?.map(|sas| sas.trim().trim_start_matches('?').to_owned());
    let UploadRequest { file_name, data, mime, .. } = request;
    let data = data_bytes(data)?;
    let blob = format!("{}{file_name}", config.path_prefix);
    let path = format!("/{}/{}", config.container, encode_path(&blob));
    let length = data.len();

    let mut headers = vec![("x-ms-blob-type".to_owned(), "BlockBlob".to_owned()), ("x-ms-version".to_owned(), AZURE_VERSION.to_owned())];
    let mut url = format!("{}{path}", config.blob_base.trim_end_matches('/'));
    match sas {
        Some(sas) => {
            // Appended raw: the SAS signature is already percent-encoded.
            url.push('?');
            url.push_str(&sas);
        }
        None => {
            let key = secret(ACCOUNT_KEY)?.ok_or_else(|| {
                UploadError::Auth(format!("no SAS token or account key is stored for Azure ({SAS_KEY} or {ACCOUNT_KEY})"))
            })?;
            let date = http_date(now_unix())?;
            headers.push(("x-ms-date".to_owned(), date));
            let ms: Vec<(&str, &str)> = headers.iter().map(|(name, value)| (name.as_str(), value.as_str())).collect();
            let to_sign = string_to_sign("PUT", length, &mime, &ms, &config.account, &path);
            let signature = sign_shared_key(&key, &to_sign)?;
            headers.push(("Authorization".to_owned(), format!("SharedKey {}:{signature}", config.account)));
        }
    }

    let mut put = Request::new(Method::Put, url);
    put.headers = headers;
    put.body = Body::Bytes { data, content_type: mime };
    let response = upload_call(put, progress, None).await?;

    let shared = match &config.public_url_base {
        Some(base) => format!("{}/{}", base.trim_end_matches('/'), encode_path(&blob)),
        None => format!("{}{path}", config.blob_base.trim_end_matches('/')),
    };
    Ok(UploadResult { url: Some(shared), thumbnail_url: None, deletion_url: None, response: response.text() })
}

impl Destination for AzureBlob {
    fn id(&self) -> &str {
        "azure-blob"
    }

    fn name(&self) -> &str {
        "Azure Blob Storage"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), Arc::clone(&self.now_unix), request, progress))
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::string_to_sign;

    /// Azure's string-to-sign: twelve standard fields, each on its own line (the Date line is empty
    /// because `x-ms-date` is sent), the sorted `x-ms-*` headers, then `/account/path`.
    #[test]
    fn string_to_sign_has_twelve_standard_lines_then_the_headers_and_the_resource() {
        let headers = [("x-ms-version", "2023-11-03"), ("x-ms-date", "D"), ("x-ms-blob-type", "BlockBlob")];
        let text = string_to_sign("PUT", 0, "", &headers, "acct", "/c/b");
        let expected = format!("PUT{}x-ms-blob-type:BlockBlob\nx-ms-date:D\nx-ms-version:2023-11-03\n/acct/c/b", "\n".repeat(12));
        assert_eq!(text, expected);
    }

    #[test]
    fn a_body_length_and_type_fill_their_standard_lines() {
        let text = string_to_sign("PUT", 3, "image/png", &[("x-ms-date", "D")], "acct", "/c/b");
        let standard = "PUT\n\n\n3\n\nimage/png\n\n\n\n\n\n\n";
        assert_eq!(text, format!("{standard}x-ms-date:D\n/acct/c/b"));
    }
}

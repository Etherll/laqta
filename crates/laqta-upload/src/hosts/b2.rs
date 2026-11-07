//! Backblaze B2 through its native API (ADR 014): authorize the account, find the bucket's id, get
//! an upload URL, then upload with `X-Bz-File-Name` and `X-Bz-Content-Sha1`. The application key is
//! read from `SecretStore` under `laqta/backblaze-b2/application_key`.

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use laqta_http::{Body, Method, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;
use serde_json::json;
use sha1::{Digest, Sha1};

use crate::hosts::{PATH_SEGMENT, call, data_bytes, json, json_body, port_error, upload_call};

/// The production root of the B2 native API (`b2_authorize_account` lives here).
pub const B2_API: &str = "https://api.backblazeb2.com";
/// Where the application key lives in `SecretStore`.
pub const SECRET_KEY: &str = "laqta/backblaze-b2/application_key";

/// One B2 destination. `api_base` is `B2_API` in production.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct B2Config {
    pub api_base: String,
    /// The application key ID (not secret; the application key is in `SecretStore`).
    pub key_id: String,
    pub bucket: String,
    /// Prepended to the file name to make the object name, e.g. `shots/`.
    pub path_prefix: String,
    /// Base of the URL shared with users. Without it, the B2 download URL of the bucket is used.
    pub public_url_base: Option<String>,
}

/// The B2 destination.
#[derive(Clone)]
pub struct BackblazeB2 {
    config: B2Config,
    secrets: Arc<dyn SecretStore>,
}

impl BackblazeB2 {
    pub fn new(config: B2Config, secrets: Arc<dyn SecretStore>) -> Self {
        BackblazeB2 { config, secrets }
    }
}

impl fmt::Debug for BackblazeB2 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("BackblazeB2").field("config", &self.config).finish_non_exhaustive()
    }
}

/// The `b2_authorize_account` reply. Newer accounts also return the URLs under `apiInfo`.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Authorized {
    account_id: String,
    authorization_token: String,
    #[serde(default)]
    api_url: Option<String>,
    #[serde(default)]
    download_url: Option<String>,
    #[serde(default)]
    api_info: Option<ApiInfo>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiInfo {
    storage_api: StorageApi,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct StorageApi {
    api_url: String,
    download_url: String,
}

/// The account session that the calls after authorization use.
struct Session {
    account_id: String,
    token: String,
    api_url: String,
    download_url: String,
}

#[derive(Deserialize)]
struct Buckets {
    buckets: Vec<BucketEntry>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct BucketEntry {
    bucket_id: String,
    bucket_name: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct UploadTarget {
    upload_url: String,
    authorization_token: String,
}

async fn authorize(config: &B2Config, key: &str) -> Result<Session, UploadError> {
    let credentials = STANDARD.encode(format!("{}:{key}", config.key_id));
    let mut request = Request::new(Method::Get, format!("{}/b2api/v2/b2_authorize_account", config.api_base));
    request.headers.push(("Authorization".to_owned(), format!("Basic {credentials}")));
    let reply = call(request, None).await?;
    let auth: Authorized = json(&reply)?;
    let (api_url, download_url) = match (auth.api_url, auth.download_url, auth.api_info) {
        (Some(api), Some(download), _) => (api, download),
        (_, _, Some(info)) => (info.storage_api.api_url, info.storage_api.download_url),
        _ => return Err(UploadError::BadResponse("B2 authorization gave no API or download URL".to_owned())),
    };
    Ok(Session { account_id: auth.account_id, token: auth.authorization_token, api_url, download_url })
}

async fn bucket_id(config: &B2Config, session: &Session) -> Result<String, UploadError> {
    let mut request = Request::new(Method::Post, format!("{}/b2api/v2/b2_list_buckets", session.api_url));
    request.headers.push(("Authorization".to_owned(), session.token.clone()));
    request.body = json_body(&json!({"accountId": session.account_id, "bucketName": config.bucket}));
    let reply = call(request, None).await?;
    let buckets: Buckets = json(&reply)?;
    buckets
        .buckets
        .into_iter()
        .find(|bucket| bucket.bucket_name == config.bucket)
        .map(|bucket| bucket.bucket_id)
        .ok_or_else(|| UploadError::Unsupported(format!("no B2 bucket named {:?} for this key", config.bucket)))
}

async fn upload_target(session: &Session, bucket_id: &str) -> Result<UploadTarget, UploadError> {
    let mut request = Request::new(Method::Post, format!("{}/b2api/v2/b2_get_upload_url", session.api_url));
    request.headers.push(("Authorization".to_owned(), session.token.clone()));
    request.body = json_body(&json!({"bucketId": bucket_id}));
    let reply = call(request, None).await?;
    json(&reply)
}

async fn transfer(
    config: B2Config,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let key = secrets
        .get(SECRET_KEY)
        .map_err(port_error)?
        .filter(|key| !key.is_empty())
        .ok_or_else(|| UploadError::Auth(format!("no B2 application key is stored under {SECRET_KEY}")))?;
    let UploadRequest { file_name, data, mime, .. } = request;
    let data = data_bytes(data)?;
    let object = format!("{}{file_name}", config.path_prefix);
    let encoded = encode_path(&object);

    let session = authorize(&config, &key).await?;
    let bucket = bucket_id(&config, &session).await?;
    let target = upload_target(&session, &bucket).await?;

    let digest = Sha1::digest(&data);
    let sha1: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    let content_type = if mime.is_empty() { "b2/x-auto".to_owned() } else { mime };
    let mut upload = Request::new(Method::Post, target.upload_url);
    upload.headers.push(("Authorization".to_owned(), target.authorization_token));
    upload.headers.push(("X-Bz-File-Name".to_owned(), encoded.clone()));
    upload.headers.push(("X-Bz-Content-Sha1".to_owned(), sha1));
    upload.body = Body::Bytes { data, content_type };
    let response = upload_call(upload, progress, None).await?;

    let url = match &config.public_url_base {
        Some(base) => format!("{}/{encoded}", base.trim_end_matches('/')),
        None => format!("{}/file/{}/{encoded}", session.download_url.trim_end_matches('/'), config.bucket),
    };
    Ok(UploadResult { url: Some(url), thumbnail_url: None, deletion_url: None, response: response.text() })
}

/// The object name as B2 takes it in `X-Bz-File-Name`: each segment percent-encoded, slashes kept.
pub fn encode_path(name: &str) -> String {
    name.split('/').map(|segment| utf8_percent_encode(segment, PATH_SEGMENT).to_string()).collect::<Vec<_>>().join("/")
}

impl Destination for BackblazeB2 {
    fn id(&self) -> &str {
        "backblaze-b2"
    }

    fn name(&self) -> &str {
        "Backblaze B2"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

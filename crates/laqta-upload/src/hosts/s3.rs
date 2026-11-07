//! Amazon S3 and S3-compatible storage. The upload is one presigned `PUT` (SigV4, through
//! `rusty-s3`), sent with `laqta_http`. The secret access key is read from `SecretStore` under
//! `laqta/s3/secret_access_key`.

use std::fmt;
use std::sync::Arc;
use std::time::Duration;

use laqta_http::{Body, HttpError, Method, Request};
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadData, UploadError, UploadRequest, UploadResult};
use laqta_ports::{BoxFuture, PortError};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use rusty_s3::{Bucket, Credentials, S3Action, UrlStyle};
use serde::{Deserialize, Serialize};
use url::Url;

/// Where the secret access key lives in `SecretStore`.
pub const SECRET_KEY: &str = "laqta/s3/secret_access_key";

/// How long a presigned URL stays valid. The upload starts at once.
const PRESIGN_EXPIRY: Duration = Duration::from_secs(15 * 60);

/// One S3 destination. `path_prefix` is a plain prefix: the caller applies any name pattern.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct S3Config {
    /// `https://s3.eu-west-1.amazonaws.com`, or the server of an S3-compatible service.
    pub endpoint: String,
    pub region: String,
    pub bucket: String,
    /// Prepended to the file name to make the object key, e.g. `shots/`.
    pub path_prefix: String,
    pub access_key_id: String,
    /// `https://host/bucket/key` instead of `https://bucket.host/key`.
    pub use_path_style: bool,
    /// Base of the URL shared with users, e.g. a CDN in front of the bucket.
    pub public_url_base: Option<String>,
    /// Sends `x-amz-acl: public-read` with the object.
    pub acl_public_read: bool,
}

/// The S3 destination.
#[derive(Clone)]
pub struct S3 {
    config: S3Config,
    secrets: Arc<dyn SecretStore>,
}

impl S3 {
    pub fn new(config: S3Config, secrets: Arc<dyn SecretStore>) -> Self {
        S3 { config, secrets }
    }
}

impl fmt::Debug for S3 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("S3").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for S3 {
    fn id(&self) -> &str {
        "s3"
    }

    fn name(&self) -> &str {
        "Amazon S3"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: S3Config,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let secret = read_secret(secrets.as_ref(), SECRET_KEY)?
        .ok_or_else(|| UploadError::Auth(format!("no S3 secret access key is stored under {SECRET_KEY}")))?;
    let bucket = bucket(&config)?;
    let credentials = Credentials::new(config.access_key_id.clone(), secret);
    let key = format!("{}{}", config.path_prefix, request.file_name);

    let mut action = bucket.put_object(Some(&credentials), &key);
    let mut headers = Vec::new();
    if config.acl_public_read {
        // Signed, so the request must send the same header: AWS rejects unsigned x-amz-* headers.
        action.headers_mut().insert("x-amz-acl", "public-read");
        headers.push(("x-amz-acl".to_owned(), "public-read".to_owned()));
    }
    let signed = action.sign(PRESIGN_EXPIRY);
    let url = object_url(&config, &key)?;

    let mut http = Request::new(Method::Put, signed.as_str());
    http.headers = headers;
    http.body = Body::Bytes { data: into_bytes(request.data)?, content_type: request.mime };
    let response = laqta_http::send_with_progress(http, progress).await.map_err(http_error)?;

    let status = response.status;
    let body = response.text();
    match status {
        200..=299 => Ok(UploadResult { url: Some(url), response: body, ..UploadResult::default() }),
        401 | 403 => Err(UploadError::Auth(format!("HTTP {status}: {body}"))),
        _ => Err(UploadError::Server { status, body }),
    }
}

/// The URL that users see for `key`: the public base plus the key, or the bucket's object URL
/// (path or virtual-host style). Nothing is signed.
pub fn object_url(config: &S3Config, key: &str) -> Result<String, UploadError> {
    match &config.public_url_base {
        Some(base) => Ok(join_url(base, key)),
        None => Ok(bucket(config)?.object_url(key).to_string()),
    }
}

fn bucket(config: &S3Config) -> Result<Bucket, UploadError> {
    let endpoint =
        Url::parse(&config.endpoint).map_err(|error| UploadError::Unsupported(format!("S3 endpoint {:?}: {error}", config.endpoint)))?;
    let style = if config.use_path_style { UrlStyle::Path } else { UrlStyle::VirtualHost };
    Bucket::new(endpoint, style, config.bucket.clone(), config.region.clone())
        .map_err(|error| UploadError::Unsupported(format!("S3 bucket settings: {error:?}")))
}

/// Characters that stay literal in a URL path segment. Unreserved characters such as `-`, `_` and
/// `.` are not in the set, so they stay as they are.
const SEGMENT: &AsciiSet = &CONTROLS
    .add(b' ')
    .add(b'"')
    .add(b'#')
    .add(b'%')
    .add(b'<')
    .add(b'>')
    .add(b'?')
    .add(b'[')
    .add(b'\\')
    .add(b']')
    .add(b'^')
    .add(b'`')
    .add(b'{')
    .add(b'|')
    .add(b'}');

/// Joins a base URL and a key, encoding each `/`-separated segment of the key.
fn join_url(base: &str, key: &str) -> String {
    let encoded: Vec<String> = key.split('/').map(|segment| utf8_percent_encode(segment, SEGMENT).to_string()).collect();
    format!("{}/{}", base.trim_end_matches('/'), encoded.join("/"))
}

fn read_secret(store: &dyn SecretStore, key: &str) -> Result<Option<String>, UploadError> {
    store.get(key).map_err(|error| match error {
        PortError::Unsupported(message) => UploadError::Unsupported(message),
        PortError::PermissionDenied => UploadError::Auth("the credential store denied access".to_owned()),
        PortError::Cancelled => UploadError::Cancelled,
        other => UploadError::Auth(other.to_string()),
    })
}

fn into_bytes(data: UploadData) -> Result<Vec<u8>, UploadError> {
    match data {
        UploadData::Bytes(bytes) => Ok(bytes),
        UploadData::Path(path) => {
            std::fs::read(&path).map_err(|error| UploadError::Unsupported(format!("cannot read {}: {error}", path.display())))
        }
    }
}

fn http_error(error: HttpError) -> UploadError {
    match error {
        HttpError::Network(message) | HttpError::Io(message) => UploadError::Network(message),
        HttpError::Timeout => UploadError::Network("timed out".to_owned()),
        HttpError::Cancelled => UploadError::Cancelled,
        HttpError::Invalid(message) => UploadError::Unsupported(message),
        other => UploadError::BadResponse(other.to_string()),
    }
}

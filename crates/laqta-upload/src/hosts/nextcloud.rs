//! Nextcloud and ownCloud: the file is PUT to the user's WebDAV folder, then a public share link is
//! made through the OCS API. `direct_link` appends `/download` to the link. Without a share, the
//! result is the WebDAV URL. The folder must exist already: `laqta_http` has no MKCOL, so the host
//! does not create it, and a missing folder fails with a message that says so. The app password is
//! read from `SecretStore` under `laqta/nextcloud/app_password`.

use std::fmt;
use std::sync::Arc;

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use laqta_http::{Body, Method, Request};
use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use percent_encoding::utf8_percent_encode;
use serde::Deserialize;

use crate::hosts::{PATH_SEGMENT, call, check, data_bytes, http_error, json, port_error};

/// Where the app password lives in `SecretStore`.
pub const SECRET_KEY: &str = "laqta/nextcloud/app_password";

/// One Nextcloud or ownCloud destination.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NextcloudConfig {
    /// The server root, e.g. `https://cloud.example.com`. A sub-path is kept.
    pub server: String,
    pub username: String,
    /// The folder in the user's files, e.g. `/Laqta`.
    pub directory: String,
    /// Creates a public share link. False returns the WebDAV URL.
    pub share: bool,
    /// Appends `/download` to the share link, so it downloads the file.
    pub direct_link: bool,
}

/// The Nextcloud / ownCloud destination.
#[derive(Clone)]
pub struct Nextcloud {
    config: NextcloudConfig,
    secrets: Arc<dyn SecretStore>,
}

impl Nextcloud {
    pub fn new(config: NextcloudConfig, secrets: Arc<dyn SecretStore>) -> Self {
        Nextcloud { config, secrets }
    }
}

impl fmt::Debug for Nextcloud {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Nextcloud").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Destination for Nextcloud {
    fn id(&self) -> &str {
        "nextcloud"
    }

    fn name(&self) -> &str {
        "Nextcloud / ownCloud"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), Arc::clone(&self.secrets), request, progress))
    }
}

async fn transfer(
    config: NextcloudConfig,
    secrets: Arc<dyn SecretStore>,
    request: UploadRequest,
    progress: ProgressCallback,
) -> Result<UploadResult, UploadError> {
    let server = config.server.trim_end_matches('/').to_owned();
    if server.is_empty() {
        return Err(UploadError::Unsupported("no Nextcloud server URL is set".to_owned()));
    }
    let name = leaf_name(&request.file_name)?.to_owned();
    let folder = folder_parts(&config.directory)?;
    let password = secret(secrets.as_ref(), SECRET_KEY)?;
    let auth = ("Authorization".to_owned(), basic(&config.username, &password));

    // The remote path, segment by segment: the folder, then the file.
    let mut remote: Vec<&str> = folder.iter().map(String::as_str).collect();
    remote.push(&name);
    let user = utf8_percent_encode(&config.username, PATH_SEGMENT);
    let mut dav_path = format!("/remote.php/dav/files/{user}");
    for part in &remote {
        dav_path.push('/');
        dav_path.extend(utf8_percent_encode(part, PATH_SEGMENT));
    }
    let webdav_url = format!("{server}{dav_path}");

    let mut put = Request::new(Method::Put, webdav_url.clone());
    put.headers = vec![auth.clone(), ("OCS-APIRequest".to_owned(), "true".to_owned())];
    put.body = Body::Bytes { data: data_bytes(request.data)?, content_type: request.mime };
    let response = laqta_http::send_with_progress(put, progress).await.map_err(http_error)?;
    if response.status == 409 {
        return Err(UploadError::BadResponse(format!(
            "the folder {} does not exist on the server; create it in Nextcloud first",
            config.directory
        )));
    }
    check(response, None)?;

    if !config.share {
        return Ok(UploadResult { url: Some(webdav_url), ..UploadResult::default() });
    }
    let share_path: String = remote.iter().map(|part| format!("/{part}")).collect();
    // OCS v1 with `format=json` in the URL: ownCloud reads the format from the
    // URL only, and Nextcloud reads both places. `permissions=1` is read-only.
    let mut ocs = Request::new(Method::Post, format!("{server}/ocs/v1.php/apps/files_sharing/api/v1/shares"));
    ocs.query = vec![("format".to_owned(), "json".to_owned())];
    ocs.headers = vec![auth, ("OCS-APIRequest".to_owned(), "true".to_owned())];
    ocs.body = Body::Form(vec![
        ("shareType".to_owned(), "3".to_owned()),
        ("path".to_owned(), share_path),
        ("permissions".to_owned(), "1".to_owned()),
    ]);
    let reply_response = call(ocs, None).await?;
    let reply: OcsEnvelope = json(&reply_response)?;
    let meta = reply.ocs.meta;
    if !matches!(meta.statuscode, 100 | 200) {
        return Err(UploadError::BadResponse(format!("Nextcloud share: {}", meta.message)));
    }
    let link =
        reply.ocs.data.map(|data| data.url).ok_or_else(|| UploadError::BadResponse("Nextcloud share: the reply has no URL".to_owned()))?;
    let url = if config.direct_link { format!("{link}/download") } else { link };
    Ok(UploadResult { url: Some(url), response: reply_response.text(), ..UploadResult::default() })
}

#[derive(Deserialize)]
struct OcsEnvelope {
    ocs: Ocs,
}

#[derive(Deserialize)]
struct Ocs {
    meta: Meta,
    data: Option<ShareData>,
}

#[derive(Deserialize)]
struct Meta {
    statuscode: i64,
    #[serde(default)]
    message: String,
}

#[derive(Deserialize)]
struct ShareData {
    url: String,
}

/// The folder's segments: `/Laqta/` becomes `["Laqta"]`. Empty and `.` segments are dropped, and
/// `..` is refused so the upload stays in the user's files.
fn folder_parts(directory: &str) -> Result<Vec<String>, UploadError> {
    let parts: Vec<String> = directory.split('/').filter(|part| !part.is_empty() && *part != ".").map(str::to_owned).collect();
    if parts.iter().any(|part| part == "..") {
        return Err(UploadError::Unsupported(format!("{directory:?} leaves the user's files")));
    }
    Ok(parts)
}

fn basic(username: &str, password: &str) -> String {
    format!("Basic {}", STANDARD.encode(format!("{username}:{password}")))
}

fn leaf_name(name: &str) -> Result<&str, UploadError> {
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']) && !name.chars().any(char::is_control);
    if plain { Ok(name) } else { Err(UploadError::Unsupported(format!("{name:?} is not a plain file name"))) }
}

fn secret(store: &dyn SecretStore, key: &str) -> Result<String, UploadError> {
    store
        .get(key)
        .map_err(port_error)?
        .filter(|value| !value.is_empty())
        .ok_or_else(|| UploadError::Auth(format!("no secret is stored under {key}")))
}

//! Native upload hosts (ADR 004, ADR 014). Each host is one file behind its `host-<name>` feature
//! and implements `Destination` or `Shortener`. This file declares them and holds the request and
//! response handling they share.

#[cfg(feature = "host-imgur")]
pub mod imgur;

#[cfg(feature = "host-dropbox")]
pub mod dropbox;

#[cfg(feature = "host-google-drive")]
pub mod google_drive;

#[cfg(feature = "host-onedrive")]
pub mod onedrive;

#[cfg(feature = "host-bitly")]
pub mod bitly;

#[cfg(feature = "host-s3")]
pub mod s3;

#[cfg(feature = "host-ftp")]
pub mod ftp;

#[cfg(feature = "host-sftp")]
pub mod sftp;

#[cfg(feature = "host-shared-folder")]
pub mod shared_folder;

pub mod azure_blob;
pub mod b2;
pub mod box_com;
pub mod flickr;
#[cfg(feature = "host-s3")]
pub mod gcs;
pub mod gist;
pub mod mediafire;
pub mod nextcloud;
pub mod privatebin;
pub mod pushbullet;
pub mod sendspace;
pub mod smtp;
pub mod youtube;

use laqta_http::{Body, HttpError, Request, Response};
use laqta_ports::{PortError, ProgressCallback, UploadData, UploadError};
use percent_encoding::{AsciiSet, NON_ALPHANUMERIC};
use serde::de::DeserializeOwned;

use crate::oauth::OAuth;

/// Path segments as the Graph and Drive APIs take them: everything except the unreserved characters
/// is percent-encoded.
pub(crate) const PATH_SEGMENT: &AsciiSet = &NON_ALPHANUMERIC.remove(b'-').remove(b'_').remove(b'.').remove(b'~');

/// A transport failure. Network failures and timeouts are retried by the caller.
pub(crate) fn http_error(error: HttpError) -> UploadError {
    match error {
        HttpError::Connect(message) | HttpError::Network(message) | HttpError::Io(message) => UploadError::Network(message),
        HttpError::Timeout => UploadError::Network("timed out".into()),
        HttpError::Cancelled => UploadError::Cancelled,
        other => UploadError::BadResponse(other.to_string()),
    }
}

/// A secret store or shell failure. `Unsupported` from the store means there is no keyring.
pub(crate) fn port_error(error: PortError) -> UploadError {
    match error {
        PortError::PermissionDenied => UploadError::Auth("permission denied".into()),
        PortError::Cancelled => UploadError::Cancelled,
        PortError::Unsupported(message) | PortError::Failed(message) => UploadError::Unsupported(message),
        other => UploadError::Unsupported(other.to_string()),
    }
}

/// The error for a non-2xx status that is not an auth failure. 429 and 5xx are `Server` and retry;
/// any other status is `BadResponse`.
pub(crate) fn status_error(status: u16, body: String) -> UploadError {
    match status {
        401 | 403 => UploadError::Auth(format!("HTTP {status}: {body}")),
        429 | 500..=599 => UploadError::Server { status, body },
        _ => UploadError::BadResponse(format!("HTTP {status}: {body}")),
    }
}

/// The response when its status is 2xx, otherwise the error for that status. A 401 or 403 also
/// drops the sign-in's token, so the next upload asks for one again.
pub(crate) fn check(response: Response, oauth: Option<&OAuth>) -> Result<Response, UploadError> {
    if (200..300).contains(&response.status) {
        return Ok(response);
    }
    if let (401 | 403, Some(oauth)) = (response.status, oauth) {
        let _ = oauth.sign_out();
    }
    Err(status_error(response.status, response.text()))
}

/// Sends a request and checks its status.
pub(crate) async fn call(request: Request, oauth: Option<&OAuth>) -> Result<Response, UploadError> {
    let response = laqta_http::send(request).await.map_err(http_error)?;
    check(response, oauth)
}

/// Sends an upload, reporting its progress, and checks its status.
pub(crate) async fn upload_call(request: Request, progress: ProgressCallback, oauth: Option<&OAuth>) -> Result<Response, UploadError> {
    let response = laqta_http::send_with_progress(request, progress).await.map_err(http_error)?;
    check(response, oauth)
}

/// The `Authorization` header for a bearer token.
pub(crate) fn bearer(token: &str) -> (String, String) {
    ("Authorization".to_owned(), format!("Bearer {token}"))
}

/// The upload's bytes. A file is read whole, which blocks the calling thread for the read.
pub(crate) fn data_bytes(data: UploadData) -> Result<Vec<u8>, UploadError> {
    match data {
        UploadData::Bytes(bytes) => Ok(bytes),
        UploadData::Path(path) => {
            std::fs::read(&path).map_err(|error| UploadError::BadResponse(format!("cannot read {}: {error}", path.display())))
        }
    }
}

/// A JSON request body.
pub(crate) fn json_body(value: &serde_json::Value) -> Body {
    Body::Bytes { data: value.to_string().into_bytes(), content_type: "application/json".to_owned() }
}

/// A 2xx body parsed as JSON.
pub(crate) fn json<T: DeserializeOwned>(response: &Response) -> Result<T, UploadError> {
    serde_json::from_slice(&response.body).map_err(|error| UploadError::BadResponse(format!("unexpected response: {error}")))
}

/// A multipart boundary that occurs in none of `parts`.
pub(crate) fn boundary_for(parts: &[&[u8]]) -> String {
    let mut n: u32 = 0;
    loop {
        let boundary = format!("laqta-{n:08x}");
        let needle = boundary.as_bytes();
        let found = parts.iter().any(|part| part.windows(needle.len()).any(|window| window == needle));
        if !found {
            return boundary;
        }
        n += 1;
    }
}

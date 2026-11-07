//! MediaFire. Not built yet: the destination refuses every upload before it sends anything.
//!
//! Sign-in (`user/get_session_token.php`) is signed with the SHA-1 of e-mail, password, application
//! id and API key (`signature`). Every later call, the upload and the polls included, carries a
//! request signature that is an MD5 of the secret key, the session time and the query (ShareX's
//! `MediaFire.cs`). laqta-upload has no MD5 crate: `md-5` reaches `Cargo.lock` only through other
//! crates, and adding it to `Cargo.toml` is outside this task. The upload is multipart with the file
//! as `Filedata`, and the link comes from `upload/poll_upload.php` once `doupload.status` is 99.

use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, UploadError, UploadRequest, UploadResult};
use sha1::{Digest, Sha1};

const NOT_BUILT: &str = "MediaFire uploads need MD5-signed API calls, which laqta-upload cannot make yet";

/// The MediaFire destination. It keeps no state while it refuses.
#[derive(Clone, Copy, Debug, Default)]
pub struct MediaFire;

impl Destination for MediaFire {
    fn id(&self) -> &str {
        "mediafire"
    }

    fn name(&self) -> &str {
        "MediaFire"
    }

    fn upload(&self, _request: UploadRequest, _progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(async { Err(UploadError::Unsupported(NOT_BUILT.to_owned())) })
    }
}

/// The sign-in signature: the SHA-1 of e-mail, password, application id and API key, concatenated
/// with no separators, as lowercase hex (ShareX's `GetInitSignature`).
pub fn signature(email: &str, password: &str, app_id: &str, api_key: &str) -> String {
    let digest = Sha1::digest(format!("{email}{password}{app_id}{api_key}").as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

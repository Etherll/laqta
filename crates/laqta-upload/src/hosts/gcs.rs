//! Google Cloud Storage through its XML API with HMAC keys (S3 interoperability, ADR 014). The
//! objects are uploaded with the S3 SigV4 signer against `storage.googleapis.com` in path style and
//! region `auto`. The HMAC secret is kept under its own `SecretStore` key, so it is never the Amazon
//! S3 account's secret.

use std::sync::Arc;

use laqta_ports::upload::{Destination, ProgressCallback, SecretStore, UploadError, UploadRequest, UploadResult};
use laqta_ports::{BoxFuture, PortError};

use crate::hosts::s3::{self, S3, S3Config};

/// The XML API root.
pub const GCS_ENDPOINT: &str = "https://storage.googleapis.com";
/// Where the HMAC secret lives in `SecretStore`.
pub const GCS_SECRET_KEY: &str = "laqta/google-cloud-storage/secret_access_key";
/// The destination id in the upload registry.
pub const GCS_ID: &str = "google-cloud-storage";

/// One GCS destination. `endpoint` is `GCS_ENDPOINT` in production.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct GcsConfig {
    pub endpoint: String,
    pub bucket: String,
    /// Prepended to the file name to make the object name, e.g. `shots/`.
    pub path_prefix: String,
    /// The HMAC access ID.
    pub access_key_id: String,
    /// Base of the URL shared with users. Without it, the bucket's XML API URL is used.
    pub public_url_base: Option<String>,
}

impl GcsConfig {
    /// The S3 settings for the same bucket. GCS takes `auto` as the region, path-style URLs, and no
    /// ACL header (uniform bucket access rejects it).
    pub fn s3(&self) -> S3Config {
        S3Config {
            endpoint: self.endpoint.clone(),
            region: "auto".to_owned(),
            bucket: self.bucket.clone(),
            path_prefix: self.path_prefix.clone(),
            access_key_id: self.access_key_id.clone(),
            use_path_style: true,
            public_url_base: self.public_url_base.clone(),
            acl_public_read: false,
        }
    }
}

/// The GCS destination: the S3 transfer with its own id, name and secret.
#[derive(Debug)]
pub struct GoogleCloudStorage {
    inner: S3,
}

impl GoogleCloudStorage {
    pub fn new(config: &GcsConfig, secrets: Arc<dyn SecretStore>) -> Self {
        GoogleCloudStorage { inner: S3::new(config.s3(), Arc::new(GcsSecret { inner: secrets })) }
    }
}

/// Answers the S3 transfer's read of its secret key with the GCS key. Everything else passes
/// through unchanged.
struct GcsSecret {
    inner: Arc<dyn SecretStore>,
}

impl SecretStore for GcsSecret {
    fn get(&self, key: &str) -> Result<Option<String>, PortError> {
        let key = if key == s3::SECRET_KEY { GCS_SECRET_KEY } else { key };
        self.inner.get(key)
    }

    fn set(&self, key: &str, secret: &str) -> Result<(), PortError> {
        self.inner.set(key, secret)
    }

    fn delete(&self, key: &str) -> Result<(), PortError> {
        self.inner.delete(key)
    }
}

impl Destination for GoogleCloudStorage {
    fn id(&self) -> &str {
        GCS_ID
    }

    fn name(&self) -> &str {
        "Google Cloud Storage"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        self.inner.upload(request, progress)
    }
}

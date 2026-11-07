//! Shared folder: copies the file into a directory (a local disk or a mapped network share) and
//! returns a URL. The URL is `url_base` plus the file name, or a `file://` URL of the copy when
//! there is no base. The copy runs when the future is polled, on the caller's executor thread.

use std::fs::File;
use std::io;
use std::path::{Path, PathBuf};

use laqta_ports::BoxFuture;
use laqta_ports::upload::{Destination, ProgressCallback, UploadData, UploadError, UploadRequest, UploadResult};
use percent_encoding::{AsciiSet, CONTROLS, utf8_percent_encode};
use serde::{Deserialize, Serialize};
use url::Url;

/// Where the copies go.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFolderConfig {
    pub directory: PathBuf,
    /// Base of the URL shared with users; the file name is appended. Without it, the URL is a
    /// `file://` URL of the copy.
    pub url_base: Option<String>,
}

/// The shared-folder destination. It needs no secrets.
#[derive(Clone, Debug)]
pub struct SharedFolder {
    config: SharedFolderConfig,
}

impl SharedFolder {
    pub fn new(config: SharedFolderConfig) -> Self {
        SharedFolder { config }
    }
}

impl Destination for SharedFolder {
    fn id(&self) -> &str {
        "shared-folder"
    }

    fn name(&self) -> &str {
        "Shared folder"
    }

    fn upload(&self, request: UploadRequest, progress: ProgressCallback) -> BoxFuture<'_, Result<UploadResult, UploadError>> {
        Box::pin(transfer(self.config.clone(), request, progress))
    }
}

async fn transfer(config: SharedFolderConfig, request: UploadRequest, progress: ProgressCallback) -> Result<UploadResult, UploadError> {
    let name = leaf_name(&request.file_name)?;
    std::fs::create_dir_all(&config.directory).map_err(io_error)?;
    let target = config.directory.join(name);

    let size = match request.data {
        UploadData::Bytes(bytes) => {
            std::fs::write(&target, &bytes).map_err(io_error)?;
            bytes.len() as u64
        }
        UploadData::Path(source) => {
            let mut input = File::open(&source).map_err(|error| unreadable(&source, &error))?;
            let mut output = File::create(&target).map_err(io_error)?;
            io::copy(&mut input, &mut output).map_err(io_error)?
        }
    };
    progress(size, Some(size));

    let url = match &config.url_base {
        Some(base) => join_url(base, name),
        None => {
            let absolute = std::path::absolute(&target).map_err(io_error)?;
            Url::from_file_path(&absolute)
                .map_err(|()| UploadError::Unsupported(format!("cannot make a file URL for {}", target.display())))?
                .to_string()
        }
    };
    Ok(UploadResult { url: Some(url), ..UploadResult::default() })
}

fn leaf_name(name: &str) -> Result<&str, UploadError> {
    let plain = !name.is_empty() && name != "." && name != ".." && !name.contains(['/', '\\']) && !name.chars().any(char::is_control);
    if plain { Ok(name) } else { Err(UploadError::Unsupported(format!("{name:?} is not a plain file name"))) }
}

/// Characters that stay literal in a URL path segment.
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

fn join_url(base: &str, name: &str) -> String {
    let encoded = utf8_percent_encode(name, SEGMENT).to_string();
    format!("{}/{encoded}", base.trim_end_matches('/'))
}

fn unreadable(path: &Path, error: &io::Error) -> UploadError {
    UploadError::Unsupported(format!("cannot read {}: {error}", path.display()))
}

fn io_error(error: io::Error) -> UploadError {
    match error.kind() {
        io::ErrorKind::PermissionDenied => UploadError::Auth(format!("permission denied: {error}")),
        io::ErrorKind::StorageFull => UploadError::Quota(error.to_string()),
        _ => UploadError::Network(error.to_string()),
    }
}

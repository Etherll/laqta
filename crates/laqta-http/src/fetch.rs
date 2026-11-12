//! Verified fetch (ARCHITECTURE §6): download to a partial file beside `dest`, check SHA-256, then
//! rename into place. A failed or unverified download removes its own partial file.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use reqwest::Client;
use sha2::{Digest, Sha256};
use tokio::io::AsyncWriteExt;

use crate::client::map_error;
use crate::runtime::{dispatch, finish};
use crate::{HttpError, Progress};

/// A model or FFmpeg download can outlast the 120 s request timeout, so the whole request gets a
/// much longer limit. A stalled transfer is ended by `STALL_LIMIT` and `CANCEL_POLL` instead.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(24 * 60 * 60);
/// How often a download waiting for data checks its cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(250);
/// A download that receives no data for this long fails with `Timeout`.
const STALL_LIMIT: Duration = Duration::from_secs(60);

pub(crate) fn is_sha256_hex(text: &str) -> bool {
    text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `expected` must already be lowercase and checked by `is_sha256_hex`.
pub(crate) async fn fetch(
    url: String,
    dest: PathBuf,
    expected: String,
    progress: Progress,
    cancel: Arc<AtomicBool>,
) -> Result<(), HttpError> {
    let rx = dispatch(move |client| async move { download(&client, &url, &dest, &expected, &progress, &cancel).await })?;
    finish(rx).await
}

async fn download(
    client: &Client,
    url: &str,
    dest: &Path,
    expected: &str,
    progress: &Progress,
    cancel: &AtomicBool,
) -> Result<(), HttpError> {
    let part = part_path(dest);
    if let Some(parent) = dest.parent().filter(|parent| !parent.as_os_str().is_empty()) {
        tokio::fs::create_dir_all(parent).await.map_err(io_error)?;
    }
    let result = match transfer(client, url, &part, progress, cancel).await {
        Ok(actual) if actual == expected => tokio::fs::rename(&part, dest).await.map_err(io_error),
        Ok(actual) => Err(HttpError::Checksum { expected: expected.to_owned(), actual }),
        Err(err) => Err(err),
    };
    if result.is_err() {
        discard(&part).await;
    }
    result
}

/// Removes a failed call's partial file. Each call has its own name, so this never touches another's.
async fn discard(part: &Path) {
    let _ = tokio::fs::remove_file(part).await;
}

/// Streams the body into `part` and returns its SHA-256 as lowercase hex.
async fn transfer(client: &Client, url: &str, part: &Path, progress: &Progress, cancel: &AtomicBool) -> Result<String, HttpError> {
    let mut response = client.get(url).timeout(DOWNLOAD_TIMEOUT).send().await.map_err(map_error)?;
    let status = response.status();
    if !status.is_success() {
        return Err(HttpError::Network(format!("HTTP {status} for {url}")));
    }
    let total = response.content_length();
    let mut file = tokio::fs::File::create(part).await.map_err(io_error)?;
    let mut hasher = Sha256::new();
    let mut received = 0u64;
    let mut last_data = Instant::now();
    loop {
        if cancel.load(Ordering::Relaxed) {
            return Err(HttpError::Cancelled);
        }
        // Waking every CANCEL_POLL keeps a silent server from holding the cancel flag unread.
        // `chunk` only polls the body, so dropping it while it waits loses no data.
        let next = match tokio::time::timeout(CANCEL_POLL, response.chunk()).await {
            Ok(next) => next.map_err(map_error)?,
            Err(_elapsed) => {
                if last_data.elapsed() >= STALL_LIMIT {
                    return Err(HttpError::Timeout);
                }
                continue;
            }
        };
        last_data = Instant::now();
        let Some(chunk) = next else {
            break;
        };
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(io_error)?;
        received += chunk.len() as u64;
        progress(received, total);
    }
    // The progress callback may have raised the flag during the last chunk.
    if cancel.load(Ordering::Relaxed) {
        return Err(HttpError::Cancelled);
    }
    file.flush().await.map_err(io_error)?;
    file.sync_all().await.map_err(io_error)?;
    Ok(to_hex(hasher.finalize().as_slice()))
}

/// Counts the calls in this process, so concurrent downloads of one destination get different names.
static PART_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A partial file for one call: `<dest>.<pid>-<n>.part`.
fn part_path(dest: &Path) -> PathBuf {
    let n = PART_COUNTER.fetch_add(1, Ordering::Relaxed);
    let mut name = dest.as_os_str().to_owned();
    name.push(format!(".{}-{n}.part", std::process::id()));
    PathBuf::from(name)
}

fn to_hex(bytes: &[u8]) -> String {
    let mut hex = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn io_error(err: std::io::Error) -> HttpError {
    HttpError::Io(err.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use std::fs;

    #[test]
    fn each_call_gets_its_own_part_file() {
        let dest = Path::new("models/ort.so");
        let (first, second) = (part_path(dest), part_path(dest));
        assert_ne!(first, second);
        for part in [&first, &second] {
            assert_eq!(part.parent(), dest.parent());
            let name = part.file_name().unwrap().to_string_lossy().into_owned();
            assert!(name.starts_with("ort.so.") && name.ends_with(".part"), "{name}");
        }
    }

    #[test]
    fn discarding_one_part_file_leaves_the_other_for_the_same_destination() {
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("ort.so");
        let (mine, theirs) = (part_path(&dest), part_path(&dest));
        fs::write(&mine, b"mine").unwrap();
        fs::write(&theirs, b"theirs").unwrap();
        tokio::runtime::Builder::new_current_thread().build().unwrap().block_on(discard(&mine));
        assert!(!mine.exists());
        assert_eq!(fs::read(&theirs).unwrap(), b"theirs");
    }
}

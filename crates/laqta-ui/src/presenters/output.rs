//! Editor output: encoding on the compute pool and file access, both off the UI thread.
//!
//! The views never call `std::fs` or wait on the pool (`tests/view_guards.rs`). They await
//! [`encode`], and run the blocking file functions here from a background executor.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use laqta_domain::Bitmap;
use laqta_imaging::{Codecs, EncodeOptions};
use laqta_ports::BoxFuture;

use super::render_gate::{ComputeFn, compute};

/// Encodes the image `image` resolves to. Nothing here runs on the caller's thread.
pub async fn encode(
    pool: ComputeFn,
    codecs: Codecs,
    image: BoxFuture<'static, Option<Bitmap>>,
    opts: EncodeOptions,
) -> Result<Vec<u8>, String> {
    let image = image.await.ok_or_else(|| "Couldn't read the image.".to_owned())?;
    match compute(&pool, move || codecs.encode(&image, &opts)).await {
        Some(Ok(bytes)) => Ok(bytes),
        Some(Err(e)) => Err(e.to_string()),
        None => Err("Couldn't encode the image.".to_owned()),
    }
}

/// Writes `bytes` to `path`, creating its folder. Blocks: call it from a background executor.
pub fn write_file(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// Reads a whole file. Blocks: call it from a background executor.
pub fn read_file(path: &Path) -> Result<Vec<u8>, String> {
    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Temporary uploads older than this are removed when the next one is written.
const TEMP_MAX_AGE: Duration = Duration::from_secs(24 * 60 * 60);

/// Writes a temporary file for an upload as `<dir>/<stem>_<time>_<n>.<ext>`, with the first free `n`, and
/// removes the files in `dir` older than a day. An upload reads its file later, so the file can't be
/// removed when this returns. The time (in seconds) keeps a name from being used again once its file
/// is gone, as history may still point at it. Blocks: call it from a background executor.
pub fn write_temp(dir: &Path, stem: &str, ext: &str, bytes: &[u8], now: SystemTime) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let old = entry.metadata().and_then(|m| m.modified()).ok().and_then(|at| now.duration_since(at).ok());
            if old.is_some_and(|age| age > TEMP_MAX_AGE) {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
    // `create_new` reserves the name, so two uploads that finish together never share a file.
    let time = now.duration_since(SystemTime::UNIX_EPOCH).map_or(0, |since| since.as_secs());
    for n in 0u32.. {
        let path = dir.join(format!("{stem}_{time}_{n}.{ext}"));
        match std::fs::OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(mut file) => {
                use std::io::Write as _;
                file.write_all(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
                return Ok(path);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    Err(format!("{}: no free file name", dir.display()))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn write_file_creates_the_folder() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("a/b/c.bin");
        write_file(&path, b"xy").unwrap();
        assert_eq!(read_file(&path).unwrap(), b"xy");
    }

    #[test]
    fn concurrent_temp_writes_never_share_a_file() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let dir = tmp.path().to_path_buf();
        let writers: Vec<_> = (0u8..16)
            .map(|i| {
                let dir = dir.clone();
                std::thread::spawn(move || (i, write_temp(&dir, "edit", "png", &[i], now).unwrap()))
            })
            .collect();
        let mut paths = std::collections::HashSet::new();
        for writer in writers {
            let (i, path) = writer.join().unwrap();
            assert_eq!(read_file(&path).unwrap(), [i]);
            assert!(paths.insert(path));
        }
    }

    #[test]
    fn temp_files_get_fresh_names_and_old_ones_go() {
        let tmp = tempfile::tempdir().unwrap();
        let now = SystemTime::now();
        let first = write_temp(tmp.path(), "edit", "png", b"1", now).unwrap();
        let second = write_temp(tmp.path(), "edit", "png", b"2", now).unwrap();
        assert_ne!(first, second);
        assert_eq!(read_file(&first).unwrap(), b"1");
        // Two days later both are stale: the next write removes them, and takes a name of its own (the
        // history entry of an upload still names the old file).
        let third = write_temp(tmp.path(), "edit", "png", b"3", now + Duration::from_secs(2 * 24 * 60 * 60)).unwrap();
        assert_ne!(third, first);
        assert!(!first.exists() && !second.exists());
        assert_eq!(read_file(&third).unwrap(), b"3");
    }

    #[test]
    fn encode_reports_a_missing_image() {
        let pool: ComputeFn = std::sync::Arc::new(|work| {
            work();
            Box::pin(async {})
        });
        let opts = EncodeOptions { format: laqta_domain::ImageFormat::Png, jpeg_quality: 90 };
        let r = futures_lite::future::block_on(encode(pool, Codecs::builtin(), Box::pin(async { None }), opts));
        assert_eq!(r, Err("Couldn't read the image.".to_owned()));
    }
}

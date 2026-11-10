//! Installs onnxruntime on first use: the pinned archive from the catalog, checked by SHA-256 and
//! unpacked down to the one library file the runtime loads (ADR 023, `load-dynamic`).

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crate::catalog::{OrtArchive, catalog};
use crate::error::MlError;

/// The archive entry for this machine.
pub fn ort_for_host() -> Option<&'static OrtArchive> {
    let os = if cfg!(windows) {
        "windows"
    } else if cfg!(target_os = "macos") {
        "macos"
    } else {
        "linux"
    };
    catalog().onnxruntime_for(os, std::env::consts::ARCH)
}

/// Where the library ends up: `<dir>/<file name>`.
pub fn library_path(dir: &Path, entry: &OrtArchive) -> PathBuf {
    let name = Path::new(&entry.library).file_name().map(|n| n.to_owned()).unwrap_or_default();
    dir.join(name)
}

/// The partial file the library is written to before it is renamed into place: `<target>.part`.
fn part_path(target: &Path) -> PathBuf {
    let mut name = target.as_os_str().to_owned();
    name.push(".part");
    PathBuf::from(name)
}

/// Copies the library out of a `.tgz` or `.zip` archive.
pub fn extract_library(archive: &Path, entry: &OrtArchive, dir: &Path) -> Result<PathBuf, MlError> {
    let io = |e: std::io::Error| MlError::Io(e.to_string());
    fs::create_dir_all(dir).map_err(io)?;
    let target = library_path(dir, entry);
    let mut bytes = Vec::new();
    let mut found = false;
    match entry.archive.as_str() {
        "tgz" => {
            let f = fs::File::open(archive).map_err(io)?;
            let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(f));
            for e in tar.entries().map_err(io)? {
                let mut e = e.map_err(io)?;
                let p = e.path().map_err(io)?.to_string_lossy().replace('\\', "/");
                if p == entry.library {
                    e.read_to_end(&mut bytes).map_err(io)?;
                    found = true;
                    break;
                }
            }
        }
        "zip" => {
            let f = fs::File::open(archive).map_err(io)?;
            let mut z = zip::ZipArchive::new(f).map_err(|e| MlError::Io(e.to_string()))?;
            if let Ok(mut e) = z.by_name(&entry.library) {
                e.read_to_end(&mut bytes).map_err(io)?;
                found = true;
            }
        }
        other => return Err(MlError::Io(format!("unknown archive kind {other}"))),
    }
    if !found {
        return Err(MlError::Io(format!("{} not found in the archive", entry.library)));
    }
    // Written under a partial name and renamed, so the final name only ever holds a whole library.
    let part = part_path(&target);
    let placed = fs::write(&part, bytes).and_then(|()| fs::rename(&part, &target));
    if let Err(error) = placed {
        let _ = fs::remove_file(&part);
        return Err(io(error));
    }
    Ok(target)
}

/// Downloads (verified) and unpacks onnxruntime into `dir`; returns the library path. Already
/// installed → returns at once. A catalog entry with a placeholder hash is refused before anything is
/// fetched.
pub async fn install_onnxruntime(
    dir: &Path,
    progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync>,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, MlError> {
    let entry = ort_for_host().ok_or_else(|| MlError::Io("no pinned onnxruntime for this platform".into()))?;
    install_entry(dir, entry, progress, cancel).await
}

async fn install_entry(
    dir: &Path,
    entry: &OrtArchive,
    progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync>,
    cancel: Arc<AtomicBool>,
) -> Result<PathBuf, MlError> {
    if entry.is_pending() {
        return Err(MlError::PendingHash(format!("onnxruntime-{}-{}", entry.os, entry.arch)));
    }
    let lib = library_path(dir, entry);
    if lib.exists() {
        return Ok(lib);
    }
    let archive = dir.join(Path::new(&entry.url).file_name().unwrap_or_default());
    laqta_http::fetch_verified(&entry.url, &archive, &entry.sha256, progress, cancel).await.map_err(|e| MlError::Io(e.to_string()))?;
    let out = extract_library(&archive, entry, dir);
    let _ = fs::remove_file(&archive);
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use crate::catalog::PENDING_HASH;
    use std::future::Future;
    use std::pin::pin;
    use std::task::{Context, Poll, Waker};
    use std::time::Duration;

    fn block_on<F: Future>(future: F) -> F::Output {
        let mut context = Context::from_waker(Waker::noop());
        let mut future = pin!(future);
        loop {
            if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
                return output;
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn sample_entry() -> OrtArchive {
        OrtArchive {
            os: "linux".into(),
            arch: "x86_64".into(),
            version: "1".into(),
            url: "https://x/ort.tgz".into(),
            sha256: String::new(),
            size_bytes: 0,
            archive: "tgz".into(),
            library: "ort/lib/libonnxruntime.so.1".into(),
        }
    }

    /// A `.tgz` in `dir` holding the sample library, `ELF`, at the entry's path.
    fn sample_tgz(dir: &Path) -> PathBuf {
        let archive = dir.join("ort.tgz");
        let f = fs::File::create(&archive).unwrap();
        let gz = flate2::write::GzEncoder::new(f, flate2::Compression::fast());
        let mut b = tar::Builder::new(gz);
        let data = b"ELF";
        let mut h = tar::Header::new_gnu();
        h.set_size(data.len() as u64);
        h.set_mode(0o644);
        h.set_cksum();
        b.append_data(&mut h, "ort/lib/libonnxruntime.so.1", &data[..]).unwrap();
        b.into_inner().unwrap().finish().unwrap();
        archive
    }

    #[test]
    fn extracts_from_a_tgz() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = sample_entry();
        let archive = sample_tgz(tmp.path());
        let lib = extract_library(&archive, &entry, &tmp.path().join("out")).unwrap();
        assert_eq!(fs::read(&lib).unwrap(), b"ELF");
        assert!(!part_path(&lib).exists());
    }

    #[test]
    fn a_failed_write_leaves_no_library_at_the_final_name() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = sample_entry();
        let archive = sample_tgz(tmp.path());
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let target = library_path(&out, &entry);
        // A directory where the partial file should go makes the write fail.
        fs::create_dir(part_path(&target)).unwrap();
        assert!(extract_library(&archive, &entry, &out).is_err());
        assert!(!target.exists());
    }

    #[test]
    fn a_failed_rename_removes_the_partial_file() {
        let tmp = tempfile::tempdir().unwrap();
        let entry = sample_entry();
        let archive = sample_tgz(tmp.path());
        let out = tmp.path().join("out");
        fs::create_dir_all(&out).unwrap();
        let target = library_path(&out, &entry);
        // A directory at the final name makes the rename fail.
        fs::create_dir(&target).unwrap();
        assert!(extract_library(&archive, &entry, &out).is_err());
        assert!(!part_path(&target).exists());
    }

    #[test]
    fn a_pending_archive_is_refused_before_any_fetch() {
        let tmp = tempfile::tempdir().unwrap();
        let mut entry = sample_entry();
        entry.sha256 = PENDING_HASH.to_owned();
        let progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync> = Arc::new(|_, _| {});
        let result = block_on(install_entry(tmp.path(), &entry, progress, Arc::new(AtomicBool::new(false))));
        assert!(matches!(result, Err(MlError::PendingHash(_))), "{result:?}");
        assert!(!library_path(tmp.path(), &entry).exists());
    }
}

//! The pinned FFmpeg build (ADR 005, ADR 022). It is downloaded on first use through
//! `laqta-http`'s verified fetch, and only `bin/ffmpeg` and `bin/ffprobe` are extracted, into
//! `<data>/ffmpeg/bin`.

use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use laqta_http::HttpError;
use laqta_ports::PortError;

use crate::locate::{FfmpegPaths, exe_name};

const RELEASE_URL: &str = "https://github.com/BtbN/FFmpeg-Builds/releases/download";

/// The BtbN release the builds come from. It is pinned, never `latest`.
const RELEASE_TAG: &str = "autobuild-2026-11-06-12-50";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ArchiveKind {
    Zip,
    TarXz,
}

/// One pinned archive: its file name in the release, its SHA-256 and its format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Pinned {
    pub(crate) file: &'static str,
    pub(crate) sha256: &'static str,
    pub(crate) kind: ArchiveKind,
}

/// FFmpeg n9.0.2, GPL static build for Windows x64.
pub(crate) const PINNED_WIN64: Pinned = Pinned {
    file: "ffmpeg-n9.0.2-47-gb204e94e90-win64-gpl-9.0.zip",
    sha256: "8436c89156c3695659bf968e26bab5b08351ed8c86c3e9dbe3d6cc66a23af0a8",
    kind: ArchiveKind::Zip,
};

/// FFmpeg n9.0.2, GPL static build for Linux x64.
pub(crate) const PINNED_LINUX64: Pinned = Pinned {
    file: "ffmpeg-n9.0.2-47-gb204e94e90-linux64-gpl-9.0.tar.xz",
    sha256: "f44c6aa39c1b258d3ceafcee26d961445afdb810c8a1ffe85c4ba6a55b6ce60b",
    kind: ArchiveKind::TarXz,
};

fn pinned_for_host() -> Option<Pinned> {
    if cfg!(windows) {
        Some(PINNED_WIN64)
    } else if cfg!(target_os = "linux") {
        Some(PINNED_LINUX64)
    } else {
        None
    }
}

pub(crate) fn download_url(pinned: Pinned) -> String {
    format!("{RELEASE_URL}/{RELEASE_TAG}/{}", pinned.file)
}

/// Downloads the pinned build into `<data_dir>/ffmpeg`, checks its SHA-256, and extracts FFmpeg and
/// FFprobe into `<data_dir>/ffmpeg/bin`. `progress` is (bytes so far, total). `cancel` stops the
/// download with `PortError::Cancelled`.
pub async fn download_ffmpeg(
    data_dir: &Path,
    progress: Arc<dyn Fn(u64, Option<u64>) + Send + Sync>,
    cancel: Arc<AtomicBool>,
) -> Result<FfmpegPaths, PortError> {
    let pinned = pinned_for_host().ok_or_else(|| PortError::Unsupported("no pinned FFmpeg build for this platform".into()))?;
    let ffmpeg_dir = data_dir.join("ffmpeg");
    let archive = ffmpeg_dir.join(pinned.file);
    laqta_http::fetch_verified(&download_url(pinned), &archive, pinned.sha256, progress, cancel).await.map_err(http_error)?;
    let bin_dir = ffmpeg_dir.join("bin");
    crate::run_blocking(move || {
        let paths = extract_build(&archive, pinned.kind, &bin_dir)?;
        // The archive is no longer needed once its two binaries are out.
        let _ = fs::remove_file(&archive);
        Ok(paths)
    })
    .await
}

fn http_error(err: HttpError) -> PortError {
    match err {
        HttpError::Cancelled => PortError::Cancelled,
        other => PortError::Failed(format!("FFmpeg download failed: {other}")),
    }
}

/// One binary to extract: its name inside the archive's `bin` folder and where it goes.
struct Target {
    entry: String,
    dest: PathBuf,
}

fn targets(bin_dir: &Path) -> Vec<Target> {
    ["ffmpeg", "ffprobe"].iter().map(|base| Target { entry: exe_name(base), dest: bin_dir.join(exe_name(base)) }).collect()
}

/// Extracts `*/bin/ffmpeg(.exe)` and `*/bin/ffprobe(.exe)` from `archive` into `bin_dir`, and makes
/// them executable on Unix. Anything else in the archive is ignored, and no entry can write outside
/// `bin_dir`.
pub(crate) fn extract_build(archive: &Path, kind: ArchiveKind, bin_dir: &Path) -> Result<FfmpegPaths, PortError> {
    fs::create_dir_all(bin_dir).map_err(|err| PortError::Failed(format!("create {}: {err}", bin_dir.display())))?;
    let targets = targets(bin_dir);
    let found = match kind {
        ArchiveKind::Zip => extract_zip(archive, &targets),
        ArchiveKind::TarXz => extract_tar_xz(archive, &targets),
    }
    .map_err(PortError::Failed)?;
    for (target, found) in targets.iter().zip(&found) {
        if !found {
            return Err(PortError::Failed(format!("the FFmpeg archive has no bin/{}", target.entry)));
        }
        make_executable(&target.dest)?;
    }
    Ok(FfmpegPaths { ffmpeg: targets[0].dest.clone(), ffprobe: targets[1].dest.clone() })
}

/// The index of the target that `entry_path` names: exactly `<top>/bin/<entry>`, with no `..`,
/// root or prefix components.
fn target_index(entry_path: &Path, targets: &[Target]) -> Option<usize> {
    let mut parts = Vec::new();
    for component in entry_path.components() {
        match component {
            Component::Normal(part) => parts.push(part),
            _ => return None,
        }
    }
    match parts.as_slice() {
        [_top, bin, name] if *bin == "bin" => targets.iter().position(|target| std::ffi::OsStr::new(&target.entry) == *name),
        _ => None,
    }
}

fn extract_zip(archive: &Path, targets: &[Target]) -> Result<Vec<bool>, String> {
    let file = File::open(archive).map_err(|err| format!("open {}: {err}", archive.display()))?;
    let mut zip = zip::ZipArchive::new(BufReader::new(file)).map_err(|err| format!("read zip: {err}"))?;
    let mut found = vec![false; targets.len()];
    for index in 0..zip.len() {
        let mut entry = zip.by_index(index).map_err(|err| format!("read zip entry: {err}"))?;
        if entry.is_dir() {
            continue;
        }
        let Some(path) = entry.enclosed_name() else { continue };
        if let Some(found_index) = target_index(&path, targets) {
            write_target(&mut entry, &targets[found_index])?;
            found[found_index] = true;
        }
    }
    Ok(found)
}

fn extract_tar_xz(archive: &Path, targets: &[Target]) -> Result<Vec<bool>, String> {
    let tar_path = sibling(archive, ".tar");
    let decompressed = decompress_xz(archive, &tar_path);
    let found = decompressed.and_then(|()| extract_tar(&tar_path, targets));
    let _ = fs::remove_file(&tar_path);
    found
}

/// Streams the xz file into a plain tar file beside it. The tar is then read entry by entry.
fn decompress_xz(archive: &Path, tar_path: &Path) -> Result<(), String> {
    let input = File::open(archive).map_err(|err| format!("open {}: {err}", archive.display()))?;
    let output = File::create(tar_path).map_err(|err| format!("create {}: {err}", tar_path.display()))?;
    let mut input = BufReader::new(input);
    let mut output = BufWriter::new(output);
    lzma_rs::xz_decompress(&mut input, &mut output).map_err(|err| format!("decompress: {err}"))?;
    output.flush().map_err(|err| format!("write tar: {err}"))
}

fn extract_tar(tar_path: &Path, targets: &[Target]) -> Result<Vec<bool>, String> {
    let file = File::open(tar_path).map_err(|err| format!("open tar: {err}"))?;
    let mut archive = tar::Archive::new(BufReader::new(file));
    let mut found = vec![false; targets.len()];
    for entry in archive.entries().map_err(|err| format!("read tar: {err}"))? {
        let mut entry = entry.map_err(|err| format!("read tar entry: {err}"))?;
        if !entry.header().entry_type().is_file() {
            continue;
        }
        let path = entry.path().map_err(|err| format!("tar entry path: {err}"))?.into_owned();
        if let Some(found_index) = target_index(&path, targets) {
            write_target(&mut entry, &targets[found_index])?;
            found[found_index] = true;
        }
    }
    Ok(found)
}

/// Copies one entry to its destination through a `.part` file, so a failed copy leaves nothing.
fn write_target(source: &mut impl Read, target: &Target) -> Result<(), String> {
    let part = sibling(&target.dest, ".part");
    {
        let mut out = File::create(&part).map_err(|err| format!("create {}: {err}", part.display()))?;
        io::copy(source, &mut out).map_err(|err| format!("extract {}: {err}", target.entry))?;
    }
    place(&part, &target.dest).map_err(|err| format!("place {}: {err}", target.dest.display()))
}

/// Moves `part` onto `dest`. Windows refuses to replace a program that is running (a recording in
/// progress) but lets it be renamed, so a refused replace moves the old file to `<dest>.old`
/// first. The next download removes that file.
fn place(part: &Path, dest: &Path) -> io::Result<()> {
    let old = sibling(dest, ".old");
    let _ = fs::remove_file(&old);
    let Err(refused) = fs::rename(part, dest) else { return Ok(()) };
    if fs::rename(dest, &old).is_err() {
        let _ = fs::remove_file(part);
        return Err(refused);
    }
    fs::rename(part, dest)
}

fn sibling(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_owned();
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> Result<(), PortError> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(0o755))
        .map_err(|err| PortError::Failed(format!("chmod {}: {err}", path.display())))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> Result<(), PortError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use super::*;

    fn is_sha256_hex(text: &str) -> bool {
        text.len() == 64 && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    }

    /// Entries are (path in the archive, contents).
    fn build_zip(path: &Path, entries: &[(String, &str)]) {
        let file = File::create(path).expect("create zip");
        let mut writer = zip::ZipWriter::new(file);
        for (name, data) in entries {
            writer.start_file(name.as_str(), zip::write::SimpleFileOptions::default()).expect("start file");
            writer.write_all(data.as_bytes()).expect("write entry");
        }
        writer.finish().expect("finish zip");
    }

    fn build_tar_xz(path: &Path, entries: &[(String, &str)]) {
        let mut builder = tar::Builder::new(Vec::new());
        for (name, data) in entries {
            let mut header = tar::Header::new_gnu();
            header.set_size(data.len() as u64);
            header.set_mode(0o755);
            header.set_entry_type(tar::EntryType::Regular);
            builder.append_data(&mut header, name, data.as_bytes()).expect("append tar entry");
        }
        let tar_bytes = builder.into_inner().expect("finish tar");
        let mut out = Vec::new();
        lzma_rs::xz_compress(&mut Cursor::new(tar_bytes), &mut out).expect("xz compress");
        fs::write(path, out).expect("write xz");
    }

    #[test]
    fn pinned_builds_have_real_hashes_and_urls() {
        for pinned in [PINNED_WIN64, PINNED_LINUX64] {
            assert!(is_sha256_hex(pinned.sha256), "{}", pinned.file);
            assert!(
                download_url(pinned).starts_with("https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-11-06-12-50/")
            );
        }
        assert!(PINNED_WIN64.file.ends_with("-win64-gpl-9.0.zip"));
        assert!(PINNED_LINUX64.file.ends_with("-linux64-gpl-9.0.tar.xz"));
        assert_eq!(PINNED_WIN64.kind, ArchiveKind::Zip);
        assert_eq!(PINNED_LINUX64.kind, ArchiveKind::TarXz);
    }

    #[test]
    fn zip_extracts_only_the_two_binaries() {
        let ffmpeg = exe_name("ffmpeg");
        let ffprobe = exe_name("ffprobe");
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("build.zip");
        build_zip(
            &archive,
            &[
                (format!("ffmpeg-n9/bin/{ffmpeg}"), "real ffmpeg"),
                (format!("ffmpeg-n9/bin/{ffprobe}"), "real ffprobe"),
                ("ffmpeg-n9/doc/readme.txt".to_owned(), "docs"),
                (format!("ffmpeg-n9/bin/nested/{ffmpeg}"), "nested"),
                (format!("../bin/{ffmpeg}"), "evil"),
            ],
        );
        let bin = dir.path().join("out").join("bin");
        let paths = extract_build(&archive, ArchiveKind::Zip, &bin).expect("extract");
        assert_eq!(fs::read(&paths.ffmpeg).expect("read"), b"real ffmpeg");
        assert_eq!(fs::read(&paths.ffprobe).expect("read"), b"real ffprobe");
        assert_eq!(paths.ffmpeg, bin.join(&ffmpeg));
        let names: Vec<_> = fs::read_dir(&bin).expect("list").map(|e| e.expect("entry").file_name()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
    }

    #[test]
    fn tar_xz_extracts_bin_entries_and_makes_them_executable() {
        let ffmpeg = exe_name("ffmpeg");
        let ffprobe = exe_name("ffprobe");
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("build.tar.xz");
        build_tar_xz(
            &archive,
            &[
                (format!("ffmpeg-n9/bin/{ffmpeg}"), "real ffmpeg"),
                (format!("ffmpeg-n9/bin/{ffprobe}"), "real ffprobe"),
                ("ffmpeg-n9/bin/ffplay".to_owned(), "not wanted"),
                ("ffmpeg-n9/doc/readme".to_owned(), "docs"),
            ],
        );
        let bin = dir.path().join("bin");
        let paths = extract_build(&archive, ArchiveKind::TarXz, &bin).expect("extract");
        assert_eq!(fs::read(&paths.ffmpeg).expect("read"), b"real ffmpeg");
        assert_eq!(fs::read(&paths.ffprobe).expect("read"), b"real ffprobe");
        assert!(!bin.join("ffplay").exists());
        assert!(!dir.path().join("build.tar.xz.tar").exists(), "the temporary tar is removed");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = fs::metadata(&paths.ffmpeg).expect("metadata").permissions().mode();
            assert_eq!(mode & 0o777, 0o755);
        }
    }

    #[test]
    fn missing_binary_is_an_error_naming_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("build.zip");
        build_zip(&archive, &[(format!("top/bin/{}", exe_name("ffmpeg")), "only ffmpeg")]);
        let err = extract_build(&archive, ArchiveKind::Zip, &dir.path().join("bin")).expect_err("must fail");
        assert!(err.to_string().contains("ffprobe"), "{err}");
    }

    #[test]
    fn garbage_archive_fails_without_leaving_binaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        let archive = dir.path().join("build.tar.xz");
        fs::write(&archive, b"not xz at all").expect("write");
        let bin = dir.path().join("bin");
        assert!(extract_build(&archive, ArchiveKind::TarXz, &bin).is_err());
        assert!(!bin.join(exe_name("ffmpeg")).exists());
    }

    #[test]
    fn placing_replaces_the_old_file_and_clears_a_stale_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (part, dest, old) = (dir.path().join("ffmpeg.part"), dir.path().join("ffmpeg"), dir.path().join("ffmpeg.old"));
        fs::write(&dest, "old").expect("write");
        fs::write(&old, "older").expect("write");
        fs::write(&part, "new").expect("write");
        place(&part, &dest).expect("place");
        assert_eq!(fs::read_to_string(&dest).expect("read"), "new");
        assert!(!old.exists() && !part.exists());
    }
}

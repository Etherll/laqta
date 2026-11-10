//! Finding FFmpeg and FFprobe: the configured path, then the downloaded copy, then `PATH`, then (on
//! macOS) the folders Homebrew installs into.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

/// The two binaries Laqta runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FfmpegPaths {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
}

/// Finds FFmpeg. `configured` is a binary or a folder that holds both binaries (empty = not set).
/// Then `<data_dir>/ffmpeg/bin` (where `download_ffmpeg` puts them), then `PATH`. An app started
/// from Finder or the Dock gets a `PATH` without Homebrew, so on macOS its folders are looked in last.
pub fn find_ffmpeg(configured: &str, data_dir: &Path) -> Option<FfmpegPaths> {
    let path_var = std::env::var_os("PATH").unwrap_or_default();
    let extra: &[&str] = if cfg!(target_os = "macos") { &HOMEBREW_DIRS } else { &[] };
    find_with(configured, data_dir, &path_var).or_else(|| first_pair(extra))
}

/// The first of `dirs` that holds both binaries.
fn first_pair<P: AsRef<Path>>(dirs: &[P]) -> Option<FfmpegPaths> {
    dirs.iter().find_map(|dir| pair_in(dir.as_ref()))
}

/// Where Homebrew puts its programs: Apple silicon, then Intel.
const HOMEBREW_DIRS: [&str; 2] = ["/opt/homebrew/bin", "/usr/local/bin"];

/// `find_ffmpeg` with the `PATH` value passed in, so tests need not change the environment.
pub(crate) fn find_with(configured: &str, data_dir: &Path, path_var: &OsStr) -> Option<FfmpegPaths> {
    let configured = configured.trim();
    if !configured.is_empty()
        && let Some(found) = from_configured(Path::new(configured))
    {
        return Some(found);
    }
    if let Some(found) = pair_in(&data_dir.join("ffmpeg").join("bin")) {
        return Some(found);
    }
    let ffmpeg = find_on_path(path_var, &exe_name("ffmpeg"))?;
    let ffprobe = find_on_path(path_var, &exe_name("ffprobe"))?;
    Some(FfmpegPaths { ffmpeg, ffprobe })
}

/// The executable name on this OS: `.exe` on Windows.
pub(crate) fn exe_name(base: &str) -> String {
    if cfg!(windows) { format!("{base}.exe") } else { base.to_owned() }
}

fn from_configured(path: &Path) -> Option<FfmpegPaths> {
    if path.is_dir() {
        return pair_in(path).or_else(|| pair_in(&path.join("bin")));
    }
    if path.is_file() {
        let ffprobe = path.parent()?.join(exe_name("ffprobe"));
        return ffprobe.is_file().then(|| FfmpegPaths { ffmpeg: path.to_path_buf(), ffprobe });
    }
    tracing::warn!(path = %path.display(), "configured FFmpeg path does not exist");
    None
}

/// Both binaries in `dir`, or nothing.
fn pair_in(dir: &Path) -> Option<FfmpegPaths> {
    let ffmpeg = dir.join(exe_name("ffmpeg"));
    let ffprobe = dir.join(exe_name("ffprobe"));
    (ffmpeg.is_file() && ffprobe.is_file()).then_some(FfmpegPaths { ffmpeg, ffprobe })
}

fn find_on_path(path_var: &OsStr, name: &str) -> Option<PathBuf> {
    std::env::split_paths(path_var).map(|dir| dir.join(name)).find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().expect("parent")).expect("create dir");
        std::fs::write(path, b"").expect("write file");
    }

    #[test]
    fn the_first_folder_with_both_binaries_wins() {
        let dir = tempfile::tempdir().expect("tempdir");
        let (only_ffmpeg, both) = (dir.path().join("a"), dir.path().join("b"));
        touch(&only_ffmpeg.join(exe_name("ffmpeg")));
        touch(&both.join(exe_name("ffmpeg")));
        touch(&both.join(exe_name("ffprobe")));
        let found = first_pair(&[dir.path().join("missing"), only_ffmpeg, both.clone()]).expect("found");
        assert_eq!(found.ffmpeg, both.join(exe_name("ffmpeg")));
        assert_eq!(first_pair::<PathBuf>(&[]), None);
    }

    #[test]
    fn configured_file_uses_sibling_ffprobe() {
        let dir = tempfile::tempdir().expect("tempdir");
        let ffmpeg = dir.path().join(exe_name("my-ffmpeg"));
        touch(&ffmpeg);
        touch(&dir.path().join(exe_name("ffprobe")));
        let found = find_with(&ffmpeg.display().to_string(), dir.path(), OsStr::new("")).expect("found");
        assert_eq!(found.ffmpeg, ffmpeg);
        assert_eq!(found.ffprobe, dir.path().join(exe_name("ffprobe")));
    }

    #[test]
    fn configured_folder_holds_both_binaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join(exe_name("ffmpeg")));
        touch(&dir.path().join(exe_name("ffprobe")));
        let found = find_with(&dir.path().display().to_string(), Path::new("/nowhere"), OsStr::new("")).expect("found");
        assert_eq!(found.ffmpeg, dir.path().join(exe_name("ffmpeg")));
    }

    #[test]
    fn configured_file_without_ffprobe_falls_through() {
        let configured = tempfile::tempdir().expect("tempdir");
        let ffmpeg = configured.path().join(exe_name("ffmpeg"));
        touch(&ffmpeg);
        let data = tempfile::tempdir().expect("tempdir");
        touch(&data.path().join("ffmpeg").join("bin").join(exe_name("ffmpeg")));
        touch(&data.path().join("ffmpeg").join("bin").join(exe_name("ffprobe")));
        let found = find_with(&ffmpeg.display().to_string(), data.path(), OsStr::new("")).expect("found");
        assert_eq!(found.ffmpeg, data.path().join("ffmpeg").join("bin").join(exe_name("ffmpeg")));
    }

    #[test]
    fn downloaded_copy_comes_before_path() {
        let data = tempfile::tempdir().expect("tempdir");
        touch(&data.path().join("ffmpeg").join("bin").join(exe_name("ffmpeg")));
        touch(&data.path().join("ffmpeg").join("bin").join(exe_name("ffprobe")));
        let other = tempfile::tempdir().expect("tempdir");
        touch(&other.path().join(exe_name("ffmpeg")));
        touch(&other.path().join(exe_name("ffprobe")));
        let path_var = std::env::join_paths([other.path()]).expect("join");
        let found = find_with("", data.path(), &path_var).expect("found");
        assert!(found.ffmpeg.starts_with(data.path()));
    }

    #[test]
    fn path_search_needs_both_binaries() {
        let dir = tempfile::tempdir().expect("tempdir");
        touch(&dir.path().join(exe_name("ffmpeg")));
        let path_var = std::env::join_paths([dir.path()]).expect("join");
        assert!(find_with("", Path::new("/nowhere"), &path_var).is_none());
        touch(&dir.path().join(exe_name("ffprobe")));
        let found = find_with("", Path::new("/nowhere"), &path_var).expect("found");
        assert_eq!(found.ffprobe, dir.path().join(exe_name("ffprobe")));
    }

    #[test]
    fn nothing_found_is_none() {
        let empty = tempfile::tempdir().expect("tempdir");
        assert!(find_with("", empty.path(), OsStr::new("")).is_none());
    }
}

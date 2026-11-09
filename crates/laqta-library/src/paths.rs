use std::path::{Path, PathBuf};

/// Portable mode: a `portable` file next to the executable moves the data into
/// `LaqtaData` beside it. Otherwise the data lives in `default`.
pub fn data_dir(exe_dir: Option<&Path>, default: PathBuf) -> PathBuf {
    match exe_dir {
        Some(dir) if dir.join("portable").exists() => dir.join("LaqtaData"),
        _ => default,
    }
}

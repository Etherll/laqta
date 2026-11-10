use std::path::{Path, PathBuf};

/// Portable mode: a `portable` file next to the executable, or an executable whose name
/// contains "portable" (`laqta-windows-x64-portable.exe`), moves the data into `LaqtaData`
/// beside it. Otherwise the data lives in `default`.
pub fn data_dir(exe: Option<&Path>, default: PathBuf) -> PathBuf {
    let Some(dir) = exe.and_then(Path::parent) else { return default };
    if dir.join("portable").exists() || exe.is_some_and(is_portable_name) { dir.join("LaqtaData") } else { default }
}

fn is_portable_name(exe: &Path) -> bool {
    exe.file_stem().is_some_and(|stem| stem.to_string_lossy().to_ascii_lowercase().contains("portable"))
}

//! What the tool modules share: error mapping, file access through the ports, and output names.

use std::io;
use std::path::{Path, PathBuf};

use laqta_domain::naming;
use laqta_imaging::CodecError;
use laqta_ports::AppError;

use crate::Ports;
use crate::files;

pub(super) fn io_error(error: io::Error) -> AppError {
    AppError::Io(error.to_string())
}

/// Codec errors are reported as I/O errors, as the Task module reports them.
pub(super) fn codec_error(error: CodecError) -> AppError {
    AppError::Io(error.to_string())
}

pub(super) async fn read_file(ports: &Ports, path: &Path) -> Result<Vec<u8>, AppError> {
    files::read(ports.spawner.as_ref(), path.to_path_buf()).await.map_err(io_error)
}

/// Writes `bytes` to `path` atomically, creating its folder when it is missing.
pub(super) async fn write_file(ports: &Ports, path: PathBuf, bytes: Vec<u8>) -> Result<(), AppError> {
    files::save_atomic(ports.spawner.as_ref(), path, bytes).await.map_err(io_error)
}

/// The file name without its extension, or `image` when there is none.
pub(super) fn file_stem(path: &Path) -> String {
    path.file_stem().map_or_else(|| "image".to_owned(), |stem| stem.to_string_lossy().into_owned())
}

/// Writes `bytes` as `<stem><suffix>.<extension>` in `dir`, under the first name that is not taken. A name that is taken,
/// the input file included, is never overwritten. Returns the path written.
pub(super) async fn save_named(
    ports: &Ports,
    dir: &Path,
    stem: &str,
    suffix: &str,
    extension: &str,
    bytes: Vec<u8>,
) -> Result<PathBuf, AppError> {
    let candidates: Vec<PathBuf> = naming::candidates(dir, "", &format!("{stem}{suffix}"), extension).collect();
    let path = files::first_free(ports.spawner.as_ref(), candidates).await.map_err(io_error)?;
    write_file(ports, path.clone(), bytes).await?;
    Ok(path)
}

/// Fails when `output` is one of `inputs`, so a tool never writes over what it reads.
pub(super) fn ensure_not_input(output: &Path, inputs: &[PathBuf]) -> Result<(), AppError> {
    if inputs.iter().any(|input| input == output) {
        return Err(AppError::Io(format!("{} is one of the inputs", output.display())));
    }
    Ok(())
}

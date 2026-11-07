//! Video and animation tools: wrappers over the `Media` port (M5 D1). The adapter does the encoding; these functions
//! choose the spec and the output names.

use std::path::{Path, PathBuf};

use laqta_domain::naming;
use laqta_domain::record::VideoCodec;
use laqta_ports::{AppError, TranscodeSpec};

use super::common::{file_stem, io_error};
use crate::Ports;
use crate::files;

/// Converts `input` to `output` with `spec`. The output folder is created when it is missing.
pub async fn convert_video(ports: &Ports, input: &Path, output: &Path, spec: TranscodeSpec) -> Result<(), AppError> {
    if let Some(dir) = output.parent().filter(|dir| !dir.as_os_str().is_empty()) {
        files::create_dir_all(ports.spawner.as_ref(), dir.to_path_buf()).await.map_err(io_error)?;
    }
    Ok(ports.media.transcode(input, output, spec).await?)
}

/// Keeps `duration_secs` of `input` from `start_secs`, written to `output` with the adapter's default codec.
pub async fn trim_video(ports: &Ports, input: &Path, output: &Path, start_secs: f64, duration_secs: f64) -> Result<(), AppError> {
    let spec = TranscodeSpec { start_secs: Some(start_secs), duration_secs: Some(duration_secs), ..TranscodeSpec::default() };
    convert_video(ports, input, output, spec).await
}

/// Makes an animated GIF from `input` at `fps` frames per second. A `width` of 0 keeps the source width.
pub async fn make_gif(ports: &Ports, input: &Path, output: &Path, fps: u32, width: u32) -> Result<(), AppError> {
    let spec =
        TranscodeSpec { codec: Some(VideoCodec::Gif), fps: Some(fps), width: (width > 0).then_some(width), ..TranscodeSpec::default() };
    convert_video(ports, input, output, spec).await
}

/// `count` still frames from `input`, evenly spaced inside its length and kept clear of the first and last instant. Each
/// is saved in `out_dir` as a PNG named `<stem>_frame001.png` and so on. `count` 0 gives none.
pub async fn video_thumbnails(ports: &Ports, input: &Path, out_dir: &Path, count: u32) -> Result<Vec<PathBuf>, AppError> {
    if count == 0 {
        return Ok(Vec::new());
    }
    let duration = ports.media.probe(input).await?.duration_secs;
    if !(duration.is_finite() && duration > 0.0) {
        return Err(AppError::Unsupported("the file has no duration to take frames from".to_owned()));
    }
    let spawner = ports.spawner.as_ref();
    files::create_dir_all(spawner, out_dir.to_path_buf()).await.map_err(io_error)?;
    let stem = file_stem(input);
    let mut frames = Vec::with_capacity(count as usize);
    for index in 1..=count {
        let at = duration * f64::from(index) / (f64::from(count) + 1.0);
        let candidates: Vec<PathBuf> = naming::candidates(out_dir, "", &format!("{stem}_frame{index:03}"), "png").collect();
        let frame = files::first_free(spawner, candidates).await.map_err(io_error)?;
        let spec = TranscodeSpec { start_secs: Some(at), single_frame: true, ..TranscodeSpec::default() };
        ports.media.transcode(input, &frame, spec).await?;
        frames.push(frame);
    }
    Ok(frames)
}

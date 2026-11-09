//! Image tools: file wrappers over `laqta-imaging::tools`. Each function reads its inputs on the blocking pool,
//! does the pixel work on the compute pool, and writes the outputs. Outputs get a new name beside the input or in the
//! chosen folder, and never overwrite an existing file. An output the caller names exactly (`out_path`) may be
//! overwritten, but never an input.

use std::path::{Path, PathBuf};

use laqta_domain::color::Rgba;
use laqta_domain::effects::draw::Placement;
use laqta_domain::{Bitmap, ImageFormat};
use laqta_imaging::tools::combine::{Align, Orientation, combine};
use laqta_imaging::tools::compare::{Diff, compare};
use laqta_imaging::tools::convert::{convert, resize_bytes};
use laqta_imaging::tools::icon::encode_ico;
use laqta_imaging::tools::metadata::{read_exif, strip_metadata};
use laqta_imaging::tools::split::split_grid;
use laqta_imaging::tools::watermark::watermark_image;
use laqta_imaging::{Codecs, EncodeOptions, sniff, thumbnail};
use laqta_ports::{AppError, MediaInfo};

use super::common::{codec_error, ensure_not_input, file_stem, io_error, read_file, save_named, write_file};
use crate::Ports;
use crate::files;
use crate::task::compute;

/// JPEG quality for outputs whose quality the caller does not choose.
const JPEG_QUALITY: u8 = 90;

/// Bytes read to tell which format a file is. The longest signature is 12 bytes.
const SNIFF_BYTES: u64 = 16;

/// Joins `paths` in order into one image written to `out_path`, in the format its extension names. Gaps and
/// uncovered space take a transparent background, or white for formats without alpha.
pub async fn combine_files(
    ports: &Ports,
    paths: &[PathBuf],
    orientation: Orientation,
    align: Align,
    spacing: u32,
    out_path: &Path,
) -> Result<PathBuf, AppError> {
    let format = format_for_path(out_path)?;
    ensure_not_input(out_path, paths)?;
    let mut sources = Vec::with_capacity(paths.len());
    for path in paths {
        sources.push(read_file(ports, path).await?);
    }
    let codecs = ports.codecs.clone();
    let background = if matches!(format, ImageFormat::Jpeg | ImageFormat::Bmp) { Rgba::WHITE } else { Rgba::TRANSPARENT };
    let bytes = compute(ports.spawner.as_ref(), move || {
        let images = decode_all(&codecs, &sources)?;
        let strip = combine(&images, orientation, align, spacing, background, false);
        encode(&codecs, &strip, format, JPEG_QUALITY)
    })
    .await?;
    write_file(ports, out_path.to_path_buf(), bytes).await?;
    Ok(out_path.to_path_buf())
}

/// Cuts the image at `path` into `rows` by `cols` tiles, saved in `out_dir` as `<stem>_001`, `<stem>_002` and so on, in
/// row order, in the input's format (PNG when that cannot be encoded). The counts are clamped to the image's size.
pub async fn split_file(ports: &Ports, path: &Path, rows: u32, cols: u32, out_dir: &Path) -> Result<Vec<PathBuf>, AppError> {
    let source = read_file(ports, path).await?;
    let format = output_format(ports, &source);
    let codecs = ports.codecs.clone();
    let tiles = compute(ports.spawner.as_ref(), move || {
        let image = codecs.decode(&source).map_err(codec_error)?;
        split_grid(&image, rows, cols).iter().map(|tile| encode(&codecs, tile, format, JPEG_QUALITY)).collect::<Result<Vec<_>, _>>()
    })
    .await?;
    let stem = file_stem(path);
    let mut outputs = Vec::with_capacity(tiles.len());
    for (index, tile) in tiles.into_iter().enumerate() {
        let suffix = format!("_{:03}", index + 1);
        outputs.push(save_named(ports, out_dir, &stem, &suffix, format.extension(), tile).await?);
    }
    Ok(outputs)
}

/// Resizes each image to `width` by `height`, or with `keep_aspect` to the largest size that fits inside that box (a 0
/// side is unbounded). Without `keep_aspect` a 0 side keeps the source size on that axis. Saved as `<stem>_resized`.
pub async fn resize_files(
    ports: &Ports,
    paths: &[PathBuf],
    width: u32,
    height: u32,
    keep_aspect: bool,
    out_dir: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mut outputs = Vec::with_capacity(paths.len());
    for path in paths {
        let source = read_file(ports, path).await?;
        let format = output_format(ports, &source);
        let codecs = ports.codecs.clone();
        let bytes = compute(ports.spawner.as_ref(), move || {
            resize_bytes(&source, &codecs, width, height, keep_aspect, options(format, JPEG_QUALITY)).map_err(codec_error)
        })
        .await?;
        outputs.push(save_named(ports, out_dir, &file_stem(path), "_resized", format.extension(), bytes).await?);
    }
    Ok(outputs)
}

/// Re-encodes each image as `format` at `quality` (1..=100, JPEG only). Saved as `<stem>.<new extension>`, or with a
/// number after the stem when that name is taken, so converting a PNG to PNG in place keeps the original.
pub async fn convert_files(
    ports: &Ports,
    paths: &[PathBuf],
    format: ImageFormat,
    quality: u8,
    out_dir: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mut outputs = Vec::with_capacity(paths.len());
    for path in paths {
        let source = read_file(ports, path).await?;
        let codecs = ports.codecs.clone();
        let bytes =
            compute(ports.spawner.as_ref(), move || convert(&source, &codecs, options(format, quality)).map_err(codec_error)).await?;
        outputs.push(save_named(ports, out_dir, &file_stem(path), "", format.extension(), bytes).await?);
    }
    Ok(outputs)
}

/// Scales each image down to fit `max_width` by `max_height` (a 0 bound is unbounded). An image that already fits keeps its
/// size. Saved as `<stem>_thumb`.
pub async fn thumbnail_files(
    ports: &Ports,
    paths: &[PathBuf],
    max_width: u32,
    max_height: u32,
    out_dir: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mut outputs = Vec::with_capacity(paths.len());
    for path in paths {
        let source = read_file(ports, path).await?;
        let format = output_format(ports, &source);
        let codecs = ports.codecs.clone();
        let bytes = compute(ports.spawner.as_ref(), move || {
            let image = codecs.decode(&source).map_err(codec_error)?;
            let small = thumbnail(&image, max_width, max_height);
            encode(&codecs, &small, format, JPEG_QUALITY)
        })
        .await?;
        outputs.push(save_named(ports, out_dir, &file_stem(path), "_thumb", format.extension(), bytes).await?);
    }
    Ok(outputs)
}

/// Draws the image at `mark_path` over each image, anchored at `placement`, with `opacity` (0..=1) on the mark. Saved as
/// `<stem>_watermark`.
pub async fn watermark_files(
    ports: &Ports,
    paths: &[PathBuf],
    mark_path: &Path,
    placement: Placement,
    opacity: f32,
    out_dir: &Path,
) -> Result<Vec<PathBuf>, AppError> {
    let mark_bytes = read_file(ports, mark_path).await?;
    let mut outputs = Vec::with_capacity(paths.len());
    for path in paths {
        let source = read_file(ports, path).await?;
        let format = output_format(ports, &source);
        let codecs = ports.codecs.clone();
        let mark = mark_bytes.clone();
        let bytes = compute(ports.spawner.as_ref(), move || {
            let base = codecs.decode(&source).map_err(codec_error)?;
            let mark = codecs.decode(&mark).map_err(codec_error)?;
            let marked = watermark_image(&base, &mark, placement, (0, 0), opacity, 100.0);
            encode(&codecs, &marked, format, JPEG_QUALITY)
        })
        .await?;
        outputs.push(save_named(ports, out_dir, &file_stem(path), "_watermark", format.extension(), bytes).await?);
    }
    Ok(outputs)
}

/// Compares two images pixel by pixel. A pixel counts as changed when a channel differs by more than `tolerance`.
pub async fn compare_files(ports: &Ports, a: &Path, b: &Path, tolerance: u8) -> Result<Diff, AppError> {
    let left = read_file(ports, a).await?;
    let right = read_file(ports, b).await?;
    let codecs = ports.codecs.clone();
    compute(ports.spawner.as_ref(), move || {
        let left = codecs.decode(&left).map_err(codec_error)?;
        let right = codecs.decode(&right).map_err(codec_error)?;
        Ok(compare(&left, &right, tolerance))
    })
    .await
}

/// Writes an ICO file to `out_path` with one entry per size in `sizes` (each 1..=256).
pub async fn make_icon(ports: &Ports, path: &Path, sizes: &[u32], out_path: &Path) -> Result<PathBuf, AppError> {
    ensure_not_input(out_path, &[path.to_path_buf()])?;
    let source = read_file(ports, path).await?;
    let codecs = ports.codecs.clone();
    let sizes = sizes.to_vec();
    let bytes = compute(ports.spawner.as_ref(), move || {
        let image = codecs.decode(&source).map_err(codec_error)?;
        encode_ico(&image, &sizes).map_err(codec_error)
    })
    .await?;
    write_file(ports, out_path.to_path_buf(), bytes).await?;
    Ok(out_path.to_path_buf())
}

/// Copies each JPEG, PNG or WebP file without its metadata, saved as `<stem>_stripped` in the same format.
pub async fn strip_metadata_files(ports: &Ports, paths: &[PathBuf], out_dir: &Path) -> Result<Vec<PathBuf>, AppError> {
    let mut outputs = Vec::with_capacity(paths.len());
    for path in paths {
        let source = read_file(ports, path).await?;
        let format = output_format(ports, &source);
        let bytes = compute(ports.spawner.as_ref(), move || strip_metadata(&source).map_err(codec_error)).await?;
        outputs.push(save_named(ports, out_dir, &file_stem(path), "_stripped", format.extension(), bytes).await?);
    }
    Ok(outputs)
}

/// The metadata of a file as `(name, value)` rows. An image gives its EXIF fields, which is empty when it has none. Any
/// other file is probed as media: duration, size, dimensions, frame rate and codecs.
pub async fn read_metadata(ports: &Ports, path: &Path) -> Result<Vec<(String, String)>, AppError> {
    let head = files::read_prefix(ports.spawner.as_ref(), path.to_path_buf(), SNIFF_BYTES).await.map_err(io_error)?;
    if sniff(&head).is_some() {
        let source = read_file(ports, path).await?;
        return compute(ports.spawner.as_ref(), move || Ok(read_exif(&source))).await;
    }
    let info = ports.media.probe(path).await?;
    Ok(media_rows(&info))
}

fn media_rows(info: &MediaInfo) -> Vec<(String, String)> {
    let mut rows = vec![
        ("Duration".to_owned(), format!("{:.2} s", info.duration_secs)),
        ("File size".to_owned(), format!("{} bytes", info.size_bytes)),
    ];
    if info.width > 0 && info.height > 0 {
        rows.push(("Dimensions".to_owned(), format!("{} x {}", info.width, info.height)));
    }
    if info.fps > 0.0 {
        rows.push(("Frame rate".to_owned(), format!("{:.2} fps", info.fps)));
    }
    if !info.video_codec.is_empty() {
        rows.push(("Video codec".to_owned(), info.video_codec.clone()));
    }
    if let Some(audio) = &info.audio_codec {
        rows.push(("Audio codec".to_owned(), audio.clone()));
    }
    rows
}

/// The format an output keeps: the input's own when it can be encoded, PNG otherwise.
fn output_format(ports: &Ports, source: &[u8]) -> ImageFormat {
    sniff(source).filter(|format| ports.codecs.can_encode(*format)).unwrap_or(ImageFormat::Png)
}

/// The format an output file name asks for.
fn format_for_path(path: &Path) -> Result<ImageFormat, AppError> {
    let extension = path.extension().map(|ext| ext.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
    let format = match extension.as_str() {
        "jpg" | "jpeg" => Some(ImageFormat::Jpeg),
        "tif" | "tiff" => Some(ImageFormat::Tiff),
        _ => ImageFormat::ALL.into_iter().find(|format| format.extension() == extension),
    };
    format.ok_or_else(|| AppError::Unsupported(format!("no image format for {}", path.display())))
}

fn options(format: ImageFormat, jpeg_quality: u8) -> EncodeOptions {
    EncodeOptions { format, jpeg_quality }
}

fn encode(codecs: &Codecs, image: &Bitmap, format: ImageFormat, jpeg_quality: u8) -> Result<Vec<u8>, AppError> {
    codecs.encode(image, &options(format, jpeg_quality)).map_err(codec_error)
}

fn decode_all(codecs: &Codecs, sources: &[Vec<u8>]) -> Result<Vec<Bitmap>, AppError> {
    sources.iter().map(|bytes| codecs.decode(bytes).map_err(codec_error)).collect()
}

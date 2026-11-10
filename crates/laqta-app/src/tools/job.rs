//! One entry for the tools window: a [`ToolJob`] in, a [`ToolOutput`] out.
//!
//! The UI describes the job and formats the output. The ports stay inside `laqta-app`, and a
//! panic in a tool becomes `AppError::Panic` here (ADR 020).

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;

use futures_lite::FutureExt as _;
use laqta_domain::ImageFormat;
use laqta_domain::effects::draw::Placement;
use laqta_imaging::tools::combine::{Align, Orientation};
use laqta_imaging::tools::compare::Diff;
use laqta_ports::{AppError, ClipboardContent, TranscodeSpec};

use super::{HashKind, IndexFormat, IndexOptions};
use crate::App;

/// A tool run the tools window asks for. Output paths are the caller's choice.
#[derive(Clone, Debug)]
pub enum ToolJob {
    Compare {
        a: PathBuf,
        b: PathBuf,
        tolerance: u8,
    },
    Icon {
        path: PathBuf,
        sizes: Vec<u32>,
        out: PathBuf,
    },
    Combine {
        paths: Vec<PathBuf>,
        orientation: Orientation,
        align: Align,
        spacing: u32,
        out: PathBuf,
    },
    Split {
        path: PathBuf,
        rows: u32,
        cols: u32,
        out_dir: PathBuf,
    },
    Resize {
        paths: Vec<PathBuf>,
        width: u32,
        height: u32,
        keep_aspect: bool,
        out_dir: PathBuf,
    },
    Convert {
        paths: Vec<PathBuf>,
        format: ImageFormat,
        quality: u8,
        out_dir: PathBuf,
    },
    Watermark {
        paths: Vec<PathBuf>,
        mark: PathBuf,
        placement: Placement,
        opacity: f32,
        out_dir: PathBuf,
    },
    Thumbnails {
        paths: Vec<PathBuf>,
        max_width: u32,
        max_height: u32,
        out_dir: PathBuf,
    },
    ConvertVideo {
        input: PathBuf,
        output: PathBuf,
        spec: TranscodeSpec,
    },
    TrimVideo {
        input: PathBuf,
        output: PathBuf,
        start_secs: f64,
        duration_secs: f64,
    },
    VideoThumbnails {
        input: PathBuf,
        out_dir: PathBuf,
        count: u32,
    },
    Gif {
        input: PathBuf,
        output: PathBuf,
        fps: u32,
        width: u32,
    },
    QrDecodeFile(PathBuf),
    Hash {
        path: PathBuf,
        kinds: Vec<HashKind>,
    },
    Metadata(PathBuf),
    StripMetadata {
        paths: Vec<PathBuf>,
        out_dir: PathBuf,
    },
    Index {
        dir: PathBuf,
        format: IndexFormat,
        options: IndexOptions,
    },
    /// What the clipboard holds now.
    Clipboard,
    /// Sets or removes the border of the first window whose title contains `title` (ignoring case).
    Borderless {
        title: String,
        on: bool,
    },
}

/// What a [`ToolJob`] produced.
#[derive(Debug)]
pub enum ToolOutput {
    /// The job wrote the output the caller named.
    Done,
    Path(PathBuf),
    Paths(Vec<PathBuf>),
    Diff(Diff),
    /// Decoded codes, hashes are separate.
    Lines(Vec<String>),
    Hashes(Vec<(HashKind, String)>),
    Pairs(Vec<(String, String)>),
    Text(String),
    Clipboard(Option<ClipboardContent>),
    /// The title of the window the job changed. `None`: no window matched.
    Window(Option<String>),
}

impl App {
    /// Runs one tool job. A panic inside it comes back as `AppError::Panic`.
    pub async fn run_tool(&self, job: ToolJob) -> Result<ToolOutput, AppError> {
        match AssertUnwindSafe(self.tool_job(job)).catch_unwind().await {
            Ok(result) => result,
            Err(payload) => Err(AppError::Panic(crate::task::panic_message(&*payload))),
        }
    }

    async fn tool_job(&self, job: ToolJob) -> Result<ToolOutput, AppError> {
        use ToolOutput as O;
        let ports = &self.ports;
        Ok(match job {
            ToolJob::Compare { a, b, tolerance } => O::Diff(super::compare_files(ports, &a, &b, tolerance).await?),
            ToolJob::Icon { path, sizes, out } => O::Path(super::make_icon(ports, &path, &sizes, &out).await?),
            ToolJob::Combine { paths, orientation, align, spacing, out } => {
                O::Path(super::combine_files(ports, &paths, orientation, align, spacing, &out).await?)
            }
            ToolJob::Split { path, rows, cols, out_dir } => O::Paths(super::split_file(ports, &path, rows, cols, &out_dir).await?),
            ToolJob::Resize { paths, width, height, keep_aspect, out_dir } => {
                O::Paths(super::resize_files(ports, &paths, width, height, keep_aspect, &out_dir).await?)
            }
            ToolJob::Convert { paths, format, quality, out_dir } => {
                O::Paths(super::convert_files(ports, &paths, format, quality, &out_dir).await?)
            }
            ToolJob::Watermark { paths, mark, placement, opacity, out_dir } => {
                O::Paths(super::watermark_files(ports, &paths, &mark, placement, opacity, &out_dir).await?)
            }
            ToolJob::Thumbnails { paths, max_width, max_height, out_dir } => {
                O::Paths(super::thumbnail_files(ports, &paths, max_width, max_height, &out_dir).await?)
            }
            ToolJob::ConvertVideo { input, output, spec } => {
                super::convert_video(ports, &input, &output, spec).await?;
                O::Done
            }
            ToolJob::TrimVideo { input, output, start_secs, duration_secs } => {
                super::trim_video(ports, &input, &output, start_secs, duration_secs).await?;
                O::Done
            }
            ToolJob::VideoThumbnails { input, out_dir, count } => O::Paths(super::video_thumbnails(ports, &input, &out_dir, count).await?),
            ToolJob::Gif { input, output, fps, width } => {
                super::make_gif(ports, &input, &output, fps, width).await?;
                O::Done
            }
            ToolJob::QrDecodeFile(path) => O::Lines(super::qr_decode_file(ports, &path).await?),
            ToolJob::Hash { path, kinds } => O::Hashes(super::hash_file(ports, &path, &kinds).await?),
            ToolJob::Metadata(path) => O::Pairs(super::read_metadata(ports, &path).await?),
            ToolJob::StripMetadata { paths, out_dir } => O::Paths(super::strip_metadata_files(ports, &paths, &out_dir).await?),
            ToolJob::Index { dir, format, options } => O::Text(super::index_folder(ports, &dir, format, options).await?),
            ToolJob::Clipboard => O::Clipboard(ports.clipboard.get()?),
            ToolJob::Borderless { title, on } => {
                let needle = title.to_lowercase();
                let target =
                    ports.hover.targets().await?.into_iter().find(|w| !needle.is_empty() && w.title.to_lowercase().contains(&needle));
                match target {
                    Some(target) => {
                        ports.window_ops.set_borderless(&target, on)?;
                        O::Window(Some(target.title))
                    }
                    None => O::Window(None),
                }
            }
        })
    }
}

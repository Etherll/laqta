//! Choosing file names and writing the saved image and its thumbnail (M1 §4.3). Choosing a name and writing
//! it happen under the save slot, so two jobs never pick the same name.

use std::io;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::Ordering;

use jiff::tz::TimeZone;
use laqta_domain::naming::{self, NameContext};
use laqta_domain::workflow::{FileExistsAction, TaskSettings};
use laqta_ports::AppError;

use super::encode::Encoded;
use super::{CaptureInfo, Context};
use crate::files;

/// A file this job saved.
#[derive(Clone, Debug)]
pub(super) struct SavedFile {
    pub(super) path: PathBuf,
}

/// What saving the image did.
pub(super) enum Outcome {
    Saved(SavedFile),
    /// `FileExistsAction::Skip` found the first name taken. Nothing was written.
    Skipped(PathBuf),
}

/// Picks the image's file name and writes the image. The `%i` counter advances once per file written.
pub(super) async fn image(
    cx: &Context,
    settings: &TaskSettings,
    info: &CaptureInfo,
    size: (u32, u32),
    encoded: Encoded,
) -> Result<Outcome, AppError> {
    let Some(dir) = settings.screenshots_dir.as_deref() else {
        return Err(AppError::Io("no screenshots folder".to_owned()));
    };
    let _slot = Arc::clone(&cx.saves).acquire().await;
    let spawner = cx.ports.spawner.as_ref();
    let candidates = image_candidates(cx, settings, info, dir, size, encoded.format.extension());
    let first = candidates.first().cloned().ok_or_else(|| AppError::Io("no file name".to_owned()))?;
    let path = match settings.image.file_exists {
        FileExistsAction::Overwrite => first,
        FileExistsAction::Skip => {
            if files::exists(spawner, first.clone()).await {
                return Ok(Outcome::Skipped(first));
            }
            first
        }
        FileExistsAction::AppendNumber | FileExistsAction::Ask => files::first_free(spawner, candidates).await.map_err(io_error)?,
    };
    files::save_atomic(spawner, path.clone(), encoded.bytes).await.map_err(io_error)?;
    cx.counter.fetch_add(1, Ordering::SeqCst);
    Ok(Outcome::Saved(SavedFile { path }))
}

/// Writes the thumbnail beside `saved`, as `<stem><suffix>.<ext>`, under the first free name.
pub(super) async fn thumbnail(cx: &Context, saved: &SavedFile, suffix: &str, encoded: Encoded) -> Result<PathBuf, AppError> {
    let (Some(dir), Some(stem)) = (saved.path.parent(), saved.path.file_stem()) else {
        return Err(AppError::Io("saved file has no folder".to_owned()));
    };
    let stem = format!("{}{suffix}", stem.to_string_lossy());
    let _slot = Arc::clone(&cx.saves).acquire().await;
    let spawner = cx.ports.spawner.as_ref();
    let candidates: Vec<PathBuf> = naming::candidates(dir, "", &stem, encoded.format.extension()).collect();
    let path = files::first_free(spawner, candidates).await.map_err(io_error)?;
    files::save_atomic(spawner, path.clone(), encoded.bytes).await.map_err(io_error)?;
    Ok(path)
}

/// Writes `bytes` to the path the user chose in the save dialog. It takes the save slot and counts as a file
/// written, like any other save.
pub(super) async fn chosen(cx: &Context, path: PathBuf, bytes: Vec<u8>) -> Result<(), AppError> {
    let _slot = Arc::clone(&cx.saves).acquire().await;
    files::save_atomic(cx.ports.spawner.as_ref(), path, bytes).await.map_err(io_error)?;
    cx.counter.fetch_add(1, Ordering::SeqCst);
    Ok(())
}

/// The candidate paths for the image, in order. The name and subfolder patterns are expanded here, reading
/// the clock, the counter and the random source once. Nothing awaits in here, so the random source never
/// lives across an await point.
fn image_candidates(cx: &Context, settings: &TaskSettings, info: &CaptureInfo, dir: &Path, size: (u32, u32), ext: &str) -> Vec<PathBuf> {
    with_name_context(cx, info, Some(size), |ctx| {
        let stem = naming::file_name(pattern(settings, info), ctx);
        let folder = subfolder(&naming::expand(&settings.naming.subfolder, ctx));
        naming::candidates(dir, &folder, &stem, ext).collect()
    })
}

/// A file name, with `ext`, for a file that has no path on disk (an upload of bytes that were not saved).
pub(super) fn generated_name(cx: &Context, settings: &TaskSettings, info: &CaptureInfo, size: Option<(u32, u32)>, ext: &str) -> String {
    with_name_context(cx, info, size, |ctx| format!("{}.{ext}", naming::file_name(pattern(settings, info), ctx)))
}

/// The name pattern for a job: the window pattern when the capture has a window title.
fn pattern<'a>(settings: &'a TaskSettings, info: &CaptureInfo) -> &'a str {
    if info.window_title.is_empty() { &settings.naming.pattern } else { &settings.naming.window_pattern }
}

/// Runs `work` with this job's name context: the clock, the counter, the window, the machine and the random
/// source. Nothing awaits inside, so the random source never lives across an await point.
fn with_name_context<T>(cx: &Context, info: &CaptureInfo, size: Option<(u32, u32)>, work: impl FnOnce(&mut NameContext<'_>) -> T) -> T {
    let ports = cx.ports.as_ref();
    let now = ports.clock.now();
    let unix_seconds = now.to_zoned(TimeZone::UTC).map_or(0, |zoned| zoned.timestamp().as_second());
    let mut rng = || ports.rng.next_u64();
    let mut ctx = NameContext {
        now,
        unix_seconds,
        window_title: &info.window_title,
        process_name: &info.process_name,
        width: size.map(|size| size.0),
        height: size.map(|size| size.1),
        counter: cx.counter.load(Ordering::SeqCst),
        user_name: &ports.host.user_name,
        user_login: &ports.host.user_login,
        computer_name: &ports.host.computer_name,
        random_file_lines: &[],
        rng: &mut rng,
    };
    work(&mut ctx)
}

/// The expanded subfolder as relative path segments, each cleaned the way a file name is. Empty, `.` and
/// `..` segments are dropped, so the image stays inside the screenshots folder.
fn subfolder(expanded: &str) -> String {
    expanded
        .split(['/', '\\'])
        .map(|segment| segment.chars().map(|c| if c.is_control() || ":*?\"<>|".contains(c) { '_' } else { c }).collect::<String>())
        .map(|segment| segment.trim_end_matches(['.', ' ']).to_owned())
        .filter(|segment| !segment.is_empty())
        .collect::<Vec<_>>()
        .join("/")
}

fn io_error(error: io::Error) -> AppError {
    AppError::Io(error.to_string())
}

//! The `Media` adapter: FFmpeg as a subprocess (ADR 005). It finds or downloads FFmpeg, builds
//! argument lists as data (`argv`), runs them without a shell, and records through segments
//! (`process`). It never calls platform code: it only maps the screen and audio inputs it is given.
#![forbid(unsafe_code)]

pub mod argv;
mod download;
mod locate;
mod media;
mod process;

pub use download::{can_download_ffmpeg, download_ffmpeg};
pub use locate::{FfmpegPaths, find_ffmpeg};
pub use media::FfmpegMedia;
pub use process::FfmpegRecorder;

use std::path::Path;
use std::process::Command;

use laqta_ports::{BoxFuture, PortError};

/// Runs `work` on a new thread and returns its result to the caller's executor. Blocking process
/// and file work goes through here, so no executor thread waits on a child process.
pub(crate) fn run_blocking<T, F>(work: F) -> BoxFuture<'static, Result<T, PortError>>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, PortError> + Send + 'static,
{
    let (send, receive) = futures_channel::oneshot::channel();
    let spawned = std::thread::Builder::new().name("laqta-ffmpeg".to_owned()).spawn(move || {
        let _ = send.send(work());
    });
    Box::pin(async move {
        if let Err(err) = spawned {
            return Err(PortError::Failed(format!("could not start a thread: {err}")));
        }
        match receive.await {
            Ok(result) => result,
            Err(_) => Err(PortError::Failed("the FFmpeg worker ended without a result".into())),
        }
    })
}

/// A command for an FFmpeg tool. On Windows it does not open a console window.
#[cfg(windows)]
pub(crate) fn ffmpeg_command(program: &Path) -> Command {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let mut command = Command::new(program);
    command.creation_flags(CREATE_NO_WINDOW);
    command
}

/// A command for an FFmpeg tool.
#[cfg(not(windows))]
pub(crate) fn ffmpeg_command(program: &Path) -> Command {
    Command::new(program)
}

/// The end of FFmpeg's stderr, for error messages: at most 600 characters, trimmed.
pub(crate) fn tail(bytes: &[u8]) -> String {
    const LIMIT: usize = 600;
    let text = String::from_utf8_lossy(bytes);
    let trimmed = text.trim();
    let start = trimmed.char_indices().rev().nth(LIMIT - 1).map_or(0, |(index, _)| index);
    trimmed[start..].to_owned()
}

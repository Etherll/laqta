//! The driver of one recording: it owns the encoder, takes control commands, ticks every second while the recording
//! runs, and ends the recording on stop, abort, the fixed duration, or when the encoder exits on its own. It runs as
//! a spawned future, so it never holds a lock across an await.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use futures_lite::{StreamExt, future};
use laqta_ports::{AppError, TaskEvent, TaskId};

use super::{Control, Live, SECOND};
use crate::dispatcher::App;
use crate::files;
use crate::task::{TaskJob, UploadSource};

/// What the driver waits for: a command, or the next second.
enum Next {
    Command(Option<Control>),
    Second,
}

/// Runs one recording until it ends. Pause and resume toggle the encoder, and each second of running emits a
/// tick. A failed encoder keeps its partial file.
pub(super) async fn run(app: Arc<App>, id: TaskId, live: Live) {
    // "Highlight the mouse while recording": on for this recording only, unless it was already on.
    let highlight = live.ts.recording.highlight_mouse && !app.ports.highlighter.is_enabled();
    if highlight && let Err(error) = app.ports.highlighter.set_enabled(true) {
        tracing::warn!("mouse highlighter unavailable: {error}");
    }
    drive(&app, id, live).await;
    if highlight {
        let _ = app.ports.highlighter.set_enabled(false);
    }
}

async fn drive(app: &Arc<App>, id: TaskId, mut live: Live) {
    let mut elapsed = 0u32;
    let mut paused = false;
    loop {
        let next = if paused {
            Next::Command(live.commands.next().await)
        } else {
            future::or(async { Next::Command(live.commands.next().await) }, async {
                app.ports.clock.sleep(SECOND).await;
                Next::Second
            })
            .await
        };
        match next {
            Next::Command(None | Some(Control::Abort)) => return abort_recording(app, id, live).await,
            Next::Command(Some(Control::Stop)) => return stop_recording(app, id, live).await,
            Next::Command(Some(control)) => {
                let want_pause = match control {
                    Control::Pause => true,
                    Control::Resume => false,
                    Control::TogglePause => !paused,
                    // Stop and abort are handled above. Nothing changes for them here.
                    Control::Stop | Control::Abort => paused,
                };
                if want_pause == paused {
                    continue;
                }
                let result = if want_pause { live.handle.pause().await } else { live.handle.resume().await };
                match result {
                    Ok(()) => {
                        paused = want_pause;
                        let event = if want_pause { TaskEvent::RecordingPaused { id } } else { TaskEvent::RecordingResumed { id } };
                        app.emit(event);
                    }
                    Err(error) => return fail_recording(app, id, live, AppError::from(error)).await,
                }
            }
            Next::Second => {
                if !live.handle.is_alive() {
                    return fail_recording(app, id, live, AppError::Io("the recorder stopped unexpectedly".to_owned())).await;
                }
                elapsed += 1;
                app.emit(TaskEvent::RecordingTick { id, elapsed_secs: elapsed });
                if live.fixed_secs > 0 && elapsed >= live.fixed_secs {
                    return stop_recording(app, id, live).await;
                }
                future::yield_now().await;
            }
        }
    }
}

/// Finishes the file, reports it and hands it to Task with the settings the recording started with.
async fn stop_recording(app: &App, id: TaskId, live: Live) {
    let Live { handle, output, ts, .. } = live;
    match handle.stop().await {
        Ok(path) => {
            app.release(id);
            app.emit(TaskEvent::RecordingStopped { id, path: path.clone() });
            app.task.submit(TaskJob::Upload { source: UploadSource::SavedFile(path), settings: ts });
        }
        Err(error) => {
            let partial = salvaged(app, output).await;
            app.release(id);
            app.emit_failed(id, AppError::from(error), Some(partial), &ts.general.toast);
        }
    }
}

/// The file to report when the encoder could not finish `output`. A GIF, WebP or APNG whose conversion failed leaves
/// the lossless recording next to the output, named by `kept_intermediate`. Any other failure reports `output`.
async fn salvaged(app: &App, output: PathBuf) -> PathBuf {
    let kept = kept_intermediate(&output);
    if files::exists(app.ports.spawner.as_ref(), kept.clone()).await { kept } else { output }
}

/// Where the media adapter leaves the intermediate of a failed animation: `<output name>.mkv` (laqta-media
/// `process.rs`, `kept_path`). `RecordingHandle::stop` returns only an error, so the path is found by this name.
fn kept_intermediate(output: &Path) -> PathBuf {
    let mut name = output.as_os_str().to_owned();
    name.push(".mkv");
    PathBuf::from(name)
}

/// Discards the recording: the encoder deletes its output.
async fn abort_recording(app: &App, id: TaskId, live: Live) {
    let Live { handle, ts, .. } = live;
    let result = handle.abort().await;
    app.release(id);
    let error = match result {
        Ok(()) => AppError::Cancelled,
        Err(error) => AppError::from(error),
    };
    app.emit_failed(id, error, None, &ts.general.toast);
}

/// The encoder failed or a command could not be applied. The encoder is stopped to finish the segments it has, and
/// the failure is reported with that partial file. Task gets nothing.
async fn fail_recording(app: &App, id: TaskId, live: Live, error: AppError) {
    let Live { handle, output, ts, .. } = live;
    let partial = match handle.stop().await {
        Ok(path) => path,
        Err(_) => salvaged(app, output).await,
    };
    app.release(id);
    app.emit_failed(id, error, Some(partial), &ts.general.toast);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_kept_intermediate_is_the_output_name_plus_mkv() {
        assert_eq!(kept_intermediate(Path::new("/v/Screencast 1.gif")), Path::new("/v/Screencast 1.gif.mkv"));
    }
}

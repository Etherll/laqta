//! The `Capture` job after admission: compose and encode, pick a name, save, copy (M0 §5.2).

use std::panic::AssertUnwindSafe;
use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::oneshot;
use laqta_domain::{Bitmap, Frame, Settings, naming};
use laqta_imaging::{EncodeError, compose_monitors, encode_png};
use laqta_ports::{AppError, Spawner, TaskEvent, TaskId, TaskStatus};

use super::panic_message;
use super::status::{Queue, Reporter};
use crate::dispatcher::Ports;
use crate::files;

pub(super) async fn capture(
    ports: Arc<Ports>,
    settings: Arc<Settings>,
    saves: Arc<Queue>,
    reporter: Reporter,
    id: TaskId,
    frames: Vec<Frame>,
) {
    reporter.status(id, TaskStatus::Preparing);
    reporter.status(id, TaskStatus::Working);

    let (composed, png) = match compose_and_encode(ports.spawner.as_ref(), frames).await {
        Ok(encoded) => encoded,
        Err(error) => {
            reporter.fail(id, error);
            return;
        }
    };
    let path = match save(&ports, saves, &settings, png).await {
        Ok(path) => path,
        Err(error) => {
            reporter.fail(id, error);
            return;
        }
    };
    reporter.emit(TaskEvent::Saved { id, path });

    match ports.clipboard.set_image(&composed) {
        Ok(()) => reporter.emit(TaskEvent::Copied { id }),
        Err(error) => reporter.emit(TaskEvent::CopyFailed { id, reason: error.to_string() }),
    }
    reporter.status(id, TaskStatus::Completed);
}

type Encoded = (Bitmap, Vec<u8>);

/// Composes and encodes on the compute pool. A panic there comes back as a value, because the
/// pool may drop the work instead of reporting it.
async fn compose_and_encode(spawner: &dyn Spawner, frames: Vec<Frame>) -> Result<Encoded, AppError> {
    let (tx, rx) = oneshot::channel::<Result<Result<Encoded, EncodeError>, String>>();
    spawner
        .run_compute(Box::new(move || {
            let outcome = std::panic::catch_unwind(AssertUnwindSafe(|| -> Result<Encoded, EncodeError> {
                let composed = compose_monitors(&frames);
                let png = encode_png(&composed)?;
                Ok((composed, png))
            }));
            let _ = tx.send(outcome.map_err(|payload| panic_message(&*payload)));
        }))
        .await;
    match rx.await {
        Ok(Ok(Ok(encoded))) => Ok(encoded),
        Ok(Ok(Err(error))) => Err(AppError::Io(error.to_string())),
        Ok(Err(message)) => Err(AppError::Panic(message)),
        Err(oneshot::Canceled) => Err(AppError::Cancelled),
    }
}

/// Picks the first free name and writes the file. The save slot covers both steps, so two jobs
/// never pick the same name.
async fn save(ports: &Ports, saves: Arc<Queue>, settings: &Settings, png: Vec<u8>) -> Result<PathBuf, AppError> {
    let candidates: Vec<PathBuf> = naming::capture_candidates(&settings.screenshots_dir, &ports.clock.now()).collect();
    let _save = saves.acquire().await;
    let path = files::first_free(ports.spawner.as_ref(), candidates).await.map_err(io_error)?;
    files::save_atomic(ports.spawner.as_ref(), path.clone(), png).await.map_err(io_error)?;
    Ok(path)
}

fn io_error(error: std::io::Error) -> AppError {
    AppError::Io(error.to_string())
}

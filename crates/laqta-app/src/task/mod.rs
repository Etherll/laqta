//! The Task module owns the task lifecycle (ADR 011). Submitting a job emits `InQueue` and returns;
//! the job waits for a capture slot, runs, and reports through `TaskEvent`s. A panic in the job
//! body becomes a `Failed` event (ADR 020).

mod run;
mod status;

use std::any::Any;
use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use async_broadcast::Sender;
use futures_lite::FutureExt;
use laqta_domain::{Frame, Settings};
use laqta_ports::{AppError, BoxFuture, TaskEvent, TaskId, TaskStatus};

use crate::dispatcher::Ports;
use status::{Queue, Reporter};

/// Capture jobs that may run at once. Further jobs wait in `InQueue`.
const CAPTURE_SLOTS: usize = 2;
/// Choosing a file name and writing it happen one at a time, so two jobs never pick the same name.
const SAVE_SLOTS: usize = 1;

/// Work the dispatcher hands to Task.
#[derive(Clone, Debug)]
pub enum TaskJob {
    Capture { frames: Vec<Frame> },
}

pub(crate) struct Task {
    ports: Arc<Ports>,
    settings: Arc<Settings>,
    reporter: Reporter,
    capture_slots: Arc<Queue>,
    save_slots: Arc<Queue>,
    next_id: AtomicU64,
}

impl Task {
    pub(crate) fn new(ports: Arc<Ports>, settings: Arc<Settings>, events: Sender<TaskEvent>) -> Task {
        Task {
            ports,
            settings,
            reporter: Reporter::new(events),
            capture_slots: Arc::new(Queue::new(CAPTURE_SLOTS)),
            save_slots: Arc::new(Queue::new(SAVE_SLOTS)),
            next_id: AtomicU64::new(1),
        }
    }

    /// Emits `InQueue` and hands the job to the spawner. The job runs later.
    pub(crate) fn submit(&self, job: TaskJob) -> TaskId {
        let id = TaskId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.reporter.status(id, TaskStatus::InQueue);
        self.ports.spawner.spawn(self.job(id, job));
        id
    }

    fn job(&self, id: TaskId, job: TaskJob) -> BoxFuture<'static, ()> {
        let capture_slots = Arc::clone(&self.capture_slots);
        let save_slots = Arc::clone(&self.save_slots);
        let ports = Arc::clone(&self.ports);
        let settings = Arc::clone(&self.settings);
        let reporter = self.reporter.clone();
        Box::pin(async move {
            // The slot is taken and released outside the guarded body (ADR 020).
            let _slot = capture_slots.acquire().await;
            let TaskJob::Capture { frames } = job;
            let body = run::capture(ports, settings, save_slots, reporter.clone(), id, frames);
            if let Err(payload) = AssertUnwindSafe(body).catch_unwind().await {
                reporter.fail(id, AppError::Panic(panic_message(&*payload)));
            }
        })
    }
}

fn panic_message(payload: &(dyn Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_owned()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic".to_owned()
    }
}

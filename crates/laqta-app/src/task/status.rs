//! Admission and status. `Queue` decides when a job leaves `InQueue`. `Reporter` sends the status
//! changes and results as `TaskEvent`s.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_broadcast::Sender;
use futures_channel::oneshot;
use laqta_ports::{AppError, TaskEvent, TaskId, TaskStatus};

/// Admits at most `slots` jobs at once. Waiting jobs are admitted first in, first out.
pub(super) struct Queue {
    slots: usize,
    state: Mutex<QueueState>,
}

#[derive(Default)]
struct QueueState {
    running: usize,
    waiting: VecDeque<oneshot::Sender<()>>,
}

impl Queue {
    pub(super) fn new(slots: usize) -> Queue {
        Queue { slots, state: Mutex::new(QueueState::default()) }
    }

    fn lock(&self) -> MutexGuard<'_, QueueState> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Waits for a free slot. The slot is held until the returned `Permit` is dropped.
    pub(super) async fn acquire(self: Arc<Self>) -> Permit {
        let wait = {
            let mut state = self.lock();
            if state.running < self.slots {
                state.running += 1;
                None
            } else {
                let (tx, rx) = oneshot::channel();
                state.waiting.push_back(tx);
                Some(rx)
            }
        };
        if let Some(rx) = wait {
            // The releasing permit hands its slot to this job, so `running` already counts it.
            // The sender lives in `self`, which this future keeps alive, so the wait is not cancelled.
            let _ = rx.await;
        }
        Permit { queue: self }
    }
}

/// A held slot. Dropping it hands the slot to the next waiter, or frees it.
pub(super) struct Permit {
    queue: Arc<Queue>,
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut state = self.queue.lock();
        while let Some(next) = state.waiting.pop_front() {
            if next.send(()).is_ok() {
                return;
            }
        }
        state.running -= 1;
    }
}

/// Jobs that have not finished, with their stop flags. A job leaves the map when it finishes, or when a
/// stop request finds it still queued.
#[derive(Default)]
pub(super) struct Jobs {
    entries: Mutex<HashMap<TaskId, Entry>>,
}

struct Entry {
    running: bool,
    stop: Arc<AtomicBool>,
}

/// What a stop request found.
pub(super) enum Stop {
    Unknown,
    /// Still waiting for a capture slot. It is removed, so it never runs.
    Queued,
    /// Running. Its flag is set, and it stops at its next checkpoint.
    Running,
}

impl Jobs {
    fn lock(&self) -> MutexGuard<'_, HashMap<TaskId, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(super) fn add(&self, id: TaskId) {
        self.lock().insert(id, Entry { running: false, stop: Arc::new(AtomicBool::new(false)) });
    }

    pub(super) fn contains(&self, id: TaskId) -> bool {
        self.lock().contains_key(&id)
    }

    /// Marks the job running and returns its stop flag. `None` if it was stopped while queued.
    pub(super) fn start(&self, id: TaskId) -> Option<Arc<AtomicBool>> {
        let mut entries = self.lock();
        let entry = entries.get_mut(&id)?;
        entry.running = true;
        Some(Arc::clone(&entry.stop))
    }

    pub(super) fn finish(&self, id: TaskId) {
        self.lock().remove(&id);
    }

    pub(super) fn request_stop(&self, id: TaskId) -> Stop {
        let mut entries = self.lock();
        let Some(entry) = entries.get(&id) else { return Stop::Unknown };
        if entry.running {
            entry.stop.store(true, Ordering::SeqCst);
            return Stop::Running;
        }
        entries.remove(&id);
        Stop::Queued
    }

    /// Every unfinished job, lowest id first.
    pub(super) fn ids(&self) -> Vec<TaskId> {
        let mut ids: Vec<TaskId> = self.lock().keys().copied().collect();
        ids.sort_by_key(|id| id.0);
        ids
    }
}

/// Sends task events. Sending never blocks or fails: with no listener the event is dropped.
#[derive(Clone)]
pub(super) struct Reporter {
    events: Sender<TaskEvent>,
}

impl Reporter {
    pub(super) fn new(events: Sender<TaskEvent>) -> Reporter {
        Reporter { events }
    }

    pub(super) fn status(&self, id: TaskId, status: TaskStatus) {
        self.emit(TaskEvent::StatusChanged { id, status });
    }

    /// The error, then `Failed`.
    pub(super) fn fail(&self, id: TaskId, error: AppError) {
        self.emit(TaskEvent::Failed { id, error });
        self.status(id, TaskStatus::Failed);
    }

    pub(super) fn emit(&self, event: TaskEvent) {
        let _ = self.events.try_broadcast(event);
    }
}

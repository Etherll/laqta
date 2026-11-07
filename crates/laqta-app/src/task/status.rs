//! Admission and status. `Queue` decides when a job leaves `InQueue`. `Reporter` sends the status
//! changes and results as `TaskEvent`s.

use std::collections::VecDeque;
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

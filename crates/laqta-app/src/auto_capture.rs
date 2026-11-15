//! Auto capture: a timer on `Clock::sleep` that dispatches `settings.auto_capture.action`
//! every interval, `count` times or until stopped.

use std::panic::AssertUnwindSafe;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, MutexGuard, PoisonError};
use std::time::Duration;

use futures_lite::FutureExt;
use laqta_domain::Action;
use laqta_domain::settings::AutoCaptureOptions;
use laqta_ports::{AppError, Toast};

use crate::dispatcher::{App, Dispatched};

/// Runs in a row that fail before auto capture gives up, so a capture that cannot work does not repeat forever.
const MAX_FAILURES_IN_A_ROW: u32 = 3;

impl App {
    /// `Some(true)` starts, `Some(false)` stops, `None` toggles. Starting while one runs does nothing.
    pub(crate) fn auto_capture(self: &Arc<Self>, start: Option<bool>) -> Result<Dispatched, AppError> {
        let mut slot = self.auto_slot();
        let running = slot.is_some();
        match (start.unwrap_or(!running), running) {
            (true, false) => {
                let options = self.settings.get().auto_capture.clone();
                // The loop would dispatch into itself, so these actions cannot be the captured one.
                if is_auto_capture(options.action) {
                    return Err(AppError::Unsupported("auto capture cannot run an auto capture action".into()));
                }
                let stop = Arc::new(AtomicBool::new(false));
                *slot = Some(Arc::clone(&stop));
                drop(slot);
                self.spawn_auto_capture(stop, options);
            }
            (false, true) => {
                if let Some(stop) = slot.take() {
                    stop.store(true, Ordering::Release);
                }
            }
            _ => {}
        }
        Ok(Dispatched::Done)
    }

    /// Whether an auto capture is running.
    pub fn auto_capture_running(&self) -> bool {
        self.auto_slot().is_some()
    }

    fn auto_slot(&self) -> MutexGuard<'_, Option<Arc<AtomicBool>>> {
        self.auto_capture.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn spawn_auto_capture(self: &Arc<Self>, stop: Arc<AtomicBool>, options: AutoCaptureOptions) {
        let interval = Duration::from_millis(u64::from(
            options.interval_ms.clamp(AutoCaptureOptions::MIN_INTERVAL_MS, AutoCaptureOptions::MAX_INTERVAL_MS),
        ));
        let app = Arc::clone(self);
        self.ports.spawner.spawn(Box::pin(async move {
            // A panic in a run ends the loop and frees the slot (ADR 020).
            if AssertUnwindSafe(app.run_auto_capture(&stop, options, interval)).catch_unwind().await.is_err() {
                tracing::error!("auto capture ended by a panic");
            }
            app.end_auto_capture(&stop);
        }));
    }

    /// The loop: one run, then a wait, `count` times (0 = until stopped). The stop flag is checked before each run.
    async fn run_auto_capture(self: &Arc<Self>, stop: &AtomicBool, options: AutoCaptureOptions, interval: Duration) {
        let mut runs = 0u32;
        let mut failures = 0u32;
        while !stop.load(Ordering::Acquire) {
            match self.dispatch(options.action, options.workflow).await {
                Ok(_) => failures = 0,
                Err(AppError::Cancelled) => break,
                // Another selection is open: this run is skipped. It is no failure, and it does not clear the
                // failures before it either.
                Err(AppError::Busy) => tracing::debug!(action = ?options.action, "auto capture run skipped: a selection is open"),
                Err(error) => {
                    tracing::warn!(%error, action = ?options.action, "auto capture run failed");
                    failures += 1;
                    if failures >= MAX_FAILURES_IN_A_ROW {
                        self.auto_capture_gave_up(&error);
                        break;
                    }
                }
            }
            runs += 1;
            if options.count != 0 && runs >= options.count {
                break;
            }
            self.ports.clock.sleep(interval).await;
        }
    }

    /// Tells the user that the loop stopped on its own, and why.
    fn auto_capture_gave_up(&self, error: &AppError) {
        let toast = Toast {
            title: Action::AutoCapture.label().to_owned(),
            body: format!("Stopped after {MAX_FAILURES_IN_A_ROW} captures in a row failed: {error}"),
            image: None,
            path: None,
            url: None,
            options: self.settings.get().defaults.general.toast.clone(),
        };
        if let Err(error) = self.ports.notifier.toast(toast) {
            tracing::warn!(%error, "auto capture: the stop notice was not shown");
        }
    }

    /// Frees the slot when it still holds this loop's flag. A stop may have cleared it already, and a later start
    /// may hold another flag that must stay.
    fn end_auto_capture(&self, stop: &Arc<AtomicBool>) {
        let mut slot = self.auto_slot();
        if slot.as_ref().is_some_and(|current| Arc::ptr_eq(current, stop)) {
            *slot = None;
        }
    }
}

fn is_auto_capture(action: Action) -> bool {
    matches!(action, Action::AutoCapture | Action::StartAutoCapture | Action::StopAutoCapture)
}

//! Auto capture (M6 D5): a timer on `Clock::sleep` that dispatches `settings.auto_capture.action`
//! every interval, `count` times or until stopped.

use std::sync::Arc;

use laqta_ports::AppError;

use crate::dispatcher::{App, Dispatched};

impl App {
    /// `Some(true)` starts, `Some(false)` stops, `None` toggles.
    pub(crate) fn auto_capture(self: &Arc<Self>, _start: Option<bool>) -> Result<Dispatched, AppError> {
        Err(AppError::Unsupported("auto capture".into()))
    }

    /// Whether an auto capture is running.
    pub fn auto_capture_running(&self) -> bool {
        self.auto_capture.lock().map(|g| g.is_some()).unwrap_or(false)
    }
}

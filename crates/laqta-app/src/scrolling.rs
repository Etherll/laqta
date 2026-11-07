//! Scrolling capture (M6 D4): select a region, then capture it, scroll the window under it through
//! `InputSynth`, and stitch each frame with `laqta_imaging::stitch` until the page stops moving.

use std::sync::Arc;

use laqta_domain::workflow::{TaskSettings, WorkflowId};
use laqta_ports::AppError;

use crate::dispatcher::{App, Dispatched};

impl App {
    pub(crate) async fn scrolling_capture(
        self: &Arc<Self>,
        _ts: TaskSettings,
        _workflow: Option<WorkflowId>,
    ) -> Result<Dispatched, AppError> {
        Err(AppError::Unsupported("scrolling capture".into()))
    }
}

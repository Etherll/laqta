//! Application layer: the dispatcher and the Task module (ADR 011, ADR 020). File access stays in the
//! private `files` module (ADR 024).
#![forbid(unsafe_code)]

mod dispatcher;
mod files;
mod region;
mod task;

pub use dispatcher::{App, Dispatched, Ports};
pub use task::TaskJob;
pub use task::{CaptureImage, CaptureInfo};

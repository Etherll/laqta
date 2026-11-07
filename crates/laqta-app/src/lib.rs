//! Application layer: the dispatcher and the Task module (ADR 011, ADR 020). File access stays in the
//! private `files` module (ADR 024).
#![forbid(unsafe_code)]

mod dispatcher;
mod files;
mod recording;
mod region;
mod task;
mod tools;

pub use dispatcher::{App, Dispatched, Ports};
pub use region::Selection;
pub use task::TaskJob;
pub use task::UploadSource;
pub use task::{CaptureImage, CaptureInfo};
pub use tools::*;

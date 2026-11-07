//! Application layer: the dispatcher and the Task module (ADR 011, ADR 020). File access stays in the
//! private `files` module (ADR 024).
#![forbid(unsafe_code)]

mod auto_capture;
mod dispatcher;
mod files;
mod recording;
mod region;
mod scrolling;
mod task;
mod tool_routes;
mod tools;
mod watch;

pub use dispatcher::{App, Dispatched, Ports};
pub use region::Selection;
pub use task::TaskJob;
pub use task::UploadSource;
pub use task::{CaptureImage, CaptureInfo};
pub use tools::*;

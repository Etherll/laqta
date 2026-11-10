//! Application layer: the dispatcher and the Task module (ADR 011, ADR 020). File access stays in the
//! private `files` module (ADR 024).
#![forbid(unsafe_code)]

mod ai;
mod auto_capture;
mod dispatcher;
mod effects;
mod files;
mod history;
mod images;
mod recording;
mod region;
mod scrolling;
mod task;
mod tool_routes;
mod tools;
mod watch;

pub use dispatcher::{ActionInput, App, Dispatched, Ports};
pub use effects::EffectText;
pub use region::Selection;
pub use task::TaskJob;
pub use task::UploadSource;
pub use task::{CaptureImage, CaptureInfo};
pub use tools::*;

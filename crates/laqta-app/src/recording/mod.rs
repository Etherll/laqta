//! The Recording session (M4). The controls below are filled in by the recording-session task.

use crate::App;

impl App {
    /// Pauses the current recording, if any.
    pub fn recording_pause(&self) {}
    pub fn recording_resume(&self) {}
    /// Stops the current recording; the file goes to Task.
    pub fn recording_stop(&self) {}
    /// Stops and discards the current recording.
    pub fn recording_abort(&self) {}
}

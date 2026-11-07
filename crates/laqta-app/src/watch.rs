//! Watch folders (M6 D6): each enabled `WatchFolder` gets a `FileWatcher` watch; matching files are
//! (optionally moved into the screenshots folder and) uploaded with the folder's workflow.

use std::sync::Arc;

use crate::dispatcher::App;

impl App {
    /// Replaces the running watches with the current settings' enabled folders. `main` calls it at
    /// start and after the settings change.
    pub fn start_watch_folders(self: &Arc<Self>) {
        if let Ok(mut w) = self.watches.lock() {
            w.clear();
        }
    }
}

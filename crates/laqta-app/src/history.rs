//! History edits for the windows. A window used to write the store itself; it now asks the application, which runs
//! the call on the blocking pool through the `HistoryEdit` port. Without one (history is turned off) the calls report
//! `Unsupported`.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_channel::oneshot;
use laqta_ports::{AppError, BoxFuture, HistoryEdit, PortError};

use crate::dispatcher::App;

impl App {
    /// Gives the application the history to edit. `main` calls it once, when history is on.
    pub fn attach_history(&self, history: Arc<dyn HistoryEdit>) {
        // The first one stays: history does not change while the process runs.
        let _ = self.history.set(history);
    }

    /// Replaces the tags of a history entry.
    pub fn history_set_tags(&self, id: i64, tags: Vec<String>) -> BoxFuture<'static, Result<(), AppError>> {
        self.history_edit(move |history| history.set_tags(id, &tags))
    }

    /// Removes a history entry. The file it names stays.
    pub fn history_delete(&self, id: i64) -> BoxFuture<'static, Result<(), AppError>> {
        self.history_edit(move |history| history.delete(id))
    }

    /// Runs one edit on the blocking pool. A panic in it resumes in the caller, as it would have from the call itself.
    fn history_edit(
        &self,
        edit: impl FnOnce(&dyn HistoryEdit) -> Result<(), PortError> + Send + 'static,
    ) -> BoxFuture<'static, Result<(), AppError>> {
        let Some(history) = self.history.get().cloned() else {
            return Box::pin(async { Err(AppError::Unsupported("history is turned off".into())) });
        };
        let spawner = Arc::clone(&self.ports.spawner);
        Box::pin(async move {
            let (tx, rx) = oneshot::channel();
            spawner
                .run_blocking(Box::new(move || {
                    let outcome = std::panic::catch_unwind(AssertUnwindSafe(move || edit(&*history)));
                    let _ = tx.send(outcome);
                }))
                .await;
            match rx.await {
                Ok(Ok(result)) => result.map_err(AppError::from),
                Ok(Err(payload)) => std::panic::resume_unwind(payload),
                Err(oneshot::Canceled) => Err(AppError::Cancelled),
            }
        })
    }
}

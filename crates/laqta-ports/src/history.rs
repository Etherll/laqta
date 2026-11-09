//! The edits the windows make to history entries. New entries come from the task-event subscriber in
//! `laqta-library` (ADR 018) and reads go through its query functions, so the application needs only these two
//! writes, which a window used to make straight on the store.

use crate::PortError;

/// Changes to an existing history entry. The calls block on the database, so the application runs them on
/// `Spawner::run_blocking`. An unknown id is not an error.
pub trait HistoryEdit: Send + Sync {
    /// Replaces the tags of an entry.
    fn set_tags(&self, id: i64, tags: &[String]) -> Result<(), PortError>;
    /// Removes the entry. The file it names stays where it is.
    fn delete(&self, id: i64) -> Result<(), PortError>;
}

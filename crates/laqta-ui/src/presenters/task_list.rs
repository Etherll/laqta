//! Task-list rows built from task events.

use std::path::PathBuf;

use laqta_ports::{TaskEvent, TaskId, TaskStatus};

/// Rows kept in the main window's task list.
pub const MAX_ROWS: usize = 100;

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub id: TaskId,
    pub status: TaskStatus,
    pub title: String,
    pub detail: String,
    pub path: Option<PathBuf>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskList {
    rows: Vec<TaskRow>,
}

pub fn status_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::InQueue => "In queue",
        TaskStatus::Preparing => "Preparing",
        TaskStatus::Working => "Working",
        TaskStatus::Stopping => "Stopping",
        TaskStatus::Stopped => "Stopped",
        TaskStatus::Failed => "Failed",
        TaskStatus::Completed => "Completed",
    }
}

impl TaskList {
    /// Newest first.
    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }

    pub fn is_running(&self) -> bool {
        self.rows.iter().any(|r| matches!(r.status, TaskStatus::InQueue | TaskStatus::Preparing | TaskStatus::Working | TaskStatus::Stopping))
    }

    fn row(&mut self, id: TaskId) -> &mut TaskRow {
        let pos = match self.rows.iter().position(|r| r.id == id) {
            Some(p) => p,
            None => {
                self.rows.insert(
                    0,
                    TaskRow { id, status: TaskStatus::InQueue, title: format!("Task {}", id.0), detail: String::new(), path: None, error: None },
                );
                self.rows.truncate(MAX_ROWS);
                0
            }
        };
        &mut self.rows[pos]
    }

    pub fn apply(&mut self, event: &TaskEvent) {
        let row = self.row(event.id());
        match event {
            TaskEvent::StatusChanged { status, .. } => {
                row.status = *status;
                if row.detail.is_empty() || matches!(status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped) {
                    if row.error.is_none() || *status != TaskStatus::Failed {
                        row.detail = status_label(*status).to_string();
                    }
                }
            }
            TaskEvent::Saved { path, .. } => {
                row.title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| row.title.clone());
                row.path = Some(path.clone());
                row.detail = "Saved".into();
            }
            TaskEvent::Copied { .. } => row.detail = "Copied to clipboard".into(),
            TaskEvent::CopyFailed { reason, .. } => row.detail = format!("Copy failed: {reason}"),
            TaskEvent::Failed { error, .. } => {
                row.error = Some(error.to_string());
                row.detail = format!("Failed: {error}");
            }
            TaskEvent::SaveSkipped { path, .. } => row.detail = format!("Skipped: {} exists", path.display()),
            TaskEvent::StepFailed { step, reason, .. } => row.detail = format!("{step} failed: {reason}"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_follow_events() {
        let mut list = TaskList::default();
        let id = TaskId(1);
        list.apply(&TaskEvent::StatusChanged { id, status: TaskStatus::InQueue });
        assert!(list.is_running());
        list.apply(&TaskEvent::Saved { id, path: "/s/a.png".into() });
        list.apply(&TaskEvent::StatusChanged { id, status: TaskStatus::Completed });
        let r = &list.rows()[0];
        assert_eq!(r.title, "a.png");
        assert_eq!(r.detail, "Completed");
        assert!(!list.is_running());
        list.apply(&TaskEvent::StatusChanged { id: TaskId(2), status: TaskStatus::InQueue });
        assert_eq!(list.rows()[0].id, TaskId(2));
    }

    #[test]
    fn failure_keeps_error() {
        let mut list = TaskList::default();
        let id = TaskId(3);
        list.apply(&TaskEvent::Failed { id, error: laqta_ports::AppError::Io("disk full".into()) });
        list.apply(&TaskEvent::StatusChanged { id, status: TaskStatus::Failed });
        assert_eq!(list.rows()[0].detail, "Failed: i/o: disk full");
    }

    #[test]
    fn capped() {
        let mut list = TaskList::default();
        for i in 0..(MAX_ROWS as u64 + 5) {
            list.apply(&TaskEvent::StatusChanged { id: TaskId(i), status: TaskStatus::InQueue });
        }
        assert_eq!(list.rows().len(), MAX_ROWS);
    }
}

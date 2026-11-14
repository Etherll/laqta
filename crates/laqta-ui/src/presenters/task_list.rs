//! Task-list rows built from task events.

use std::path::PathBuf;

use laqta_ports::{AppError, TaskEvent, TaskId, TaskStatus};

/// Rows kept in the main window's task list.
pub const MAX_ROWS: usize = 100;

/// The title of a row before its task names it (a file name, or "Recording").
const NEW_ROW_TITLE: &str = "Capture";

#[derive(Clone, Debug, PartialEq)]
pub struct TaskRow {
    pub id: TaskId,
    pub status: TaskStatus,
    pub title: String,
    pub detail: String,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub error: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct TaskList {
    rows: Vec<TaskRow>,
    /// Counts the events that can change the history library, so a view reloads it only then.
    library_generation: u64,
}

/// Whether `event` can change what the history library holds: a save (the history writer records
/// it), a delete, or a task settling. Progress, recording ticks and the rest only repaint.
pub fn changes_library(event: &TaskEvent) -> bool {
    match event {
        TaskEvent::Saved { .. } | TaskEvent::Deleted { .. } => true,
        TaskEvent::StatusChanged { status, .. } => matches!(status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped),
        _ => false,
    }
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

/// How a status reads at a glance: the view picks a colour per tone and always shows the label too.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatusTone {
    Neutral,
    Busy,
    Success,
    Danger,
}

pub fn status_tone(status: TaskStatus) -> StatusTone {
    match status {
        TaskStatus::InQueue | TaskStatus::Stopped => StatusTone::Neutral,
        TaskStatus::Preparing | TaskStatus::Working | TaskStatus::Stopping => StatusTone::Busy,
        TaskStatus::Completed => StatusTone::Success,
        TaskStatus::Failed => StatusTone::Danger,
    }
}

impl TaskList {
    /// Newest first.
    pub fn rows(&self) -> &[TaskRow] {
        &self.rows
    }

    /// Goes up by one for each applied event where `changes_library` holds.
    pub fn library_generation(&self) -> u64 {
        self.library_generation
    }

    pub fn is_running(&self) -> bool {
        self.rows
            .iter()
            .any(|r| matches!(r.status, TaskStatus::InQueue | TaskStatus::Preparing | TaskStatus::Working | TaskStatus::Stopping))
    }

    /// Removes a settled row. A running one stays, since its task still reports to it.
    pub fn dismiss(&mut self, id: TaskId) {
        if let Some(pos) = self
            .rows
            .iter()
            .position(|r| r.id == id && matches!(r.status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped))
        {
            self.rows.remove(pos);
        }
    }

    fn row(&mut self, id: TaskId) -> &mut TaskRow {
        let pos = match self.rows.iter().position(|r| r.id == id) {
            Some(p) => p,
            None => {
                self.rows.insert(
                    0,
                    TaskRow {
                        id,
                        status: TaskStatus::InQueue,
                        title: NEW_ROW_TITLE.into(),
                        detail: String::new(),
                        path: None,
                        url: None,
                        error: None,
                    },
                );
                self.rows.truncate(MAX_ROWS);
                0
            }
        };
        &mut self.rows[pos]
    }

    pub fn apply(&mut self, event: &TaskEvent) {
        if changes_library(event) {
            self.library_generation += 1;
        }
        let row = self.row(event.id());
        match event {
            TaskEvent::StatusChanged { status, .. } => {
                row.status = *status;
                let settled = matches!(status, TaskStatus::Completed | TaskStatus::Failed | TaskStatus::Stopped);
                if (row.detail.is_empty() || settled) && (row.error.is_none() || *status != TaskStatus::Failed) {
                    row.detail = status_label(*status).to_string();
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
                row.detail = error.to_string();
            }
            TaskEvent::UploadStarted { destination, .. } => row.detail = format!("Uploading to {destination}"),
            TaskEvent::Progress { sent, total, .. } => {
                row.detail = match total {
                    Some(t) if *t > 0 => format!("Uploading {}%", sent * 100 / t),
                    _ => format!("Uploading {}", crate::presenters::history::format_size(*sent)),
                }
            }
            TaskEvent::UploadRetry { attempt, reason, .. } => row.detail = format!("Retry {attempt}: {reason}"),
            TaskEvent::Uploaded { url, .. } => {
                row.detail = url.clone();
                row.url = Some(url.clone());
            }
            TaskEvent::Shortened { url, .. } => row.url = Some(url.clone()),
            TaskEvent::RecordingCountdown { seconds_left, .. } => {
                row.title = "Recording".into();
                row.status = TaskStatus::Preparing;
                row.detail = format!("Starting in {seconds_left}");
            }
            TaskEvent::RecordingStarted { .. } | TaskEvent::RecordingResumed { .. } => {
                row.title = "Recording".into();
                row.status = TaskStatus::Working;
                row.detail = "Recording".into();
            }
            TaskEvent::RecordingTick { elapsed_secs, .. } => {
                row.status = TaskStatus::Working;
                row.detail = format!("Recording {}:{:02}", elapsed_secs / 60, elapsed_secs % 60);
            }
            TaskEvent::RecordingPaused { .. } => row.detail = "Paused".into(),
            // The row keeps its "Recording" state until the file is done; the recording bar shows the wait.
            TaskEvent::RecordingFinishing { .. } => {}
            TaskEvent::RecordingStopped { path, .. } => {
                row.title = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                row.path = Some(path.clone());
                row.status = TaskStatus::Completed;
                row.detail = "Recorded".into();
            }
            TaskEvent::RecordingFailed { error, partial, .. } => {
                if row.title == NEW_ROW_TITLE {
                    row.title = "Recording".into();
                }
                if matches!(error, AppError::Cancelled) {
                    row.status = TaskStatus::Stopped;
                    row.detail = status_label(TaskStatus::Stopped).to_string();
                } else {
                    row.status = TaskStatus::Failed;
                    row.error = Some(error.to_string());
                    row.detail = error.to_string();
                    row.path = partial.clone().or(row.path.take());
                }
            }
            TaskEvent::SaveSkipped { path, .. } => row.detail = format!("Skipped: a file named “{}” already exists", path.display()),
            TaskEvent::StepFailed { step, reason, .. } => row.detail = format!("{step} failed: {reason}"),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tones_cover_every_status() {
        assert_eq!(status_tone(TaskStatus::Working), StatusTone::Busy);
        assert_eq!(status_tone(TaskStatus::Completed), StatusTone::Success);
        assert_eq!(status_tone(TaskStatus::Failed), StatusTone::Danger);
        assert_eq!(status_tone(TaskStatus::InQueue), StatusTone::Neutral);
    }

    #[test]
    fn only_saves_deletes_and_settling_change_the_library() {
        let mut list = TaskList::default();
        let id = TaskId(1);
        list.apply(&TaskEvent::StatusChanged { id, status: TaskStatus::Working });
        list.apply(&TaskEvent::Progress { id, sent: 10, total: Some(100) });
        assert_eq!(list.library_generation(), 0);
        list.apply(&TaskEvent::Saved { id, path: "/s/a.png".into() });
        list.apply(&TaskEvent::StatusChanged { id, status: TaskStatus::Completed });
        list.apply(&TaskEvent::Deleted { id, path: "/s/a.png".into() });
        assert_eq!(list.library_generation(), 3);
    }

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
        assert_eq!(list.rows()[0].detail, "i/o: disk full");
    }

    #[test]
    fn new_rows_are_titled_capture_until_named() {
        let mut list = TaskList::default();
        list.apply(&TaskEvent::StatusChanged { id: TaskId(4), status: TaskStatus::InQueue });
        assert_eq!(list.rows()[0].title, "Capture");
        list.apply(&TaskEvent::RecordingFailed { id: TaskId(4), error: AppError::Io("no mic".into()), partial: None });
        assert_eq!(list.rows()[0].title, "Recording");
        assert_eq!(list.rows()[0].detail, "i/o: no mic");
    }

    #[test]
    fn a_skipped_save_names_the_file() {
        let mut list = TaskList::default();
        list.apply(&TaskEvent::SaveSkipped { id: TaskId(5), path: "/s/a.png".into() });
        assert_eq!(list.rows()[0].detail, "Skipped: a file named “/s/a.png” already exists");
    }

    #[test]
    fn dismiss_removes_a_settled_row_only() {
        let mut list = TaskList::default();
        list.apply(&TaskEvent::Failed { id: TaskId(1), error: AppError::Io("disk full".into()) });
        list.apply(&TaskEvent::StatusChanged { id: TaskId(1), status: TaskStatus::Failed });
        list.apply(&TaskEvent::StatusChanged { id: TaskId(2), status: TaskStatus::Working });
        list.dismiss(TaskId(2));
        assert_eq!(list.rows().len(), 2, "a running row stays");
        list.dismiss(TaskId(1));
        assert_eq!(list.rows().iter().map(|r| r.id).collect::<Vec<_>>(), vec![TaskId(2)]);
        list.dismiss(TaskId(99));
        assert_eq!(list.rows().len(), 1);
    }

    #[test]
    fn upload_progress() {
        let mut list = TaskList::default();
        let id = TaskId(9);
        list.apply(&TaskEvent::Progress { id, sent: 50, total: Some(200) });
        assert_eq!(list.rows()[0].detail, "Uploading 25%");
        list.apply(&TaskEvent::Uploaded { id, url: "https://x/y".into(), thumbnail_url: None, deletion_url: None, path: None });
        assert_eq!(list.rows()[0].url.as_deref(), Some("https://x/y"));
    }

    #[test]
    fn capped() {
        let mut list = TaskList::default();
        for i in 0..(MAX_ROWS as u64 + 5) {
            list.apply(&TaskEvent::StatusChanged { id: TaskId(i), status: TaskStatus::InQueue });
        }
        assert_eq!(list.rows().len(), MAX_ROWS);
    }

    #[test]
    fn a_recording_row_follows_its_events_and_settles() {
        let mut list = TaskList::default();
        let id = TaskId(7);
        list.apply(&TaskEvent::RecordingCountdown {
            id,
            seconds_left: 2,
            area: laqta_domain::geometry::PhysRect { x: 0, y: 0, width: 10, height: 10 },
        });
        assert_eq!(list.rows()[0].status, TaskStatus::Preparing);
        list.apply(&TaskEvent::RecordingStarted { id, area: laqta_domain::geometry::PhysRect::default(), show_border: true });
        list.apply(&TaskEvent::RecordingTick { id, elapsed_secs: 65 });
        assert_eq!((list.rows()[0].status, list.rows()[0].detail.as_str()), (TaskStatus::Working, "Recording 1:05"));
        assert!(list.is_running());
        list.apply(&TaskEvent::RecordingFinishing { id });
        assert_eq!(
            (list.rows()[0].status, list.rows()[0].detail.as_str()),
            (TaskStatus::Working, "Recording 1:05"),
            "finishing leaves the row as it is"
        );
        list.apply(&TaskEvent::RecordingStopped { id, path: "/v/a.gif".into() });
        assert_eq!((list.rows()[0].status, list.rows()[0].title.as_str()), (TaskStatus::Completed, "a.gif"));
        assert!(!list.is_running());
    }

    #[test]
    fn a_cancelled_recording_is_stopped_and_a_failed_one_shows_its_error() {
        let mut list = TaskList::default();
        list.apply(&TaskEvent::RecordingFailed { id: TaskId(1), error: AppError::Cancelled, partial: None });
        assert_eq!((list.rows()[0].status, list.rows()[0].title.as_str()), (TaskStatus::Stopped, "Recording"));
        let error = AppError::Unsupported("no active window".into());
        list.apply(&TaskEvent::RecordingFailed { id: TaskId(2), error, partial: Some("/v/p.mkv".into()) });
        let row = &list.rows()[0];
        assert_eq!(row.status, TaskStatus::Failed);
        assert!(row.detail.contains("no active window"), "{}", row.detail);
        assert_eq!(row.path.as_deref(), Some(std::path::Path::new("/v/p.mkv")));
        assert!(!list.is_running());
    }
}

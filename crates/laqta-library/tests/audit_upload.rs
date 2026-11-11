//! Uploads reach history with their URL, thumbnail URL, deletion URL and host, not only `Saved`
//! events. (The short URL has no history column yet.)
#![allow(clippy::unwrap_used)]

use std::sync::Arc;

use futures_lite::future::block_on;
use jiff::civil::date;
use laqta_domain::history::HistoryQuery;
use laqta_library::{History, record_task_events};
use laqta_ports::fakes::ManualClock;
use laqta_ports::{Clock, TaskEvent, TaskId};

#[test]
fn audit_bug_uploads_never_reach_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shot.png");
    std::fs::write(&path, b"x").unwrap();
    let history = Arc::new(History::open_in_memory().unwrap());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(date(2026, 11, 7).at(14, 3, 22, 0)));
    let (sender, receiver) = async_broadcast::broadcast::<TaskEvent>(16);
    block_on(async {
        let id = TaskId(1);
        sender.broadcast(TaskEvent::Saved { id, path: path.clone() }).await.unwrap();
        sender
            .broadcast(TaskEvent::Uploaded {
                id,
                url: "https://i.example/abc.png".into(),
                thumbnail_url: Some("https://i.example/abcm.png".into()),
                deletion_url: Some("https://i.example/delete/xyz".into()),
            })
            .await
            .unwrap();
        // A text upload has no Saved event at all.
        sender
            .broadcast(TaskEvent::Uploaded {
                id: TaskId(2),
                url: "https://paste.example/q".into(),
                thumbnail_url: None,
                deletion_url: None,
            })
            .await
            .unwrap();
        drop(sender);
    });
    block_on(record_task_events(Arc::clone(&history), receiver, clock, || 0));
    let items = history.query(&HistoryQuery::default()).unwrap();
    assert!(
        items.iter().any(|item| item.url.as_deref() == Some("https://i.example/abc.png")),
        "the uploaded URL is not in history: {items:?}"
    );
    assert!(items.iter().any(|item| item.url.as_deref() == Some("https://paste.example/q")), "the text upload has no history entry");
}

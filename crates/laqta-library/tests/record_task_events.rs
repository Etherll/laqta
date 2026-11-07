#![allow(clippy::unwrap_used)]

use std::fs::{self, File};
use std::io::BufWriter;
use std::path::Path;
use std::sync::Arc;

use futures_lite::future::block_on;
use jiff::civil::date;
use laqta_domain::history::{HistoryKind, HistoryQuery};
use laqta_library::{History, record_task_events};
use laqta_ports::fakes::ManualClock;
use laqta_ports::{Clock, TaskEvent, TaskId, TaskStatus};

/// A solid-colour RGBA PNG of the given size.
fn write_png(path: &Path, width: u32, height: u32) {
    let writer = BufWriter::new(File::create(path).unwrap());
    let mut encoder = png::Encoder::new(writer, width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&vec![0x80_u8; (width * height * 4) as usize]).unwrap();
    writer.finish().unwrap();
}

fn assert_send<T: Send>(_: &T) {}

#[test]
fn saved_files_become_history_entries_until_the_channel_closes() {
    let dir = tempfile::tempdir().unwrap();
    let png_path = dir.path().join("Laqta_2026-11-07_14-03-22.png");
    write_png(&png_path, 64, 32);
    let notes_path = dir.path().join("notes.md");
    fs::write(&notes_path, "hello").unwrap();
    let missing_path = dir.path().join("deleted.png");

    let history = Arc::new(History::open_in_memory().unwrap());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(date(2026, 11, 7).at(14, 3, 22, 0)));
    let (sender, receiver) = async_broadcast::broadcast::<TaskEvent>(16);

    block_on(async {
        let id = TaskId(1);
        sender.broadcast(TaskEvent::StatusChanged { id, status: TaskStatus::Working }).await.unwrap();
        sender.broadcast(TaskEvent::Saved { id, path: png_path.clone() }).await.unwrap();
        sender.broadcast(TaskEvent::Saved { id: TaskId(2), path: notes_path.clone() }).await.unwrap();
        // The file is gone: the entry is still recorded, just without a size.
        sender.broadcast(TaskEvent::Saved { id: TaskId(3), path: missing_path.clone() }).await.unwrap();
        drop(sender);
    });

    let future = record_task_events(Arc::clone(&history), receiver, clock);
    assert_send(&future);
    block_on(future);

    // Newest first. All three share the clock's time, so insert order decides.
    let items = history.query(&HistoryQuery::default()).unwrap();
    assert_eq!(items.len(), 3);

    let missing = &items[0];
    assert_eq!(missing.file_name, "deleted.png");
    assert_eq!(missing.path.as_deref(), Some(missing_path.as_path()));
    assert_eq!(missing.size_bytes, None);

    let notes = &items[1];
    assert_eq!(notes.kind, HistoryKind::Text);
    assert_eq!(notes.file_name, "notes.md");
    assert_eq!(notes.path.as_deref(), Some(notes_path.as_path()));
    assert_eq!(notes.size_bytes, Some(5));
    assert_eq!((notes.width, notes.height), (None, None));
    assert_eq!(notes.time, date(2026, 11, 7).at(14, 3, 22, 0));

    let shot = &items[2];
    assert_eq!(shot.kind, HistoryKind::Image);
    assert_eq!(shot.file_name, "Laqta_2026-11-07_14-03-22.png");
    assert_eq!(shot.path.as_deref(), Some(png_path.as_path()));
    assert_eq!((shot.width, shot.height), (Some(64), Some(32)));
    assert_eq!(shot.size_bytes, Some(fs::metadata(&png_path).unwrap().len()));
    assert!(shot.tags.is_empty() && shot.url.is_none());
}

#[test]
fn a_non_png_image_name_gets_no_dimensions() {
    let dir = tempfile::tempdir().unwrap();
    let jpg_path = dir.path().join("photo.jpg");
    fs::write(&jpg_path, b"not really a jpeg").unwrap();

    let history = Arc::new(History::open_in_memory().unwrap());
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(date(2026, 11, 7).at(9, 0, 0, 0)));
    let (sender, receiver) = async_broadcast::broadcast::<TaskEvent>(4);
    block_on(async {
        sender.broadcast(TaskEvent::Saved { id: TaskId(1), path: jpg_path.clone() }).await.unwrap();
        drop(sender);
    });
    block_on(record_task_events(Arc::clone(&history), receiver, clock));

    let items = history.query(&HistoryQuery::default()).unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0].kind, HistoryKind::Image);
    assert_eq!((items[0].width, items[0].height), (None, None));
}

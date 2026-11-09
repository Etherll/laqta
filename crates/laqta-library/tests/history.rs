#![allow(clippy::unwrap_used)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_lite::future::block_on;
use jiff::civil::{DateTime, date};
use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery, HistoryStats, NewHistoryItem};
use laqta_library::{History, LibraryError};
use laqta_ports::fakes::{InlineSpawner, ManualClock};
use laqta_ports::{BoxFuture, Clock};

fn new_item(time: DateTime, kind: HistoryKind, file_name: &str) -> NewHistoryItem {
    NewHistoryItem {
        time,
        kind,
        file_name: file_name.to_owned(),
        path: None,
        url: None,
        thumbnail_url: None,
        deletion_url: None,
        host: None,
        width: None,
        height: None,
        size_bytes: None,
        thumbnail_path: None,
        tags: Vec::new(),
    }
}

/// Five entries. Ids follow insertion order. Entries 4 and 5 share a second.
///
/// | id | time                | kind  | file / url                     | tags         | bytes |
/// |----|---------------------|-------|--------------------------------|--------------|-------|
/// | 1  | 2026-11-01 09:00:00 | Image | Laqta_2026-11-01_09-00-00.png  | work         | 1000  |
/// | 2  | 2026-11-03 10:00:00 | Image | holiday-beach.png (imgur)      | fun          | 2000  |
/// | 3  | 2026-11-05 11:00:00 | Text  | notes-2026.txt                 | work         | 300   |
/// | 4  | 2026-11-07 14:03:22 | Url   | https://short.example/q        | none         | none  |
/// | 5  | 2026-11-07 14:03:22 | Image | Laqta_same-second.png          | none         | 500   |
fn seeded() -> (History, Vec<i64>) {
    let db = History::open_in_memory().unwrap();
    let mut ids = Vec::new();

    let mut a = new_item(date(2026, 11, 1).at(9, 0, 0, 0), HistoryKind::Image, "Laqta_2026-11-01_09-00-00.png");
    a.size_bytes = Some(1000);
    a.tags = vec!["work".into()];
    ids.push(db.add(&a).unwrap());

    let mut b = new_item(date(2026, 11, 3).at(10, 0, 0, 0), HistoryKind::Image, "holiday-beach.png");
    b.url = Some("https://imgur.example/abc".into());
    b.host = Some("imgur.example".into());
    b.size_bytes = Some(2000);
    b.tags = vec!["fun".into()];
    ids.push(db.add(&b).unwrap());

    let mut c = new_item(date(2026, 11, 5).at(11, 0, 0, 0), HistoryKind::Text, "notes-2026.txt");
    c.size_bytes = Some(300);
    c.tags = vec!["work".into()];
    ids.push(db.add(&c).unwrap());

    let mut d = new_item(date(2026, 11, 7).at(14, 3, 22, 0), HistoryKind::Url, "https://short.example/q");
    d.url = Some("https://short.example/q".into());
    d.host = Some("short.example".into());
    ids.push(db.add(&d).unwrap());

    let mut e = new_item(date(2026, 11, 7).at(14, 3, 22, 0), HistoryKind::Image, "Laqta_same-second.png");
    e.size_bytes = Some(500);
    ids.push(db.add(&e).unwrap());

    (db, ids)
}

fn names(items: &[HistoryItem]) -> Vec<&str> {
    items.iter().map(|item| item.file_name.as_str()).collect()
}

#[test]
fn add_then_get_returns_the_same_entry() {
    let db = History::open_in_memory().unwrap();
    let item = NewHistoryItem {
        time: date(2026, 11, 7).at(14, 3, 22, 0),
        kind: HistoryKind::Image,
        file_name: "Laqta_2026-11-07_14-03-22.png".into(),
        path: Some(PathBuf::from("/shots/Laqta_2026-11-07_14-03-22.png")),
        url: Some("https://host.example/i/x.png".into()),
        thumbnail_url: Some("https://host.example/t/x.png".into()),
        deletion_url: Some("https://host.example/d/x".into()),
        host: Some("host.example".into()),
        width: Some(1920),
        height: Some(1080),
        size_bytes: Some(4096),
        thumbnail_path: Some(PathBuf::from("/data/thumbs/1.png")),
        tags: vec!["work".into(), "shot".into()],
    };
    let id = db.add(&item).unwrap();

    let expected = HistoryItem {
        id,
        time: item.time,
        kind: item.kind,
        file_name: item.file_name.clone(),
        path: item.path.clone(),
        url: item.url.clone(),
        thumbnail_url: item.thumbnail_url.clone(),
        deletion_url: item.deletion_url.clone(),
        host: item.host.clone(),
        width: item.width,
        height: item.height,
        size_bytes: item.size_bytes,
        thumbnail_path: item.thumbnail_path.clone(),
        tags: item.tags.clone(),
    };
    assert_eq!(db.get(id).unwrap(), Some(expected));
    assert_eq!(db.get(id + 100).unwrap(), None);
}

#[test]
fn times_are_stored_to_the_second() {
    let db = History::open_in_memory().unwrap();
    let id = db.add(&new_item(date(2026, 11, 7).at(14, 3, 22, 750_000_000), HistoryKind::File, "a.bin")).unwrap();
    assert_eq!(db.get(id).unwrap().unwrap().time, date(2026, 11, 7).at(14, 3, 22, 0));
}

#[test]
fn text_matches_file_name_prefix_url_and_host() {
    let (db, ids) = seeded();
    let text = |text: &str| HistoryQuery { text: Some(text.into()), ..Default::default() };

    assert_eq!(names(&db.query(&text("holiday")).unwrap()), ["holiday-beach.png"]);
    assert_eq!(names(&db.query(&text("hol")).unwrap()), ["holiday-beach.png"]);
    assert_eq!(names(&db.query(&text("imgur")).unwrap()), ["holiday-beach.png"]);
    assert_eq!(names(&db.query(&text("short.example")).unwrap()), ["https://short.example/q"]);
    // Every word must match: this one is in the file name only.
    assert_eq!(names(&db.query(&text("laqta same")).unwrap()), ["Laqta_same-second.png"]);
    assert!(db.query(&text("laqta holiday")).unwrap().is_empty());
    assert_eq!(db.query(&text("laqta")).unwrap().iter().map(|i| i.id).collect::<Vec<_>>(), [ids[4], ids[0]]);
}

#[test]
fn text_with_fts_syntax_characters_does_not_error() {
    let (db, _) = seeded();
    let text = |text: &str| HistoryQuery { text: Some(text.into()), ..Default::default() };

    assert_eq!(names(&db.query(&text("laqta-2026")).unwrap()), ["Laqta_2026-11-01_09-00-00.png"]);
    assert!(db.query(&text("\"")).unwrap().is_empty());
    assert!(db.query(&text("NEAR OR (")).unwrap().is_empty());
    // Blank text is no filter at all.
    assert_eq!(db.query(&text("   ")).unwrap().len(), 5);
}

#[test]
fn like_wildcards_in_text_are_literal() {
    let db = History::open_in_memory().unwrap();
    let mut encoded = new_item(date(2026, 11, 7).at(9, 0, 0, 0), HistoryKind::Url, "a-b.png");
    encoded.url = Some("https://x.example/a%20b".into());
    db.add(&encoded).unwrap();
    let mut other = new_item(date(2026, 11, 7).at(9, 1, 0, 0), HistoryKind::Url, "other.png");
    other.url = Some("https://x.example/a20b".into());
    db.add(&other).unwrap();

    let query = HistoryQuery { text: Some("%20".into()), ..Default::default() };
    assert_eq!(names(&db.query(&query).unwrap()), ["a-b.png"]);
}

#[test]
fn filters_by_kind_tag_and_date_range() {
    let (db, ids) = seeded();

    let images = HistoryQuery { kind: Some(HistoryKind::Image), ..Default::default() };
    let image_ids: Vec<i64> = db.query(&images).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(image_ids, [ids[4], ids[1], ids[0]]);

    let work = HistoryQuery { tag: Some("work".into()), ..Default::default() };
    let work_ids: Vec<i64> = db.query(&work).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(work_ids, [ids[2], ids[0]]);

    // Both bounds are inclusive.
    let range =
        HistoryQuery { from: Some(date(2026, 11, 3).at(10, 0, 0, 0)), to: Some(date(2026, 11, 5).at(11, 0, 0, 0)), ..Default::default() };
    assert_eq!(names(&db.query(&range).unwrap()), ["notes-2026.txt", "holiday-beach.png"]);

    let combined = HistoryQuery { kind: Some(HistoryKind::Image), tag: Some("work".into()), ..Default::default() };
    assert_eq!(names(&db.query(&combined).unwrap()), ["Laqta_2026-11-01_09-00-00.png"]);
}

#[test]
fn query_is_newest_first_and_pages() {
    let (db, ids) = seeded();

    let all = db.query(&HistoryQuery::default()).unwrap();
    let all_ids: Vec<i64> = all.iter().map(|i| i.id).collect();
    // Entries 4 and 5 share a time, so the later insert comes first.
    assert_eq!(all_ids, [ids[4], ids[3], ids[2], ids[1], ids[0]]);

    let page = HistoryQuery { offset: 1, limit: 2, ..Default::default() };
    let page_ids: Vec<i64> = db.query(&page).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(page_ids, [ids[3], ids[2]]);

    let past_end = HistoryQuery { offset: 10, limit: 2, ..Default::default() };
    assert!(db.query(&past_end).unwrap().is_empty());
}

#[test]
fn stats_count_entries_kinds_and_bytes() {
    let (db, _) = seeded();
    let stats = db.stats().unwrap();
    assert_eq!(
        stats,
        HistoryStats {
            total: 5,
            by_kind: vec![(HistoryKind::Image, 3), (HistoryKind::Text, 1), (HistoryKind::File, 0), (HistoryKind::Url, 1),],
            total_bytes: 3800,
        }
    );
}

#[test]
fn stats_on_empty_history() {
    let db = History::open_in_memory().unwrap();
    let stats = db.stats().unwrap();
    assert_eq!(stats.total, 0);
    assert_eq!(stats.total_bytes, 0);
    assert_eq!(stats.by_kind.len(), 4);
    assert!(stats.by_kind.iter().all(|(_, count)| *count == 0));
}

#[test]
fn trim_keeps_the_newest_and_reports_the_count() {
    let (db, ids) = seeded();
    assert_eq!(db.trim(0).unwrap(), 0);
    assert_eq!(db.trim(10).unwrap(), 0);
    assert_eq!(db.trim(3).unwrap(), 2);

    let kept: Vec<i64> = db.query(&HistoryQuery::default()).unwrap().iter().map(|i| i.id).collect();
    assert_eq!(kept, [ids[4], ids[3], ids[2]]);
    assert!(db.get(ids[0]).unwrap().is_none());
    // The tag query no longer sees the trimmed entries.
    let work = HistoryQuery { tag: Some("work".into()), ..Default::default() };
    assert_eq!(names(&db.query(&work).unwrap()), ["notes-2026.txt"]);
}

#[test]
fn delete_removes_the_entry_and_its_tags() {
    let (db, ids) = seeded();
    db.delete(ids[2]).unwrap();
    assert!(db.get(ids[2]).unwrap().is_none());
    let work = HistoryQuery { tag: Some("work".into()), ..Default::default() };
    assert_eq!(names(&db.query(&work).unwrap()), ["Laqta_2026-11-01_09-00-00.png"]);

    db.delete(ids[2] + 100).unwrap();
    assert_eq!(db.stats().unwrap().total, 4);
}

#[test]
fn set_tags_replaces_them_in_order() {
    let (db, ids) = seeded();
    db.set_tags(ids[1], &["b".into(), "a".into(), "b".into()]).unwrap();
    assert_eq!(db.get(ids[1]).unwrap().unwrap().tags, ["b", "a", "b"]);

    db.set_tags(ids[1], &[]).unwrap();
    assert!(db.get(ids[1]).unwrap().unwrap().tags.is_empty());

    // An unknown id is not an error and adds nothing.
    db.set_tags(ids[4] + 100, &["ghost".into()]).unwrap();
    let ghost = HistoryQuery { tag: Some("ghost".into()), ..Default::default() };
    assert!(db.query(&ghost).unwrap().is_empty());
}

#[test]
fn set_thumbnail_stores_the_path() {
    let (db, ids) = seeded();
    db.set_thumbnail(ids[3], Path::new("/data/thumbs/4.png")).unwrap();
    assert_eq!(db.get(ids[3]).unwrap().unwrap().thumbnail_path, Some(PathBuf::from("/data/thumbs/4.png")));
    db.set_thumbnail(ids[4] + 100, Path::new("/data/thumbs/x.png")).unwrap();
}

#[test]
fn data_survives_reopening_the_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested/history.db");
    let id = {
        let db = History::open(&path).unwrap();
        db.add(&new_item(date(2026, 11, 7).at(14, 3, 22, 0), HistoryKind::Text, "kept.txt")).unwrap()
    };
    let db = History::open(&path).unwrap();
    assert_eq!(db.get(id).unwrap().unwrap().file_name, "kept.txt");
}

#[test]
fn newer_schema_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.db");
    {
        let conn = rusqlite::Connection::open(&path).unwrap();
        conn.pragma_update(None, "user_version", 99).unwrap();
    }
    let error = History::open(&path).expect_err("newer schema must be refused");
    assert!(matches!(error, LibraryError::Io(_)), "{error}");
}

#[test]
fn history_is_send_and_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<History>();
}

#[test]
fn domain_types_serialize_times_as_iso_text() {
    let item = HistoryItem {
        id: 7,
        time: date(2026, 11, 7).at(14, 3, 22, 0),
        kind: HistoryKind::Url,
        file_name: "x".into(),
        path: None,
        url: Some("https://a.example/x".into()),
        thumbnail_url: None,
        deletion_url: None,
        host: None,
        width: None,
        height: None,
        size_bytes: None,
        thumbnail_path: None,
        tags: vec!["t".into()],
    };
    let json = serde_json::to_value(&item).unwrap();
    assert_eq!(json["time"], "2026-11-07T14:03:22");
    assert_eq!(json["kind"], "url");
    let back: HistoryItem = serde_json::from_value(json).unwrap();
    assert_eq!(back, item);

    let query = HistoryQuery { from: Some(date(2026, 11, 1).at(0, 0, 0, 0)), to: None, limit: 20, ..Default::default() };
    let json = serde_json::to_value(&query).unwrap();
    assert_eq!(json["from"], "2026-11-01T00:00:00");
    assert!(json["to"].is_null());
    let back: HistoryQuery = serde_json::from_value(json).unwrap();
    assert_eq!(back, query);

    // A query written without the optional bounds still reads.
    let bare: HistoryQuery = serde_json::from_value(serde_json::json!({ "offset": 0, "limit": 0 })).unwrap();
    assert_eq!(bare, HistoryQuery::default());
}

/// Runs every future the spawner holds: the release timers.
fn run_timers(spawner: &InlineSpawner) -> usize {
    let timers = spawner.take_spawned();
    let count = timers.len();
    for timer in timers {
        block_on(timer);
    }
    count
}

#[test]
fn an_idle_connection_is_released_and_the_next_call_reopens_it() {
    let dir = tempfile::tempdir().unwrap();
    let db = History::open(&dir.path().join("history.db")).unwrap();
    let start = date(2026, 11, 7).at(14, 3, 22, 0);
    let clock = Arc::new(ManualClock::new(start));
    let spawner = Arc::new(InlineSpawner::default());
    let id = db.add(&new_item(start, HistoryKind::Text, "kept.txt")).unwrap();

    db.release_when_idle(clock.clone(), spawner.clone());
    assert!(db.connected(), "the connection stays open until the timer says it was idle");
    assert_eq!(spawner.spawned.lock().unwrap().len(), 1, "one timer exists while the connection is held");
    db.stats().unwrap();
    assert_eq!(spawner.spawned.lock().unwrap().len(), 1, "a use does not arm a second timer");

    // The timer sleeps on the Clock port and releases once a full minute passed without a use.
    let idle_minute = db.get(id).unwrap().unwrap();
    assert_eq!(idle_minute.file_name, "kept.txt");
    assert_eq!(run_timers(&spawner), 1);
    assert!(!db.connected(), "the idle connection was released");
    assert_eq!(clock.now(), start.checked_add(Duration::from_secs(120)).unwrap(), "the use during the first minute cost a second sleep");

    // The next call opens the file again, with the data in it, and arms a new timer.
    assert_eq!(db.get(id).unwrap().unwrap().file_name, "kept.txt");
    assert!(db.connected());
    assert_eq!(spawner.spawned.lock().unwrap().len(), 1);
    assert_eq!(run_timers(&spawner), 1);
    assert!(!db.connected());
    assert_eq!(run_timers(&spawner), 0, "with nothing held there is no timer, so no wakeups");
}

/// A clock that runs a hook during its first sleep, so a test can use the history while the release timer waits.
struct HookClock {
    inner: ManualClock,
    sleeps: Mutex<u32>,
    first_sleep: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl Clock for HookClock {
    fn now(&self) -> DateTime {
        self.inner.now()
    }
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        *self.sleeps.lock().unwrap() += 1;
        if let Some(hook) = self.first_sleep.lock().unwrap().take() {
            hook();
        }
        self.inner.sleep(duration)
    }
}

#[test]
fn a_use_while_the_timer_sleeps_keeps_the_connection_for_another_minute() {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(History::open(&dir.path().join("history.db")).unwrap());
    let used = Arc::clone(&db);
    let clock = Arc::new(HookClock {
        inner: ManualClock::new(date(2026, 11, 7).at(14, 3, 22, 0)),
        sleeps: Mutex::new(0),
        first_sleep: Mutex::new(Some(Box::new(move || {
            used.stats().unwrap();
        }))),
    });
    let spawner = Arc::new(InlineSpawner::default());

    db.release_when_idle(clock.clone(), spawner.clone());
    assert_eq!(run_timers(&spawner), 1);

    assert_eq!(*clock.sleeps.lock().unwrap(), 2, "the use woke the first sleep into a second one");
    assert!(!db.connected(), "and the connection went once the second minute passed idle");
}

#[test]
fn an_in_memory_history_is_never_released() {
    let db = History::open_in_memory().unwrap();
    let spawner = Arc::new(InlineSpawner::default());
    db.release_when_idle(Arc::new(ManualClock::new(date(2026, 11, 7).at(14, 3, 22, 0))), spawner.clone());
    assert_eq!(run_timers(&spawner), 0, "dropping the connection would drop the data");
    assert!(db.connected());
}

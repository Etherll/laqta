#![allow(clippy::unwrap_used)]

use std::collections::HashMap;
use std::path::PathBuf;

use jiff::civil::date;
use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery};
use laqta_library::{History, ImportReport, import_sharex_history, import_sharex_history_file};

/// Three entries in ShareX's array form. The first has an offset, a shortened URL to ignore, and a
/// tags object. The second has a tags array with blank strings. The third has `null` tags.
const ARRAY: &str = r#"[
  {
    "FileName": "Laqta_2023-05-01_12-34-56.png",
    "FilePath": "C:\\Shots\\Laqta_2023-05-01_12-34-56.png",
    "DateTime": "2023-05-01T12:34:56.789+02:00",
    "Type": "Image",
    "Host": "imgur.example",
    "URL": "https://imgur.example/abc",
    "ShortenedURL": "https://short.example/q",
    "ThumbnailURL": "https://imgur.example/abc_t.jpg",
    "DeletionURL": "https://imgur.example/delete/abc",
    "Tags": { "source": "hotkey", "app": "" }
  },
  {
    "FileName": "notes.txt",
    "FilePath": "C:\\Notes\\notes.txt",
    "DateTime": "2023-05-02T08:00:00.000+00:00",
    "Type": "Text",
    "Tags": [ "work", " ", "" ]
  },
  {
    "FileName": "",
    "DateTime": "2023-05-03T09:30:00.000-05:00",
    "Type": "URL",
    "URL": "https://example.com/page",
    "Host": "example.com",
    "Tags": null
  }
]"#;

/// The same kind of objects written as a stream: no brackets, commas between objects, and a trailing
/// comma after the last one.
const STREAM: &str = r#"{
  "FileName": "a.png",
  "DateTime": "2023-05-01T12:34:56.789+02:00",
  "Type": "Image"
},
{
  "FileName": "b.md",
  "DateTime": "2023-05-02T08:00:00.000+02:00",
  "Type": "Text"
},
"#;

fn all(db: &History) -> Vec<HistoryItem> {
    db.query(&HistoryQuery::default()).unwrap()
}

/// Entries keyed by file name. Names in these tests are unique.
fn by_name(items: &[HistoryItem]) -> HashMap<&str, &HistoryItem> {
    items.iter().map(|item| (item.file_name.as_str(), item)).collect()
}

fn sorted(mut tags: Vec<String>) -> Vec<String> {
    tags.sort();
    tags
}

#[test]
fn array_form_imports_every_entry_with_its_fields() {
    let db = History::open_in_memory().unwrap();
    let report = import_sharex_history(&db, ARRAY).unwrap();
    assert_eq!(report, ImportReport { imported: 3, skipped: 0, failed: 0 });

    let items = all(&db);
    let names = by_name(&items);

    let shot = names["Laqta_2023-05-01_12-34-56.png"];
    assert_eq!(shot.time, date(2023, 5, 1).at(12, 34, 56, 0), "the offset is dropped, the wall clock kept");
    assert_eq!(shot.kind, HistoryKind::Image);
    assert_eq!(shot.path, Some(PathBuf::from("C:\\Shots\\Laqta_2023-05-01_12-34-56.png")));
    assert_eq!(shot.url.as_deref(), Some("https://imgur.example/abc"), "URL wins over ShortenedURL");
    assert_eq!(shot.host.as_deref(), Some("imgur.example"));
    assert_eq!(shot.thumbnail_url.as_deref(), Some("https://imgur.example/abc_t.jpg"));
    assert_eq!(shot.deletion_url.as_deref(), Some("https://imgur.example/delete/abc"));
    assert_eq!(sorted(shot.tags.clone()), ["app", "source=hotkey"]);
    assert_eq!((shot.width, shot.height, shot.size_bytes), (None, None, None));

    let notes = names["notes.txt"];
    assert_eq!(notes.kind, HistoryKind::Text);
    assert_eq!(notes.url, None);
    assert_eq!(notes.tags, ["work"], "blank tags are dropped");

    let link = items.iter().find(|item| item.kind == HistoryKind::Url).unwrap();
    assert_eq!(link.file_name, "");
    assert_eq!(link.time, date(2023, 5, 3).at(9, 30, 0, 0));
    assert_eq!(link.url.as_deref(), Some("https://example.com/page"));
    assert!(link.tags.is_empty() && link.path.is_none());
}

#[test]
fn stream_form_imports_like_the_array_form() {
    let db = History::open_in_memory().unwrap();
    let report = import_sharex_history(&db, STREAM).unwrap();
    assert_eq!(report, ImportReport { imported: 2, skipped: 0, failed: 0 });
    assert_eq!(all(&db).len(), 2);
}

#[test]
fn a_byte_order_mark_before_either_form_is_ignored() {
    let db = History::open_in_memory().unwrap();
    let report = import_sharex_history(&db, &format!("\u{feff}{STREAM}")).unwrap();
    assert_eq!(report.imported, 2);
    let report = import_sharex_history(&db, &format!("\u{feff}{ARRAY}")).unwrap();
    assert_eq!(report.failed, 0);
}

#[test]
fn an_empty_file_imports_nothing() {
    let db = History::open_in_memory().unwrap();
    assert_eq!(import_sharex_history(&db, "").unwrap(), ImportReport::default());
    assert_eq!(import_sharex_history(&db, " \n").unwrap(), ImportReport::default());
    assert!(all(&db).is_empty());
}

#[test]
fn tags_as_a_map_or_an_array_become_plain_strings() {
    let db = History::open_in_memory().unwrap();
    let json = r#"[
      {"FileName":"map.png","DateTime":"2023-05-01T00:00:00+00:00","Tags":{"a":"1","b":"","c":null,"d":2}},
      {"FileName":"list.png","DateTime":"2023-05-01T00:00:01+00:00","Tags":["x","  y ",3]}
    ]"#;
    import_sharex_history(&db, json).unwrap();
    let items = all(&db);
    let names = by_name(&items);
    assert_eq!(sorted(names["map.png"].tags.clone()), ["a=1", "b", "c", "d=2"]);
    assert_eq!(names["list.png"].tags, ["x", "y"], "non-string array items are dropped");
}

#[test]
fn type_names_map_to_kinds_and_a_missing_or_unknown_type_follows_the_extension() {
    let db = History::open_in_memory().unwrap();
    let json = r#"[
      {"FileName":"a.PNG","DateTime":"2023-05-01T00:00:00+00:00","Type":"image"},
      {"FileName":"b.bin","DateTime":"2023-05-01T00:00:01+00:00","Type":"File"},
      {"FileName":"c","DateTime":"2023-05-01T00:00:02+00:00","Type":"URL","URL":"https://x.example"},
      {"FileName":"d.md","DateTime":"2023-05-01T00:00:03+00:00"},
      {"FileName":"e.jpg","DateTime":"2023-05-01T00:00:04+00:00","Type":"Video"}
    ]"#;
    let report = import_sharex_history(&db, json).unwrap();
    assert_eq!(report.imported, 5);
    let items = all(&db);
    let kinds = by_name(&items).into_iter().map(|(name, item)| (name, item.kind)).collect::<HashMap<_, _>>();
    assert_eq!(kinds["a.PNG"], HistoryKind::Image);
    assert_eq!(kinds["b.bin"], HistoryKind::File);
    assert_eq!(kinds["c"], HistoryKind::Url);
    assert_eq!(kinds["d.md"], HistoryKind::Text);
    assert_eq!(kinds["e.jpg"], HistoryKind::Image);
}

#[test]
fn a_shortened_url_alone_does_not_become_the_link() {
    let db = History::open_in_memory().unwrap();
    let json = r#"[{"FileName":"s.png","DateTime":"2023-05-01T00:00:00+00:00","Type":"Image","ShortenedURL":"https://short.example/x"}]"#;
    import_sharex_history(&db, json).unwrap();
    assert_eq!(all(&db)[0].url, None);
}

#[test]
fn a_second_import_skips_entries_already_in_the_history() {
    let db = History::open_in_memory().unwrap();
    assert_eq!(import_sharex_history(&db, ARRAY).unwrap().imported, 3);
    let again = import_sharex_history(&db, ARRAY).unwrap();
    assert_eq!(again, ImportReport { imported: 0, skipped: 3, failed: 0 });
    assert_eq!(all(&db).len(), 3);
}

#[test]
fn duplicates_are_matched_on_time_file_name_and_url() {
    let db = History::open_in_memory().unwrap();
    let json = r#"[
      {"FileName":"same.png","DateTime":"2023-05-01T10:00:00+00:00","URL":"https://a.example/1"},
      {"FileName":"same.png","DateTime":"2023-05-01T10:00:00+00:00","URL":"https://a.example/1"},
      {"FileName":"same.png","DateTime":"2023-05-01T10:00:00+00:00","URL":"https://a.example/2"},
      {"FileName":"same.png","DateTime":"2023-05-01T10:00:01+00:00","URL":"https://a.example/1"}
    ]"#;
    let report = import_sharex_history(&db, json).unwrap();
    assert_eq!(report, ImportReport { imported: 3, skipped: 1, failed: 0 });
}

#[test]
fn an_unreadable_entry_is_counted_as_failed_and_the_rest_import() {
    let db = History::open_in_memory().unwrap();
    let json = r#"[
      {"FileName":"ok.png","DateTime":"2023-05-01T00:00:00+00:00","Type":"Image"},
      {"FileName":"no-time.png","Type":"Image"},
      {"FileName":"bad-time.png","DateTime":"yesterday","Type":"Image"},
      {"FileName":"short-time.png","DateTime":"2023-05-01","Type":"Image"},
      42,
      {"FileName":["not","a","string"],"DateTime":"2023-05-01T00:00:05+00:00"}
    ]"#;
    let report = import_sharex_history(&db, json).unwrap();
    assert_eq!(report, ImportReport { imported: 1, skipped: 0, failed: 5 });
    assert_eq!(all(&db)[0].file_name, "ok.png");
}

#[test]
fn a_file_that_is_neither_form_is_an_error_and_stores_nothing() {
    let db = History::open_in_memory().unwrap();
    assert!(import_sharex_history(&db, "[{\"FileName\": \"x.png\", ").is_err());
    assert!(import_sharex_history(&db, "not json at all").is_err());
    assert!(all(&db).is_empty());
}

#[test]
fn import_reads_a_history_json_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("History.json");
    std::fs::write(&path, ARRAY).unwrap();
    let db = History::open_in_memory().unwrap();
    let report = import_sharex_history_file(&db, &path).unwrap();
    assert_eq!(report, ImportReport { imported: 3, skipped: 0, failed: 0 });
}

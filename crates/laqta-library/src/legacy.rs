//! Import of ShareX's `History.json`.
//! Older ShareX builds write comma-separated objects without the enclosing array; both forms are read.

use std::fs;
use std::path::{Path, PathBuf};

use laqta_domain::history::{HistoryKind, HistoryQuery, NewHistoryItem, kind_for_path};
use serde::Deserialize;
use serde_json::Value;

use crate::{History, LibraryError};

/// What one import did. `skipped` entries were already in the history. `failed` entries could not
/// be read and were not stored.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ImportReport {
    pub imported: u32,
    pub skipped: u32,
    pub failed: u32,
}

/// Imports the entries of a ShareX `History.json` text.
///
/// An unreadable entry counts as failed and the rest still import. A text that is neither a JSON
/// array nor a run of objects is an error, and nothing is stored. Each entry keeps the wall-clock
/// part of its `DateTime` and drops the offset. `URL` is the link and `ShortenedURL` its short form.
/// An entry whose `Type` is missing or unknown takes its kind from its file extension.
pub fn import_sharex_history(history: &History, json: &str) -> Result<ImportReport, LibraryError> {
    let entries = parse_entries(json)?;
    let mut report = ImportReport::default();
    for value in entries {
        let Some(item) = parse_entry(value) else {
            report.failed += 1;
            continue;
        };
        if is_duplicate(history, &item)? {
            report.skipped += 1;
        } else {
            history.add(&item)?;
            report.imported += 1;
        }
    }
    Ok(report)
}

/// Reads a `History.json` file and imports it. See [`import_sharex_history`].
pub fn import_sharex_history_file(history: &History, path: &Path) -> Result<ImportReport, LibraryError> {
    let json = fs::read_to_string(path)?;
    import_sharex_history(history, &json)
}

/// One ShareX history object. Field names follow ShareX's JSON. Fields not listed here are ignored.
#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct LegacyEntry {
    file_name: Option<String>,
    file_path: Option<String>,
    date_time: Option<String>,
    #[serde(rename = "Type")]
    kind: Option<String>,
    host: Option<String>,
    #[serde(rename = "URL")]
    url: Option<String>,
    #[serde(rename = "ThumbnailURL")]
    thumbnail_url: Option<String>,
    #[serde(rename = "DeletionURL")]
    deletion_url: Option<String>,
    #[serde(rename = "ShortenedURL")]
    shortened_url: Option<String>,
    tags: Option<Value>,
}

/// The objects in the file, from either form: one JSON array, or objects separated by commas.
fn parse_entries(json: &str) -> serde_json::Result<Vec<Value>> {
    let text = json.trim_start_matches('\u{feff}').trim();
    if let Ok(entries) = serde_json::from_str::<Vec<Value>>(text) {
        return Ok(entries);
    }
    let body = text.trim_end_matches(',').trim_end();
    serde_json::from_str::<Vec<Value>>(&format!("[{body}]"))
}

/// The stored form of one entry. `None` when the entry is not an object with the expected field
/// types, or has no usable `DateTime`.
fn parse_entry(value: Value) -> Option<NewHistoryItem> {
    let entry: LegacyEntry = serde_json::from_value(value).ok()?;
    let time = local_time(entry.date_time.as_deref()?)?;
    let path = non_empty(entry.file_path);
    let file_name = entry.file_name.unwrap_or_default();
    // FileName is a bare name, so it works on any host. A Windows FilePath may not split on Linux.
    let kind = match entry.kind.as_deref().and_then(kind_named) {
        Some(kind) => kind,
        None if !file_name.is_empty() => kind_for_path(Path::new(&file_name)),
        None => kind_for_path(Path::new(path.as_deref().unwrap_or_default())),
    };
    Some(NewHistoryItem {
        time,
        kind,
        file_name,
        path: path.map(PathBuf::from),
        url: non_empty(entry.url),
        thumbnail_url: non_empty(entry.thumbnail_url),
        deletion_url: non_empty(entry.deletion_url),
        short_url: non_empty(entry.shortened_url),
        host: non_empty(entry.host),
        width: None,
        height: None,
        size_bytes: None,
        thumbnail_path: None,
        tags: tags_of(entry.tags),
    })
}

/// The wall-clock part of an ISO 8601 time such as "2023-05-01T12:34:56.789+02:00".
fn local_time(text: &str) -> Option<jiff::civil::DateTime> {
    text.get(..19)?.parse().ok()
}

fn non_empty(text: Option<String>) -> Option<String> {
    text.filter(|text| !text.is_empty())
}

/// ShareX's `Type` values, matched without regard to case.
fn kind_named(text: &str) -> Option<HistoryKind> {
    match text.trim().to_ascii_lowercase().as_str() {
        "image" => Some(HistoryKind::Image),
        "text" => Some(HistoryKind::Text),
        "file" => Some(HistoryKind::File),
        "url" => Some(HistoryKind::Url),
        _ => None,
    }
}

/// ShareX writes tags as an object (`{"key": "value"}`) or as an array of strings. Both become plain
/// strings: `key=value`, or `key` alone when the value is empty.
fn tags_of(value: Option<Value>) -> Vec<String> {
    let tags: Vec<String> = match value {
        Some(Value::Object(map)) => map
            .into_iter()
            .map(|(key, value)| match value {
                Value::String(text) if !text.is_empty() => format!("{key}={text}"),
                Value::String(_) | Value::Null => key,
                other => format!("{key}={other}"),
            })
            .collect(),
        Some(Value::Array(items)) => items
            .into_iter()
            .filter_map(|item| match item {
                Value::String(text) => Some(text),
                _ => None,
            })
            .collect(),
        _ => Vec::new(),
    };
    tags.into_iter().map(|tag| tag.trim().to_owned()).filter(|tag| !tag.is_empty()).collect()
}

/// An entry is a duplicate when the history already holds one with the same time, file name and URL.
fn is_duplicate(history: &History, item: &NewHistoryItem) -> Result<bool, LibraryError> {
    let same_second = HistoryQuery { from: Some(item.time), to: Some(item.time), ..HistoryQuery::default() };
    Ok(history.query(&same_second)?.iter().any(|stored| stored.file_name == item.file_name && stored.url == item.url))
}

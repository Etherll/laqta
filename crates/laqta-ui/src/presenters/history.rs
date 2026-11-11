//! History rows and paging. Receives `laqta-domain` values; never imports the library crate.

use std::path::PathBuf;

use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery, HistoryStats};

pub const PAGE_SIZE: u32 = 50;

#[derive(Clone, Debug, PartialEq)]
pub struct HistoryRow {
    pub id: i64,
    pub title: String,
    pub when: String,
    pub size: String,
    pub dimensions: String,
    pub kind: &'static str,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    /// The shortened link, when the upload was shortened.
    pub short_url: Option<String>,
    pub thumbnail: Option<PathBuf>,
    pub tags: Vec<String>,
}

pub fn format_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut v = bytes as f64;
    let mut unit = 0;
    while v >= 1024.0 && unit < UNITS.len() - 1 {
        v /= 1024.0;
        unit += 1;
    }
    if unit == 0 { format!("{bytes} B") } else { format!("{v:.1} {}", UNITS[unit]) }
}

pub fn kind_label(kind: &HistoryKind) -> &'static str {
    match kind {
        HistoryKind::Image => "Image",
        HistoryKind::Text => "Text",
        HistoryKind::File => "File",
        HistoryKind::Url => "URL",
    }
}

pub fn row(item: &HistoryItem) -> HistoryRow {
    let t = item.time;
    HistoryRow {
        id: item.id,
        title: if item.file_name.is_empty() { item.url.clone().unwrap_or_default() } else { item.file_name.clone() },
        when: format!("{:04}-{:02}-{:02} {:02}:{:02}", t.year(), t.month(), t.day(), t.hour(), t.minute()),
        size: item.size_bytes.map(format_size).unwrap_or_default(),
        dimensions: match (item.width, item.height) {
            (Some(w), Some(h)) => format!("{w}×{h}"),
            _ => String::new(),
        },
        kind: kind_label(&item.kind),
        path: item.path.clone(),
        url: item.url.clone(),
        short_url: item.short_url.clone(),
        thumbnail: item.thumbnail_path.clone().or_else(|| if item.kind == HistoryKind::Image { item.path.clone() } else { None }),
        tags: item.tags.clone(),
    }
}

/// "1,204 items · 980 images · 1.2 GB".
pub fn stats_line(stats: &HistoryStats) -> String {
    let images = stats.by_kind.iter().find(|(k, _)| *k == HistoryKind::Image).map(|(_, n)| *n).unwrap_or(0);
    format!("{} items · {} images · {}", stats.total, images, format_size(stats.total_bytes))
}

/// Tags typed as "work, bug 12, todo" → trimmed, non-empty, de-duplicated, in order.
pub fn parse_tags(text: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for t in text.split(',').map(str::trim).filter(|t| !t.is_empty()) {
        if !out.iter().any(|o| o == t) {
            out.push(t.to_string());
        }
    }
    out
}

/// The query for one page of the history view. `page` starts at 0. A `#tag` word filters by tag.
pub fn page_query(text: &str, images_only: bool, page: u32) -> HistoryQuery {
    let mut tag = None;
    let words: Vec<&str> = text
        .split_whitespace()
        .filter(|w| match w.strip_prefix('#') {
            Some(t) if !t.is_empty() => {
                tag = Some(t.to_string());
                false
            }
            _ => true,
        })
        .collect();
    let text = words.join(" ");
    HistoryQuery {
        text: if text.is_empty() { None } else { Some(text) },
        tag,
        kind: images_only.then_some(HistoryKind::Image),
        offset: page * PAGE_SIZE,
        limit: PAGE_SIZE,
        ..HistoryQuery::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(format_size(512), "512 B");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(5 * 1024 * 1024), "5.0 MB");
    }

    #[test]
    fn tags_and_stats() {
        assert_eq!(parse_tags(" work, ,bug 12,work "), vec!["work".to_string(), "bug 12".to_string()]);
        let st = HistoryStats { total: 3, by_kind: vec![(HistoryKind::Image, 2), (HistoryKind::Text, 1)], total_bytes: 2048 };
        assert_eq!(stats_line(&st), "3 items · 2 images · 2.0 KB");
    }

    #[test]
    fn paging() {
        let q = page_query("  cat ", true, 2);
        assert_eq!(q.text.as_deref(), Some("cat"));
        assert_eq!(q.kind, Some(HistoryKind::Image));
        assert_eq!((q.offset, q.limit), (100, PAGE_SIZE));
        assert_eq!(page_query("", false, 0).text, None);
    }
}

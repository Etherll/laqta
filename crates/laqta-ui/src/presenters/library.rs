//! The library on the main window: tabs, the query each tab runs, and the text on each card.
//! Receives `laqta-domain` values; never imports the library crate.

use std::path::PathBuf;

use jiff::civil::DateTime;
use laqta_domain::history::{HistoryItem, HistoryKind, HistoryQuery};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LibraryTab {
    Recent,
    Images,
    Files,
    Uploads,
}

impl LibraryTab {
    pub const ALL: [LibraryTab; 4] = [LibraryTab::Recent, LibraryTab::Images, LibraryTab::Files, LibraryTab::Uploads];

    pub fn label(self) -> &'static str {
        match self {
            LibraryTab::Recent => "Recent",
            LibraryTab::Images => "Images",
            LibraryTab::Files => "Files",
            LibraryTab::Uploads => "Uploads",
        }
    }

    /// What an empty tab says.
    pub fn empty_text(self) -> (&'static str, &'static str) {
        match self {
            LibraryTab::Recent => ("No captures yet", "Press PrintScreen or pick a capture above."),
            LibraryTab::Images => ("No images yet", "Screenshots show up here."),
            LibraryTab::Files => ("No files yet", "Files you upload show up here."),
            LibraryTab::Uploads => ("Nothing uploaded yet", "Items with a link show up here."),
        }
    }
}

/// The history query behind a tab. Uploads have no query filter, so that tab asks for more rows
/// and [`keep`] drops the ones without a link.
pub fn query(tab: LibraryTab, search: &str, limit: u32) -> HistoryQuery {
    let text = search.trim();
    HistoryQuery {
        text: (!text.is_empty()).then(|| text.to_string()),
        kind: match tab {
            LibraryTab::Images => Some(HistoryKind::Image),
            LibraryTab::Files => Some(HistoryKind::File),
            LibraryTab::Recent | LibraryTab::Uploads => None,
        },
        limit: if tab == LibraryTab::Uploads { limit * 4 } else { limit },
        ..Default::default()
    }
}

/// Whether an item belongs on the tab once fetched.
pub fn keep(tab: LibraryTab, item: &HistoryItem) -> bool {
    tab != LibraryTab::Uploads || item.url.is_some()
}

/// What a card shows in place of a thumbnail.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CardGlyph {
    Image,
    Text,
    File,
    Link,
}

#[derive(Clone, Debug, PartialEq)]
pub struct LibraryCard {
    pub id: i64,
    /// The file name, or the link for link-only items.
    pub name: String,
    /// "Today 21:35", "Yesterday 09:12", "Wednesday", "8 Nov", "8 Nov 2025".
    pub when: String,
    /// "1192×488 · 43.0 KB".
    pub meta: String,
    pub glyph: CardGlyph,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub thumbnail: Option<PathBuf>,
    pub is_image: bool,
}

pub fn card(item: &HistoryItem, now: DateTime) -> LibraryCard {
    let row = super::history::row(item);
    let meta = [row.dimensions.clone(), row.size.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(" · ");
    LibraryCard {
        id: item.id,
        name: row.title,
        when: relative_when(item.time, now),
        meta,
        glyph: match item.kind {
            HistoryKind::Image => CardGlyph::Image,
            HistoryKind::Text => CardGlyph::Text,
            HistoryKind::File => CardGlyph::File,
            HistoryKind::Url => CardGlyph::Link,
        },
        path: row.path,
        url: row.url,
        thumbnail: row.thumbnail,
        is_image: item.kind == HistoryKind::Image,
    }
}

const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];

/// A short, human time: today and yesterday with the time, the past week by weekday, then a date.
pub fn relative_when(time: DateTime, now: DateTime) -> String {
    let hm = format!("{:02}:{:02}", time.hour(), time.minute());
    let days = now.date().since(time.date()).map(|span| span.get_days()).unwrap_or(i32::MAX);
    match days {
        0 => format!("Today {hm}"),
        1 => format!("Yesterday {hm}"),
        2..=6 => weekday(time).to_string(),
        _ if time.year() == now.year() => format!("{} {}", time.day(), MONTHS[time.month() as usize - 1]),
        _ => format!("{} {} {}", time.day(), MONTHS[time.month() as usize - 1], time.year()),
    }
}

fn weekday(time: DateTime) -> &'static str {
    use jiff::civil::Weekday::*;
    match time.weekday() {
        Monday => "Monday",
        Tuesday => "Tuesday",
        Wednesday => "Wednesday",
        Thursday => "Thursday",
        Friday => "Friday",
        Saturday => "Saturday",
        Sunday => "Sunday",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jiff::civil::date;

    #[test]
    fn relative_times() {
        let now = date(2026, 11, 8).at(21, 40, 0, 0);
        assert_eq!(relative_when(date(2026, 11, 8).at(21, 35, 0, 0), now), "Today 21:35");
        assert_eq!(relative_when(date(2026, 11, 7).at(9, 5, 0, 0), now), "Yesterday 09:05");
        assert_eq!(relative_when(date(2026, 11, 4).at(12, 0, 0, 0), now), "Wednesday");
        assert_eq!(relative_when(date(2026, 10, 2).at(12, 0, 0, 0), now), "2 Oct");
        assert_eq!(relative_when(date(2025, 10, 2).at(12, 0, 0, 0), now), "2 Oct 2025");
    }

    #[test]
    fn tabs_filter_by_kind_and_link() {
        assert_eq!(query(LibraryTab::Images, "", 24).kind, Some(HistoryKind::Image));
        assert_eq!(query(LibraryTab::Recent, "  cat ", 24).text.as_deref(), Some("cat"));
        assert_eq!(query(LibraryTab::Uploads, "", 24).limit, 96);
        let item = HistoryItem {
            id: 1,
            time: date(2026, 11, 8).at(0, 0, 0, 0),
            kind: HistoryKind::Image,
            file_name: "a.png".into(),
            path: None,
            url: None,
            thumbnail_url: None,
            deletion_url: None,
            host: None,
            width: Some(10),
            height: Some(5),
            size_bytes: Some(2048),
            thumbnail_path: None,
            tags: vec![],
        };
        assert!(!keep(LibraryTab::Uploads, &item));
        assert!(keep(LibraryTab::Recent, &item));
        let c = card(&item, date(2026, 11, 8).at(12, 0, 0, 0));
        assert_eq!(c.meta, "10×5 · 2.0 KB");
        assert_eq!(c.when, "Today 00:00");
    }
}

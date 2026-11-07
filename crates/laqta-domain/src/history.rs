//! History entries and the query that reads them (docs/plan/M1.md §4.1). Pure data: `laqta-library`
//! stores it in SQLite.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

const IMAGE_EXTENSIONS: [&str; 10] = ["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp", "avif", "jxl"];
const TEXT_EXTENSIONS: [&str; 3] = ["txt", "md", "log"];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HistoryKind {
    Image,
    Text,
    File,
    Url,
}

/// A stored entry. `time` is local time.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct HistoryItem {
    pub id: i64,
    #[serde(with = "civil_datetime")]
    pub time: jiff::civil::DateTime,
    pub kind: HistoryKind,
    pub file_name: String,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
    pub host: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: Option<u64>,
    pub thumbnail_path: Option<PathBuf>,
    pub tags: Vec<String>,
}

/// An entry before the store assigns its `id`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NewHistoryItem {
    #[serde(with = "civil_datetime")]
    pub time: jiff::civil::DateTime,
    pub kind: HistoryKind,
    pub file_name: String,
    pub path: Option<PathBuf>,
    pub url: Option<String>,
    pub thumbnail_url: Option<String>,
    pub deletion_url: Option<String>,
    pub host: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub size_bytes: Option<u64>,
    pub thumbnail_path: Option<PathBuf>,
    pub tags: Vec<String>,
}

/// Filters are ANDed. `text` matches each whitespace-separated word against the file name (prefix
/// match) or the URL and host (substring). `from` and `to` are both inclusive. `limit` 0 means no
/// limit. Results are newest first.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryQuery {
    pub text: Option<String>,
    pub kind: Option<HistoryKind>,
    pub tag: Option<String>,
    #[serde(with = "optional_civil_datetime", default)]
    pub from: Option<jiff::civil::DateTime>,
    #[serde(with = "optional_civil_datetime", default)]
    pub to: Option<jiff::civil::DateTime>,
    pub offset: u32,
    pub limit: u32,
}

/// Counts over all stored entries. `by_kind` lists every kind in declaration order, zeros included.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct HistoryStats {
    pub total: u64,
    pub by_kind: Vec<(HistoryKind, u64)>,
    pub total_bytes: u64,
}

/// The kind of entry a saved file makes: image extensions, text extensions, or any other file.
pub fn kind_for_path(path: &Path) -> HistoryKind {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return HistoryKind::File;
    };
    if IMAGE_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known)) {
        HistoryKind::Image
    } else if TEXT_EXTENSIONS.iter().any(|known| ext.eq_ignore_ascii_case(known)) {
        HistoryKind::Text
    } else {
        HistoryKind::File
    }
}

/// `civil::DateTime` as ISO text ("2026-11-07T14:03:22"). The workspace jiff has no serde feature.
mod civil_datetime {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &jiff::civil::DateTime, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.collect_str(value)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<jiff::civil::DateTime, D::Error> {
        let text = String::deserialize(deserializer)?;
        text.parse::<jiff::civil::DateTime>().map_err(D::Error::custom)
    }
}

mod optional_civil_datetime {
    use serde::de::Error as _;
    use serde::{Deserialize, Deserializer, Serializer};

    pub fn serialize<S: Serializer>(value: &Option<jiff::civil::DateTime>, serializer: S) -> Result<S::Ok, S::Error> {
        match value {
            Some(value) => serializer.collect_str(value),
            None => serializer.serialize_none(),
        }
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Option<jiff::civil::DateTime>, D::Error> {
        Option::<String>::deserialize(deserializer)?.map(|text| text.parse::<jiff::civil::DateTime>().map_err(D::Error::custom)).transpose()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_follows_extension_ignoring_case() {
        assert_eq!(kind_for_path(Path::new("a/Shot.PNG")), HistoryKind::Image);
        assert_eq!(kind_for_path(Path::new("pic.avif")), HistoryKind::Image);
        assert_eq!(kind_for_path(Path::new("notes.md")), HistoryKind::Text);
        assert_eq!(kind_for_path(Path::new("LOG.log")), HistoryKind::Text);
        assert_eq!(kind_for_path(Path::new("movie.mp4")), HistoryKind::File);
        assert_eq!(kind_for_path(Path::new("README")), HistoryKind::File);
    }

    #[test]
    fn query_defaults_to_everything() {
        let query = HistoryQuery::default();
        assert_eq!(query.offset, 0);
        assert_eq!(query.limit, 0);
        assert!(query.text.is_none() && query.kind.is_none() && query.tag.is_none());
        assert!(query.from.is_none() && query.to.is_none());
    }
}

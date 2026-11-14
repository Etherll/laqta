//! Settings search: which settings match what the user typed, best first. The entries come from
//! the rows the settings pages build (`widgets::collect_rows`), so the index is the pages
//! themselves and never drifts from them.

/// One setting a search can find.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Entry {
    /// Index of the page in the settings sidebar.
    pub page: usize,
    pub page_title: String,
    /// The section heading above the row, or empty.
    pub section: String,
    pub label: String,
    pub description: String,
}

/// The entries that match `query`, best first. Every word of the query must appear somewhere in
/// the entry (label, description, section or page title, ignoring case). A word at the start of
/// the label ranks first, then a word inside the label, then a match in the other text. Ties keep
/// the page order. A blank query matches nothing.
pub fn search<'a>(entries: &'a [Entry], query: &str) -> Vec<&'a Entry> {
    let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
    if words.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<(u8, usize, &Entry)> = entries
        .iter()
        .enumerate()
        .filter_map(|(i, e)| {
            let label = e.label.to_lowercase();
            let rest = format!("{} {} {}", e.description, e.section, e.page_title).to_lowercase();
            if !words.iter().all(|w| label.contains(w.as_str()) || rest.contains(w.as_str())) {
                return None;
            }
            let rank = if label.starts_with(words[0].as_str()) || label.split_whitespace().any(|lw| lw.starts_with(words[0].as_str())) {
                0
            } else if words.iter().any(|w| label.contains(w.as_str())) {
                1
            } else {
                2
            };
            Some((rank, i, e))
        })
        .collect();
    hits.sort_by_key(|(rank, i, _)| (*rank, *i));
    // The same label on the same page (a row built twice) shows once.
    let mut seen = std::collections::HashSet::new();
    hits.into_iter().filter(|(_, _, e)| seen.insert((e.page, e.label.clone()))).map(|(_, _, e)| e).collect()
}

/// "1 setting" / "12 settings" matching the query, for the results heading.
pub fn summary(count: usize, query: &str) -> String {
    let q = query.trim();
    match count {
        0 => format!("No settings match “{q}”"),
        n => format!("{} match “{q}”", crate::presenters::copy::count(n, "setting", "settings")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn e(page: usize, section: &str, label: &str, description: &str) -> Entry {
        Entry { page, page_title: format!("Page {page}"), section: section.into(), label: label.into(), description: description.into() }
    }

    #[test]
    fn every_word_must_match_and_label_starts_rank_first() {
        let entries = vec![
            e(0, "Sounds", "Play a sound", "After a capture is saved."),
            e(2, "Format", "JPEG quality", "Higher keeps more detail."),
            e(2, "Format", "Quality for large images", "Switch to JPEG past this size."),
        ];
        let hits: Vec<_> = search(&entries, "jpeg").iter().map(|e| e.label.as_str()).collect();
        assert_eq!(hits, ["JPEG quality", "Quality for large images"]);
        let hits: Vec<_> = search(&entries, "quality large").iter().map(|e| e.label.as_str()).collect();
        assert_eq!(hits, ["Quality for large images"]);
        assert_eq!(search(&entries, "sound").len(), 1, "the section and label both say sound; one hit");
        assert!(search(&entries, "   ").is_empty());
        assert!(search(&entries, "nothing-like-this").is_empty());
    }

    #[test]
    fn a_row_built_twice_on_one_page_shows_once() {
        let entries = vec![e(1, "", "Delay", ""), e(1, "", "Delay", "")];
        assert_eq!(search(&entries, "delay").len(), 1);
    }

    #[test]
    fn the_summary_counts_and_quotes() {
        assert_eq!(summary(0, " gif "), "No settings match “gif”");
        assert_eq!(summary(1, "gif"), "1 setting match “gif”");
        assert_eq!(summary(3, "gif"), "3 settings match “gif”");
    }
}

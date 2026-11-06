//! File-name patterns.

use std::path::{Path, PathBuf};

/// Candidate paths in order: `<dir>/<yyyy-mm>/Laqta_<yyyy-mm-dd_HH-mm-ss>.png`, then `_2`, `_3`, …
/// before `.png` (up to 999). Pure: it never checks the file system.
pub fn capture_candidates(dir: &Path, now: &jiff::civil::DateTime) -> impl Iterator<Item = PathBuf> {
    let folder = dir.join(format!("{:04}-{:02}", now.year(), now.month()));
    let stem = format!(
        "Laqta_{:04}-{:02}-{:02}_{:02}-{:02}-{:02}",
        now.year(),
        now.month(),
        now.day(),
        now.hour(),
        now.minute(),
        now.second()
    );
    (1u32..=999).map(move |n| {
        if n == 1 { folder.join(format!("{stem}.png")) } else { folder.join(format!("{stem}_{n}.png")) }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_in_order() {
        let now = jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0);
        let all: Vec<_> = capture_candidates(Path::new("/shots"), &now).collect();
        assert_eq!(all.len(), 999);
        assert_eq!(all[0], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22.png"));
        assert_eq!(all[1], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22_2.png"));
        assert_eq!(all[998], PathBuf::from("/shots/2026-11/Laqta_2026-11-07_14-03-22_999.png"));
    }
}

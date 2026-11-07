//! Watch folders (M6 D6): each enabled `WatchFolder` gets a `FileWatcher` watch; matching files are
//! (optionally moved into the screenshots folder and) uploaded with the folder's workflow.

use std::path::PathBuf;
use std::sync::{Arc, PoisonError};

use laqta_domain::settings::WatchFolder;
use laqta_ports::WatchCallback;

use crate::dispatcher::App;
use crate::files;

impl App {
    /// Replaces the running watches with the current settings' enabled folders. `main` calls it at
    /// start and after the settings change.
    pub fn start_watch_folders(self: &Arc<Self>) {
        let settings = self.settings.get();
        let mut watches = self.watches.lock().unwrap_or_else(PoisonError::into_inner);
        // Dropping the old handles stops their watches before the new ones start.
        watches.clear();
        for folder in settings.watch_folders.iter().filter(|f| f.enabled && !f.folder.as_os_str().is_empty()) {
            let on_file = self.watch_callback(folder.clone());
            match self.ports.watcher.watch(&folder.folder, folder.include_subfolders, on_file) {
                Ok(handle) => watches.push(handle),
                Err(error) => tracing::warn!(folder = %folder.folder.display(), %error, "watch folder not started"),
            }
        }
    }

    /// The callback for one folder. The watcher calls it on its own thread, so it only spawns the job. The job holds
    /// the App weakly, so a watch never keeps the App alive.
    fn watch_callback(self: &Arc<Self>, folder: WatchFolder) -> WatchCallback {
        let spawner = Arc::clone(&self.ports.spawner);
        let app = Arc::downgrade(self);
        Arc::new(move |path: PathBuf| {
            let app = app.clone();
            let folder = folder.clone();
            spawner.spawn(Box::pin(async move {
                if let Some(app) = app.upgrade() {
                    app.watched_file(path, folder).await;
                }
            }));
        })
    }

    /// One file from a watched folder. It runs only when the name matches the folder's filter. A folder that moves
    /// its files moves this one into the screenshots folder first, and the moved file is the one uploaded. A file
    /// that cannot be moved is logged and not uploaded.
    async fn watched_file(self: &Arc<Self>, path: PathBuf, folder: WatchFolder) {
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else { return };
        if !matches_filter(&folder.filter, &name) {
            return;
        }
        let path = if folder.move_to_screenshots {
            let dir = self.settings.get().screenshots_dir.clone();
            match files::move_to_free_name(self.ports.spawner.as_ref(), path.clone(), dir).await {
                Ok(moved) => moved,
                Err(error) => {
                    tracing::warn!(file = %path.display(), %error, "watched file not moved");
                    return;
                }
            }
        } else {
            path
        };
        self.upload_files(vec![path], folder.workflow);
    }
}

/// Whether `name` matches one of the `;` or `|` separated `filter` patterns. `*` and `?` are wildcards, case does
/// not matter, and a filter with no pattern in it matches every name.
fn matches_filter(filter: &str, name: &str) -> bool {
    let patterns: Vec<Vec<char>> = filter.split([';', '|']).map(str::trim).filter(|pattern| !pattern.is_empty()).map(fold).collect();
    if patterns.is_empty() {
        return true;
    }
    let name = fold(name);
    patterns.iter().any(|pattern| wildcard_matches(pattern, &name))
}

fn fold(text: &str) -> Vec<char> {
    text.chars().flat_map(char::to_lowercase).collect()
}

/// Glob matching for `*` and `?`. A mismatch falls back to the last `*`, which takes one more character, so no
/// recursion is needed.
fn wildcard_matches(pattern: &[char], text: &[char]) -> bool {
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        match pattern.get(p) {
            Some(&'*') => {
                star = Some((p, t));
                p += 1;
            }
            Some(&c) if c == '?' || c == text[t] => {
                p += 1;
                t += 1;
            }
            _ => match star {
                Some((star_p, star_t)) => {
                    p = star_p + 1;
                    t = star_t + 1;
                    star = Some((star_p, star_t + 1));
                }
                None => return false,
            },
        }
    }
    pattern[p..].iter().all(|&c| c == '*')
}

#[cfg(test)]
mod tests {
    use super::matches_filter;

    #[test]
    fn empty_filter_matches_everything() {
        assert!(matches_filter("", "anything.bin"));
        assert!(matches_filter("  ;  | ", "anything.bin"));
        assert!(matches_filter("*", ""));
    }

    #[test]
    fn star_and_question_mark_are_wildcards() {
        assert!(matches_filter("*.png", "shot.png"));
        assert!(matches_filter("*.png", ".png"));
        assert!(!matches_filter("*.png", "shot.png.part"));
        assert!(matches_filter("a?.png", "ab.png"));
        assert!(!matches_filter("a?.png", "a.png"));
        assert!(!matches_filter("a?.png", "abc.png"));
        assert!(matches_filter("*a*b*", "xxaxxbxx"));
        assert!(!matches_filter("*a*b*", "xxbxxaxx"));
    }

    #[test]
    fn matching_ignores_case() {
        assert!(matches_filter("*.PNG", "Shot.png"));
        assert!(matches_filter("*.png", "SHOT.PNG"));
    }

    #[test]
    fn any_of_several_patterns_matches() {
        assert!(matches_filter("*.jpg;*.png", "a.png"));
        assert!(matches_filter("*.jpg|*.gif", "a.gif"));
        assert!(matches_filter("  *.jpg ;  b? ", "b7"));
        assert!(!matches_filter("*.jpg;*.png", "a.gif"));
    }
}

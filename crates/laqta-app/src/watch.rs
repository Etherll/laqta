//! Watch folders: each enabled `WatchFolder` gets a `FileWatcher` watch; matching files are
//! (optionally moved into the screenshots folder and) uploaded with the folder's workflow.

use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError};

use laqta_domain::Action;
use laqta_domain::settings::WatchFolder;
use laqta_ports::WatchCallback;

use crate::dispatcher::{ActionInput, App};
use crate::files;

impl App {
    /// Replaces the running watches with the current settings' enabled folders. `main` calls it at
    /// start and after the settings change.
    pub fn start_watch_folders(self: &Arc<Self>) {
        let settings = self.settings.get();
        let mut watches = self.watches.lock().unwrap_or_else(PoisonError::into_inner);
        // Dropping the old handles stops their watches before the new ones start.
        watches.clear();
        let outputs = screenshot_folders(&settings);
        for folder in settings.watch_folders.iter().filter(|f| f.enabled && !f.folder.as_os_str().is_empty()) {
            // Laqta's own files must not come back as new ones, so its screenshots folder is not watched.
            if is_inside_any(&folder.folder, &outputs) {
                tracing::warn!(folder = %folder.folder.display(), "watch folder skipped: it is a screenshots folder");
                continue;
            }
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
    /// that cannot be moved is logged and not uploaded. The upload is a `FileUpload` dispatch.
    async fn watched_file(self: &Arc<Self>, path: PathBuf, folder: WatchFolder) {
        let Some(name) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else { return };
        if !matches_filter(&folder.filter, &name) {
            return;
        }
        // A folder above the screenshots folder, with its subfolders, sees Laqta's own files arrive.
        if is_inside_any(&path, &screenshot_folders(&self.settings.get())) {
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
        // The file goes through `dispatch` like any other delivery (ADR 015), with the folder's workflow.
        if let Err(error) = self.dispatch_with(Action::FileUpload, ActionInput::Files(vec![path.clone()]), folder.workflow).await {
            tracing::warn!(file = %path.display(), %error, "watched file not uploaded");
        }
    }
}

/// Every folder Laqta saves captures to: the default one, and the ones shortcuts override it with.
fn screenshot_folders(settings: &laqta_domain::Settings) -> Vec<PathBuf> {
    let mut folders = vec![settings.screenshots_dir.clone()];
    folders.extend(settings.defaults.screenshots_dir.clone());
    folders.extend(settings.workflows.iter().filter_map(|workflow| workflow.patch.screenshots_dir.clone()));
    folders.retain(|folder| !folder.as_os_str().is_empty());
    folders
}

/// Whether `path` is one of `folders` or below one. A pure rule on the path's components; Windows paths
/// ignore case.
fn is_inside_any(path: &Path, folders: &[PathBuf]) -> bool {
    folders.iter().any(|folder| is_inside(path, folder))
}

fn is_inside(path: &Path, folder: &Path) -> bool {
    is_inside_with(path, folder, cfg!(windows))
}

/// `is_inside`, with the Windows rules on or off: a `\\?\` verbatim prefix on either side is dropped, and
/// components compare with Unicode lowercasing.
fn is_inside_with(path: &Path, folder: &Path, windows: bool) -> bool {
    if !windows {
        return path.starts_with(folder);
    }
    let (path, folder) = (without_verbatim(path), without_verbatim(folder));
    let mut below = path.components();
    folder
        .components()
        .all(|part| below.next().is_some_and(|c| fold(&c.as_os_str().to_string_lossy()) == fold(&part.as_os_str().to_string_lossy())))
}

/// `path` without a leading `\\?\` (or `\\?\UNC\` as `\\`), which `canonicalize` adds and a typed path lacks.
fn without_verbatim(path: &Path) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = text.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        path.to_path_buf()
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
    use std::path::{Path, PathBuf};

    use super::{is_inside, is_inside_any, is_inside_with, matches_filter};

    #[test]
    fn windows_paths_ignore_case_and_a_verbatim_prefix() {
        let shots = Path::new("C:/Users/Ä/Shots");
        assert!(is_inside_with(Path::new("c:/users/ä/shots/2026/a.png"), shots, true));
        assert!(is_inside_with(Path::new(r"\\?\C:/Users/Ä/Shots/a.png"), shots, true));
        assert!(is_inside_with(Path::new("C:/Users/Ä/Shots/a.png"), Path::new(r"\\?\c:/users/ä/shots"), true));
        assert!(!is_inside_with(Path::new("C:/Users/Ä/Shots2/a.png"), shots, true));
        // Other systems compare exactly.
        assert!(!is_inside_with(Path::new("c:/users/ä/shots/a.png"), shots, false));
    }

    #[test]
    fn a_path_is_inside_a_folder_by_its_components() {
        let shots = Path::new("/data/shots");
        assert!(is_inside(Path::new("/data/shots"), shots));
        assert!(is_inside(Path::new("/data/shots/2026/a.png"), shots));
        assert!(!is_inside(Path::new("/data/shots2/a.png"), shots), "a name that only starts with it is another folder");
        assert!(!is_inside(Path::new("/data"), shots));
        assert!(!is_inside_any(Path::new("/data/x"), &[]));
        assert!(is_inside_any(Path::new("/b/a.png"), &[PathBuf::from("/a"), PathBuf::from("/b")]));
    }

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

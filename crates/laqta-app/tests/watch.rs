#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Watch folders (M6 D6), through the fake watcher: the watches follow the enabled folders, a matching file is uploaded
//! with the folder's workflow, a folder can move its files into the screenshots folder, and no watch keeps the App alive.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Weak};

use futures_lite::future::block_on;
use laqta_app::{App, Ports};
use laqta_domain::Settings;
use laqta_domain::settings::WatchFolder;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeDestination, FakeFileWatcher, FakeHoverSource, FakeNotifier, FakeOverlay,
    FakeScreen, FakeShell, FakeToolHost, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{FileWatcher, HostInfo, PortError, UploadData, UploadError, UploadResult, WatchCallback, WatchHandle};
use tempfile::TempDir;

struct H {
    app: Arc<App>,
    fake: Arc<FakeFileWatcher>,
    uploaders: Arc<FakeUploaders>,
    spawner: Arc<InlineSpawner>,
}

/// The app with the fake watcher, or the watcher `wrap` makes from it, and the settings after `edit`.
fn harness(dir: &Path, edit: impl FnOnce(&mut Settings)) -> H {
    harness_with_watcher(dir, |fake| fake, edit)
}

fn harness_with_watcher(
    dir: &Path,
    wrap: impl FnOnce(Arc<FakeFileWatcher>) -> Arc<dyn FileWatcher>,
    edit: impl FnOnce(&mut Settings),
) -> H {
    let fake = Arc::new(FakeFileWatcher::default());
    let uploaders = Arc::new(FakeUploaders::default());
    let spawner = Arc::new(InlineSpawner::default());
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: spawner.clone(),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        watcher: wrap(fake.clone()),
        uploaders: uploaders.clone(),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    let mut settings = Settings { screenshots_dir: dir.join("shots"), ..Settings::default_for(dir.to_path_buf()) };
    edit(&mut settings);
    H { app: App::new(ports, settings), fake, uploaders, spawner }
}

/// A watch on `path` with the filter, and every other field at its default (enabled, not recursive, not moved).
fn folder(path: &Path, filter: &str) -> WatchFolder {
    WatchFolder { folder: path.to_path_buf(), filter: filter.into(), ..WatchFolder::default() }
}

/// Creates `path` with some bytes in it and returns it.
fn file(path: &Path) -> PathBuf {
    std::fs::write(path, b"pixels").unwrap();
    path.to_path_buf()
}

/// A destination `id` that answers `results` uploads with a URL each. The kind of a file picks its destination: images
/// go to `imgur`, text to `sxcu:paste-ee`, and other files to `sxcu:uguu`.
fn host(h: &H, id: &str, results: usize) -> Arc<FakeDestination> {
    let results = (0..results).map(|n| Ok(UploadResult { url: Some(format!("https://u.example/{n}")), ..UploadResult::default() }));
    let destination = Arc::new(FakeDestination::new(id, results.collect::<Vec<Result<UploadResult, UploadError>>>()));
    h.uploaders.destinations.lock().unwrap().push(destination.clone());
    destination
}

/// Runs the queued jobs, the watch jobs and the uploads they submit, until none is left.
fn drain(spawner: &InlineSpawner) {
    loop {
        let jobs = spawner.take_spawned();
        if jobs.is_empty() {
            break;
        }
        for job in jobs {
            block_on(job);
        }
    }
}

fn live_folders(h: &H) -> Vec<PathBuf> {
    h.fake.live()
}

fn set_watch_folders(h: &H, folders: Vec<WatchFolder>) {
    let mut settings = h.app.settings();
    settings.watch_folders = folders;
    h.app.set_settings(settings);
}

#[test]
fn watch_uploads_matching_files_and_skips_the_rest() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    std::fs::create_dir(&watched).unwrap();
    let h = harness(tmp.path(), |s| s.watch_folders = vec![folder(&watched, "*.png")]);
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    let shot = file(&watched.join("a.png"));
    let note = file(&watched.join("notes.txt"));
    h.fake.emit(&shot);
    h.fake.emit(&note);
    drain(&h.spawner);

    let requests = uploads.requests.lock().unwrap();
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].file_name, "a.png");
    assert_eq!(requests[0].data, UploadData::Path(shot));
}

#[test]
fn watch_filter_takes_any_pattern_and_ignores_case() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    std::fs::create_dir(&watched).unwrap();
    let h = harness(tmp.path(), |s| s.watch_folders = vec![folder(&watched, "*.jpg|a?.PNG")]);
    let uploads = host(&h, "imgur", 2);
    h.app.start_watch_folders();

    for name in ["x.JPG", "b1.png", "a1.png", "a.png"] {
        h.fake.emit(&file(&watched.join(name)));
    }
    drain(&h.spawner);

    let names: Vec<String> = uploads.requests.lock().unwrap().iter().map(|r| r.file_name.clone()).collect();
    assert_eq!(names, vec!["x.JPG", "a1.png"]);
}

#[test]
fn watch_empty_filter_takes_every_file() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    std::fs::create_dir(&watched).unwrap();
    let h = harness(tmp.path(), |s| s.watch_folders = vec![folder(&watched, "")]);
    let uploads = host(&h, "sxcu:uguu", 2);
    h.app.start_watch_folders();

    h.fake.emit(&file(&watched.join("a.bin")));
    h.fake.emit(&file(&watched.join("b.bin")));
    drain(&h.spawner);

    assert_eq!(uploads.requests.lock().unwrap().len(), 2);
}

#[test]
fn watch_passes_the_include_subfolders_flag_to_the_watcher() {
    let tmp = TempDir::new().unwrap();
    let flat = tmp.path().join("flat");
    let deep = tmp.path().join("deep");
    std::fs::create_dir_all(deep.join("sub")).unwrap();
    std::fs::create_dir(&flat).unwrap();
    std::fs::create_dir(flat.join("sub")).unwrap();
    let h = harness(tmp.path(), |s| {
        s.watch_folders = vec![folder(&flat, "*"), WatchFolder { include_subfolders: true, ..folder(&deep, "*") }];
    });
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    let recursive: Vec<(PathBuf, bool)> = h.fake.watches.lock().unwrap().iter().map(|w| (w.0.clone(), w.1)).collect();
    assert_eq!(recursive, vec![(flat.clone(), false), (deep.clone(), true)]);

    // The watcher delivers a file below a recursive folder, and not one below a flat folder.
    h.fake.emit(&file(&deep.join("sub").join("n.png")));
    h.fake.emit(&file(&flat.join("sub").join("n.png")));
    drain(&h.spawner);
    assert_eq!(uploads.requests.lock().unwrap().len(), 1);
}

#[test]
fn watch_move_puts_the_file_in_the_screenshots_folder_and_uploads_it_there() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    let shots = tmp.path().join("shots");
    std::fs::create_dir(&watched).unwrap();
    let h = harness(tmp.path(), |s| {
        s.watch_folders = vec![WatchFolder { move_to_screenshots: true, ..folder(&watched, "*") }];
    });
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    h.fake.emit(&file(&watched.join("a.png")));
    drain(&h.spawner);

    assert!(!watched.join("a.png").exists());
    assert_eq!(std::fs::read(shots.join("a.png")).unwrap(), b"pixels");
    assert_eq!(uploads.requests.lock().unwrap()[0].data, UploadData::Path(shots.join("a.png")));
}

#[test]
fn watch_move_takes_the_next_free_name() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    let shots = tmp.path().join("shots");
    std::fs::create_dir_all(&watched).unwrap();
    std::fs::create_dir_all(&shots).unwrap();
    std::fs::write(shots.join("a.png"), b"old").unwrap();
    std::fs::write(shots.join("a (1).png"), b"older").unwrap();
    let h = harness(tmp.path(), |s| {
        s.watch_folders = vec![WatchFolder { move_to_screenshots: true, ..folder(&watched, "*") }];
    });
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    h.fake.emit(&file(&watched.join("a.png")));
    drain(&h.spawner);

    assert_eq!(std::fs::read(shots.join("a.png")).unwrap(), b"old");
    assert_eq!(std::fs::read(shots.join("a (1).png")).unwrap(), b"older");
    assert_eq!(std::fs::read(shots.join("a (2).png")).unwrap(), b"pixels");
    assert_eq!(uploads.requests.lock().unwrap()[0].data, UploadData::Path(shots.join("a (2).png")));
}

#[test]
fn watch_move_into_its_own_folder_leaves_the_file_where_it_is() {
    let tmp = TempDir::new().unwrap();
    let shots = tmp.path().join("shots");
    std::fs::create_dir(&shots).unwrap();
    let h = harness(tmp.path(), |s| {
        s.screenshots_dir = shots.clone();
        s.watch_folders = vec![WatchFolder { move_to_screenshots: true, ..folder(&shots, "*") }];
    });
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    let original = file(&shots.join("a.png"));
    h.fake.emit(&original);
    drain(&h.spawner);

    // A rename inside the watched folder would raise a new event for the same file, so nothing is renamed.
    assert_eq!(std::fs::read_dir(&shots).unwrap().count(), 1);
    assert_eq!(uploads.requests.lock().unwrap()[0].data, UploadData::Path(original));
}

#[test]
fn watch_file_that_cannot_be_moved_is_not_uploaded() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    std::fs::create_dir(&watched).unwrap();
    // The screenshots folder is a file, so the move cannot create it.
    let blocker = tmp.path().join("shots");
    std::fs::write(&blocker, b"not a folder").unwrap();
    let h = harness(tmp.path(), |s| {
        s.screenshots_dir = blocker.clone();
        s.watch_folders = vec![WatchFolder { move_to_screenshots: true, ..folder(&watched, "*") }];
    });
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    h.fake.emit(&file(&watched.join("a.png")));
    drain(&h.spawner);

    assert!(uploads.requests.lock().unwrap().is_empty());
    assert!(watched.join("a.png").exists());
}

#[test]
fn watch_restart_replaces_the_watches_and_skips_disabled_and_empty_folders() {
    let tmp = TempDir::new().unwrap();
    let first = tmp.path().join("first");
    let second = tmp.path().join("second");
    let disabled = tmp.path().join("disabled");
    std::fs::create_dir_all(&first).unwrap();
    std::fs::create_dir_all(&second).unwrap();
    std::fs::create_dir_all(&disabled).unwrap();
    let h = harness(tmp.path(), |s| {
        s.watch_folders = vec![
            folder(&first, "*"),
            WatchFolder { enabled: false, ..folder(&disabled, "*") },
            WatchFolder { folder: PathBuf::new(), ..WatchFolder::default() },
        ];
    });
    h.app.start_watch_folders();
    assert_eq!(live_folders(&h), vec![first.clone()]);

    set_watch_folders(&h, vec![folder(&second, "*")]);
    h.app.start_watch_folders();
    assert_eq!(live_folders(&h), vec![second.clone()]);
    // The first watch is gone for good: its handle was dropped, and no file reaches it any more.
    let uploads = host(&h, "imgur", 1);
    h.fake.emit(&file(&first.join("late.png")));
    drain(&h.spawner);
    assert!(uploads.requests.lock().unwrap().is_empty());
}

/// A watcher that cannot open one folder, as the platform watcher cannot for a folder that does not exist.
struct RefusingWatcher {
    inner: Arc<FakeFileWatcher>,
    refused: PathBuf,
}

impl FileWatcher for RefusingWatcher {
    fn watch(&self, folder: &Path, recursive: bool, on_file: WatchCallback) -> Result<WatchHandle, PortError> {
        if folder == self.refused {
            return Err(PortError::Failed("cannot open folder".into()));
        }
        self.inner.watch(folder, recursive, on_file)
    }
}

#[test]
fn watch_folder_that_cannot_be_watched_is_skipped_and_the_rest_still_watch() {
    let tmp = TempDir::new().unwrap();
    let gone = tmp.path().join("gone");
    let there = tmp.path().join("there");
    std::fs::create_dir(&there).unwrap();
    let refused = gone.clone();
    let h = harness_with_watcher(
        tmp.path(),
        move |fake| Arc::new(RefusingWatcher { inner: fake, refused }),
        |s| s.watch_folders = vec![folder(&gone, "*"), folder(&there, "*")],
    );
    h.app.start_watch_folders();

    assert_eq!(live_folders(&h), vec![there]);
}

#[test]
fn watch_callbacks_do_not_keep_the_app_alive() {
    let tmp = TempDir::new().unwrap();
    let watched = tmp.path().join("watched");
    std::fs::create_dir(&watched).unwrap();
    let h = harness(tmp.path(), |s| s.watch_folders = vec![folder(&watched, "*")]);
    let uploads = host(&h, "imgur", 1);
    h.app.start_watch_folders();

    let app: Weak<App> = Arc::downgrade(&h.app);
    let fake = h.fake.clone();
    let spawner = h.spawner.clone();
    drop(h);
    assert!(app.upgrade().is_none());

    // The watch still delivers, but its job finds no App and does nothing.
    fake.emit(&file(&watched.join("a.png")));
    drain(&spawner);
    assert!(uploads.requests.lock().unwrap().is_empty());
}

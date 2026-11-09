#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! History edits through the application (ADR 018): the windows ask `App`, which calls the `HistoryEdit` port on the
//! blocking pool. Without a history attached the calls report `Unsupported`.

use std::sync::Arc;

use futures_lite::future::block_on;
use laqta_app::{App, Ports};
use laqta_domain::Settings;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeHistoryEdit, FakeHoverSource, FakeNotifier, FakeOverlay, FakeScreen, FakeShell,
    FakeToolHost, FakeUploaders, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, HostInfo, PortError};

fn app(spawner: Arc<InlineSpawner>) -> Arc<App> {
    let ports = Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner,
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(laqta_ports::fakes::FakePrompt::default()),
        editor: Arc::new(laqta_ports::fakes::FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(laqta_ports::fakes::FakeScreenStream::default()),
        ocr: Arc::new(laqta_ports::fakes::FakeOcr::default()),
        window_ops: Arc::new(laqta_ports::fakes::FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(laqta_ports::fakes::FakeHighlighter::default()),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: laqta_imaging::Codecs::builtin(),
    };
    App::new(ports, Settings::default_for(std::env::temp_dir()))
}

#[test]
fn tags_and_deletes_reach_the_history_on_the_blocking_pool() {
    let spawner = Arc::new(InlineSpawner::default());
    let app = app(spawner.clone());
    let history = Arc::new(FakeHistoryEdit::default());
    app.attach_history(history.clone());

    block_on(app.history_set_tags(7, vec!["work".into(), "blue".into()])).unwrap();
    block_on(app.history_delete(7)).unwrap();

    assert_eq!(*history.tagged.lock().unwrap(), vec![(7, vec!["work".to_owned(), "blue".to_owned()])]);
    assert_eq!(*history.deleted.lock().unwrap(), vec![7]);
    assert_eq!(*spawner.blocking_calls.lock().unwrap(), 2, "the database calls ran through Spawner::run_blocking");
}

#[test]
fn without_a_history_the_edits_are_unsupported() {
    let app = app(Arc::new(InlineSpawner::default()));

    assert!(matches!(block_on(app.history_set_tags(1, Vec::new())), Err(AppError::Unsupported(_))));
    assert!(matches!(block_on(app.history_delete(1)), Err(AppError::Unsupported(_))));
}

#[test]
fn a_failed_edit_comes_back_as_an_app_error() {
    let app = app(Arc::new(InlineSpawner::default()));
    let history = Arc::new(FakeHistoryEdit::default());
    *history.fail.lock().unwrap() = Some(PortError::Failed("disk full".into()));
    app.attach_history(history.clone());

    let error = block_on(app.history_delete(3)).unwrap_err();

    assert_eq!(error.to_string(), AppError::Io("disk full".into()).to_string());
    assert!(history.deleted.lock().unwrap().is_empty());
}

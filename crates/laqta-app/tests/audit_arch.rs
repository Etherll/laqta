#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used)]

//! Audit probe (area `arch`): ADR 020 says each tool run is wrapped in `catch_unwind`, so a panic
//! becomes an error instead of unwinding out of `dispatch`. This plants a panicking port behind a
//! tool route (`Action::MouseHighlighter`) and expects `dispatch` to return `AppError::Panic`.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;

use futures_lite::FutureExt;
use futures_lite::future::block_on;
use laqta_app::{App, Ports};
use laqta_domain::{Action, Settings};
use laqta_imaging::Codecs;
use laqta_ports::fakes::{
    ClipboardBehaviour, CountingRng, FakeClipboard, FakeEditorHost, FakeHoverSource, FakeNotifier, FakeOcr, FakeOverlay, FakePrompt,
    FakeScreen, FakeScreenStream, FakeShell, FakeToolHost, FakeUploaders, FakeWindowOps, InlineSpawner, ManualClock,
};
use laqta_ports::{AppError, HostInfo, PointerHighlighter, PortError};

#[derive(Debug)]
struct PanickingHighlighter;

impl PointerHighlighter for PanickingHighlighter {
    fn set_enabled(&self, _on: bool) -> Result<(), PortError> {
        panic!("planted panic in a tool route")
    }
    fn is_enabled(&self) -> bool {
        panic!("planted panic in a tool route")
    }
}

fn ports() -> Ports {
    Ports {
        screen: Arc::new(FakeScreen::new(Vec::new())),
        clipboard: Arc::new(FakeClipboard::new(ClipboardBehaviour::Succeed)),
        shell: Arc::new(FakeShell::default()),
        clock: Arc::new(ManualClock::new(jiff::civil::date(2026, 11, 7).at(14, 3, 22, 0))),
        spawner: Arc::new(InlineSpawner::default()),
        overlay: Arc::new(FakeOverlay::default()),
        tools: Arc::new(FakeToolHost::default()),
        prompt: Arc::new(FakePrompt::default()),
        editor: Arc::new(FakeEditorHost::default()),
        media: Arc::new(laqta_ports::fakes::FakeMedia::default()),
        stream: Arc::new(FakeScreenStream::default()),
        ocr: Arc::new(FakeOcr::default()),
        window_ops: Arc::new(FakeWindowOps::default()),
        input: Arc::new(laqta_ports::fakes::FakeInputSynth::default()),
        highlighter: Arc::new(PanickingHighlighter),
        background: Arc::new(laqta_ports::fakes::FakeBackgroundRemover::default()),
        analyzer: Arc::new(laqta_ports::fakes::FakeAnalyzer::default()),
        inpainter: Arc::new(laqta_ports::fakes::FakeInpainter),
        watcher: Arc::new(laqta_ports::fakes::FakeFileWatcher::default()),
        uploaders: Arc::new(FakeUploaders::default()),
        hover: Arc::new(FakeHoverSource::new(Vec::new())),
        notifier: Arc::new(FakeNotifier::default()),
        rng: Arc::new(CountingRng::default()),
        host: HostInfo::default(),
        codecs: Codecs::builtin(),
    }
}

#[test]
fn audit_bug_tool_route_not_unwind_guarded() {
    let app = App::new(ports(), Settings::default());
    // Catch here so the probe reports a clean assertion failure instead of a raw panic.
    let outcome = block_on(AssertUnwindSafe(app.dispatch(Action::MouseHighlighter, None)).catch_unwind());
    match outcome {
        Ok(Err(AppError::Panic(_))) => {}
        Ok(other) => panic!("expected AppError::Panic, got {other:?}"),
        Err(_) => panic!("the tool route's panic unwound out of dispatch (no catch_unwind, ADR 020)"),
    }
}

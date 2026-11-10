//! The settings window's path rows: Browse asks the `Prompt` port for a folder.

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use laqta_ports::fakes::FakePrompt;

use super::{QueuedSpawner, install, services};

#[gpui::test]
fn browse_asks_the_folder_picker(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let prompt = Arc::new(FakePrompt::default());
    prompt.folders.lock().unwrap().push_back(Ok(PathBuf::from("/picked/Shots")));
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.prompt = prompt.clone();
    install(cx, ui);
    cx.update(crate::settings_window::open);
    let window = *cx.windows().last().expect("the settings window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    let bounds = vcx.debug_bounds("dir-browse").expect("the Browse button of the screenshots folder");
    vcx.simulate_click(bounds.center(), Modifiers::default());
    vcx.run_until_parked();

    assert!(prompt.folders.lock().unwrap().is_empty(), "Browse asked the port for a folder");
}

//! The macOS permissions onboarding: it opens at start only while Screen Recording is missing, and
//! its Allow button asks the platform for that permission.

use std::sync::{Arc, Mutex};

use gpui::{Modifiers, TestAppContext, VisualTestContext};
use laqta_ports::Permission;

use super::{QueuedSpawner, install, services};
use crate::runtime::PermissionHooks;

fn hooks(screen_granted: bool, asked: Arc<Mutex<Vec<Permission>>>) -> PermissionHooks {
    PermissionHooks {
        granted: Arc::new(move |p| p == Permission::ScreenRecording && screen_granted),
        request: Arc::new(move |p| asked.lock().unwrap().push(p)),
        relaunch: Arc::new(|| {}),
    }
}

#[gpui::test]
fn stays_closed_when_screen_recording_is_granted(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.permissions = Some(hooks(true, Arc::default()));
    install(cx, ui);
    cx.update(crate::permissions_window::open_if_needed);
    assert!(cx.windows().is_empty());
}

#[gpui::test]
fn stays_closed_where_the_os_has_no_permissions(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));
    cx.update(crate::permissions_window::open_if_needed);
    assert!(cx.windows().is_empty());
}

#[gpui::test]
fn opens_and_asks_for_screen_recording(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let asked = Arc::new(Mutex::new(Vec::new()));
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.permissions = Some(hooks(false, asked.clone()));
    install(cx, ui);
    cx.update(crate::permissions_window::open_if_needed);
    let window = *cx.windows().last().expect("the permissions window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();

    let bounds = vcx.debug_bounds("allow-screen-recording").expect("the Allow button of Screen Recording");
    vcx.simulate_click(bounds.center(), Modifiers::default());
    vcx.run_until_parked();
    assert_eq!(*asked.lock().unwrap(), vec![Permission::ScreenRecording]);
}

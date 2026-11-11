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

/// Opens the settings window, with the app's first-run settings changed by `tweak` as the
/// defaults, and draws it once.
fn open_with_defaults(cx: &mut TestAppContext, tweak: impl FnOnce(&mut laqta_domain::Settings)) -> (VisualTestContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut defaults = laqta_domain::Settings::default_for(dir.path().to_path_buf());
    tweak(&mut defaults);
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.default_settings = Arc::new(move || defaults.clone());
    install(cx, ui);
    cx.update(crate::settings_window::open);
    let window = *cx.windows().last().expect("the settings window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
    (vcx, dir)
}

fn draw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn a_changed_switch_offers_its_reset_and_the_page_reset(cx: &mut TestAppContext) {
    // The app's settings keep history on; these defaults have it off, so the row differs.
    let (mut vcx, _dir) = open_with_defaults(cx, |d| d.history.enabled = false);
    assert!(vcx.debug_bounds("page-reset").is_some(), "General differs from its defaults");
    let reset = vcx.debug_bounds("history-reset").expect("the history row's reset button");

    vcx.simulate_click(reset.center(), Modifiers::default());
    draw(&mut vcx);

    assert!(vcx.debug_bounds("history-reset").is_none(), "the row is back at its default");
}

#[gpui::test]
fn settings_at_their_defaults_show_no_reset(cx: &mut TestAppContext) {
    let (mut vcx, _dir) = open_with_defaults(cx, |_| {});
    draw(&mut vcx);
    for id in ["page-reset", "history-reset", "toast-ms-reset", "toast-pos-reset", "theme-reset", "dir-browse-reset"] {
        assert!(vcx.debug_bounds(id).is_none(), "{id} shows at the defaults");
    }
}

#[gpui::test]
fn a_changed_field_and_choice_offer_their_resets(cx: &mut TestAppContext) {
    use laqta_domain::settings::Theme;
    // Both rows are near the top of the page, so their buttons are inside the test window on every
    // platform (the window's size is the platform's default).
    let (mut vcx, _dir) = open_with_defaults(cx, |d| {
        d.screenshots_dir = d.screenshots_dir.join("elsewhere");
        d.theme = if d.theme == Theme::Dark { Theme::Light } else { Theme::Dark };
    });
    draw(&mut vcx);
    let choice = vcx.debug_bounds("theme-reset").expect("the theme choice's reset button");
    vcx.simulate_click(choice.center(), Modifiers::default());
    draw(&mut vcx);
    assert!(vcx.debug_bounds("theme-reset").is_none(), "the choice is back at its default");

    let field = vcx.debug_bounds("dir-browse-reset").expect("the folder field's reset button");
    vcx.simulate_click(field.center(), Modifiers::default());
    draw(&mut vcx);
    assert!(vcx.debug_bounds("dir-browse-reset").is_none(), "the field shows its default text");
}

/// On a platform whose window can't see some keys (Windows: Print Screen, media keys), recording a
/// shortcut also starts the system key hook, and a chord from the hook ends the recording.
#[gpui::test]
fn a_chord_from_the_key_hook_ends_the_recording(cx: &mut TestAppContext) {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    use laqta_domain::{Key, KeyChord, Modifiers as Mods};
    use laqta_ports::{ChordCallback, KeyRecording};

    struct Stopped(Arc<AtomicBool>);
    impl Drop for Stopped {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }
    let dir = tempfile::tempdir().unwrap();
    let sink: Arc<Mutex<Option<ChordCallback>>> = Arc::default();
    let stopped = Arc::new(AtomicBool::new(false));
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.record_keys = Some({
        let (sink, stopped) = (sink.clone(), stopped.clone());
        Arc::new(move |on_chord| {
            *sink.lock().unwrap() = Some(on_chord);
            Some(KeyRecording::new(Stopped(stopped.clone())))
        })
    });
    install(cx, ui);
    cx.update(crate::settings_window::open);
    let window = *cx.windows().last().expect("the settings window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    draw(&mut vcx);
    let nav = vcx.debug_bounds("nav-Shortcuts").expect("the Shortcuts page in the sidebar");
    vcx.simulate_click(nav.center(), Modifiers::default());
    draw(&mut vcx);
    let chip = vcx.debug_bounds("wf-key-0").expect("the first shortcut's key");
    vcx.simulate_click(chip.center(), Modifiers::default());
    draw(&mut vcx);

    let on_chord = sink.lock().unwrap().clone().expect("recording started the key hook");
    on_chord(KeyChord { modifiers: Mods::CTRL, key: Key::PrintScreen });
    draw(&mut vcx);
    assert!(stopped.load(Ordering::SeqCst), "the recorded chord ended the recording and its hook");
}

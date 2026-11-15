//! The shared pieces from the UI polish pass: Escape closes small windows, and Flash clears itself
//! without an older timer clearing a newer message.

use std::sync::Arc;
use std::time::Duration;

use gpui::{AppContext as _, Context, IntoElement, Modifiers, Render, TestAppContext, VisualTestContext, Window, div};

use super::{QueuedSpawner, install, services};
use crate::flash::{Flash, HasFlash};

#[gpui::test]
fn escape_closes_the_url_window_but_not_the_settings_window(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    install(cx, services(QueuedSpawner::default(), dir.path()));

    cx.update(crate::settings_window::open);
    let settings = *cx.windows().last().expect("the settings window");
    cx.update(|cx| crate::url_window::open("https://example.com/a.png".into(), false, cx));
    let url = *cx.windows().last().expect("the URL window");
    assert_ne!(settings, url);

    let mut vcx = VisualTestContext::from_window(url, cx);
    vcx.run_until_parked();
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(!cx.windows().contains(&url), "Escape closed the URL window");

    let mut vcx = VisualTestContext::from_window(settings, cx);
    vcx.simulate_keystrokes("escape");
    vcx.run_until_parked();
    assert!(cx.windows().contains(&settings), "Escape leaves the settings window open");
}

struct Flashing {
    flash: Flash,
}

impl HasFlash for Flashing {
    fn flash(&mut self) -> &mut Flash {
        &mut self.flash
    }
}

impl Render for Flashing {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

fn shown(view: &gpui::Entity<Flashing>, cx: &mut TestAppContext) -> Option<String> {
    view.read_with(cx, |v, _| v.flash.current().map(|n| n.title.clone()))
}

#[gpui::test]
fn a_flash_clears_itself_and_an_older_timer_never_clears_a_newer_one(cx: &mut TestAppContext) {
    let view = cx.new(|_| Flashing { flash: Flash::default() });
    view.update(cx, |v, cx| Flash::show(v, "Copied", cx));
    cx.executor().advance_clock(Duration::from_millis(3000));
    view.update(cx, |v, cx| Flash::show(v, "Copied the link", cx));
    // The first flash's timer fires here; the second one stays.
    cx.executor().advance_clock(Duration::from_millis(2500));
    assert_eq!(shown(&view, cx).as_deref(), Some("Copied the link"));
    cx.executor().advance_clock(Duration::from_millis(3000));
    assert_eq!(shown(&view, cx), None, "the second flash cleared itself");
}

#[gpui::test]
fn a_failure_stays_until_dismissed(cx: &mut TestAppContext) {
    let view = cx.new(|_| Flashing { flash: Flash::default() });
    view.update(cx, |v, cx| Flash::fail(v, "Couldn't copy the image", cx));
    cx.executor().advance_clock(Duration::from_secs(60));
    assert_eq!(shown(&view, cx).as_deref(), Some("Couldn't copy the image"));
    view.update(cx, Flash::dismiss);
    assert_eq!(shown(&view, cx), None);
}

/// Opens the settings window with `tweak` applied to the defaults, and draws it once.
fn open_settings_with_defaults(
    cx: &mut TestAppContext,
    tweak: impl FnOnce(&mut laqta_domain::Settings),
) -> (VisualTestContext, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let mut defaults = laqta_domain::Settings::default_for(dir.path().to_path_buf());
    tweak(&mut defaults);
    let mut ui = services(QueuedSpawner::default(), dir.path());
    ui.default_settings = Arc::new(move || defaults.clone());
    install(cx, ui);
    cx.update(crate::settings_window::open);
    let window = *cx.windows().last().expect("the settings window");
    let mut vcx = VisualTestContext::from_window(window, cx);
    redraw(&mut vcx);
    (vcx, dir)
}

fn redraw(vcx: &mut VisualTestContext) {
    vcx.update(|window, cx| window.draw(cx).clear(cx));
    vcx.run_until_parked();
}

#[gpui::test]
fn a_search_hit_opens_the_page_that_draws_its_row(cx: &mut TestAppContext) {
    // The defaults differ from the app's settings, so the JPEG row offers its reset button, which
    // only the Format page draws.
    let (mut vcx, _dir) = open_settings_with_defaults(cx, |d| d.defaults.image.jpeg_quality = 70);
    assert!(vcx.debug_bounds("jpeg-quality-reset").is_none(), "the JPEG row is not on the General page");

    let search = vcx.debug_bounds("settings-search").expect("the search box");
    vcx.simulate_click(search.center(), Modifiers::default());
    vcx.simulate_input("jpeg");
    redraw(&mut vcx);

    let hit = vcx.debug_bounds("search-hit-0").expect("the first result for \"jpeg\"");
    vcx.simulate_click(hit.center(), Modifiers::default());
    redraw(&mut vcx);

    assert!(vcx.debug_bounds("search-hit-0").is_none(), "opening a result empties the search");
    assert!(vcx.debug_bounds("jpeg-quality-reset").is_some(), "the Format page draws the JPEG quality row");
}

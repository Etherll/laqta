//! Takes a window off screen at once before a capture. Minimizing alone is not enough: the
//! compositor animates it (KWin for about 200 ms) and a capture taken meanwhile shows the window
//! half faded. So the window first draws a blank frame (its surface is transparent under
//! client-side decorations), then minimizes, and draws again when it is next activated.

use std::sync::Arc;

use gpui::{App, Context, Subscription, Window};
use gpui_component::{is_window_blank, set_window_blank};
use laqta_domain::Action;

/// Blanks `window`, and once the blank frame is drawn, takes it off the screen and dispatches
/// `action`. Where the platform can remove the window without an animation it does (Windows);
/// elsewhere the window is minimized. On macOS nothing is hidden, and nothing is hidden anywhere
/// when the capture includes Laqta's own windows: the setting of the workflow `action` runs under,
/// which is the one its capture reads.
pub(crate) fn then_dispatch(action: Action, dispatch: Arc<dyn Fn(Action) + Send + Sync>, window: &mut Window, cx: &mut App) {
    let include_own_windows = cx.try_global::<crate::runtime::UiState>().is_some_and(|state| {
        let settings = state.services.app.settings();
        settings.task_settings(settings.workflow_for_action(action).map(|w| w.id)).capture.include_own_windows
    });
    dispatch_for(include_own_windows, action, dispatch, window, cx);
}

/// Whether the window leaves the screen before a capture starts. macOS captures leave Laqta's own
/// windows out (ADR 030), so the window stays where it is; and when the capture includes them it
/// must stay on any platform.
fn hides_before_capture(include_own_windows: bool) -> bool {
    !include_own_windows && !cfg!(target_os = "macos")
}

fn dispatch_for(include_own_windows: bool, action: Action, dispatch: Arc<dyn Fn(Action) + Send + Sync>, window: &mut Window, cx: &mut App) {
    if !hides_before_capture(include_own_windows) {
        dispatch(action);
        return;
    }
    blank_then(window, cx, move |window, cx| {
        let hide = cx.try_global::<crate::runtime::UiState>().map(|state| state.services.native_window.clone());
        if !hide.is_some_and(|hide| hide(window, crate::runtime::NativeWindow::HideForCapture)) {
            window.minimize_window();
        }
        dispatch(action);
    });
}

/// Blanks `window` and runs `then` once the blank frame is drawn.
fn blank_then(window: &mut Window, cx: &mut App, then: impl FnOnce(&mut Window, &mut App) + 'static) {
    set_window_blank(window, true, cx);
    window.on_next_frame(then);
}

/// Draws the view's window again when it is activated (restored from the taskbar, or reopened).
pub(crate) fn restore_on_activation<V: 'static>(window: &mut Window, cx: &mut Context<V>) -> Subscription {
    cx.observe_window_activation(window, |_, window, cx| {
        if window.is_window_active() && is_window_blank(window, cx) {
            set_window_blank(window, false, cx);
        }
    })
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use std::sync::atomic::{AtomicUsize, Ordering};

    use gpui::{IntoElement, Render, TestAppContext, div};

    use super::*;

    struct Probe;

    impl Render for Probe {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    fn counting(counter: &Arc<AtomicUsize>) -> Arc<dyn Fn(Action) + Send + Sync> {
        let counter = counter.clone();
        Arc::new(move |_| _ = counter.fetch_add(1, Ordering::SeqCst))
    }

    #[test]
    fn including_own_windows_never_hides() {
        assert!(!hides_before_capture(true));
        assert_eq!(hides_before_capture(false), !cfg!(target_os = "macos"));
    }

    #[gpui::test]
    fn the_window_stays_up_when_captures_include_it_and_hides_when_they_do_not(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|window, cx| {
            restore_on_activation(window, cx).detach();
            Probe
        });
        let dispatched = Arc::new(AtomicUsize::new(0));

        vcx.update(|window, cx| dispatch_for(true, Action::RectangleRegion, counting(&dispatched), window, cx));
        assert_eq!(dispatched.load(Ordering::SeqCst), 1, "the capture starts at once");
        assert!(!vcx.update(|window, cx| is_window_blank(window, cx)), "the window is left as it is");

        vcx.update(|window, cx| dispatch_for(false, Action::RectangleRegion, counting(&dispatched), window, cx));
        if cfg!(target_os = "macos") {
            assert_eq!(dispatched.load(Ordering::SeqCst), 2, "macOS never hides the window");
            assert!(!vcx.update(|window, cx| is_window_blank(window, cx)));
        } else {
            assert_eq!(dispatched.load(Ordering::SeqCst), 1, "the capture waits for the blank frame");
            assert!(vcx.update(|window, cx| is_window_blank(window, cx)), "the window is blanked before it hides");
        }
    }

    #[gpui::test]
    fn the_window_blanks_before_it_minimizes_and_returns_when_activated(cx: &mut TestAppContext) {
        let (_, vcx) = cx.add_window_view(|window, cx| {
            restore_on_activation(window, cx).detach();
            Probe
        });
        let ran = Arc::new(AtomicUsize::new(0));
        let counter = ran.clone();

        vcx.update(|window, cx| blank_then(window, cx, move |_, _| _ = counter.fetch_add(1, Ordering::SeqCst)));
        assert!(vcx.update(|window, cx| is_window_blank(window, cx)));
        assert_eq!(ran.load(Ordering::SeqCst), 0, "nothing runs before the blank frame is drawn");

        vcx.update(|window, cx| window.simulate_next_frame(cx));
        assert_eq!(ran.load(Ordering::SeqCst), 1, "the capture starts after the blank frame");

        vcx.deactivate_window();
        vcx.update(|window, _| window.activate_window());
        vcx.run_until_parked();
        assert!(!vcx.update(|window, cx| is_window_blank(window, cx)), "activation draws the window again");
    }
}

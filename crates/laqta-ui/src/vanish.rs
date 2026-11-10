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
/// elsewhere the window is minimized.
pub(crate) fn then_dispatch(action: Action, dispatch: Arc<dyn Fn(Action) + Send + Sync>, window: &mut Window, cx: &mut App) {
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

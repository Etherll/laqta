//! Asks before a window with unsaved edits closes (the editor and the photo editor).

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;

use gpui::{App, AppContext as _, Context, PromptLevel, WeakEntity, Window, WindowId};

type Hook = Rc<dyn Fn(&mut Window, &mut App) -> bool>;

thread_local! {
    /// The guard of each guarded window, for the close button Laqta draws itself on Linux: it
    /// closes the window directly, so the platform's close request (and GPUI's hook) never runs.
    static HOOKS: RefCell<HashMap<WindowId, Hook>> = RefCell::new(HashMap::new());
}

/// What a guarded window says in its prompt and does with each answer.
pub(crate) struct Guard<T> {
    /// True while closing would lose edits.
    pub dirty: fn(&T) -> bool,
    pub message: fn(&T) -> &'static str,
    pub detail: fn(&T) -> &'static str,
    /// The first answer ("Save", "Use edits"). The window stays open until the save lands.
    pub keep: fn(&T) -> &'static str,
    pub on_keep: fn(&mut T, &mut Window, &mut Context<T>),
}

const DISCARD: &str = "Don't save";
const CANCEL: &str = "Cancel";

/// Installs the guard on the window's close request (the title bar's ✕, Alt+F4).
pub(crate) fn install<T: 'static>(guard: fn() -> Guard<T>, window: &mut Window, cx: &mut Context<T>) {
    let this = cx.entity().downgrade();
    let hook: Hook = Rc::new(move |window, cx| {
        let Some(view) = this.upgrade() else { return true };
        let guard = guard();
        let prompt = texts(&guard, view.read(cx));
        ask(prompt, guard.on_keep, this.clone(), window, cx)
    });
    let id = window.window_handle().window_id();
    HOOKS.with(|h| h.borrow_mut().insert(id, hook.clone()));
    cx.on_release(move |_, _| HOOKS.with(|h| drop(h.borrow_mut().remove(&id)))).detach();
    window.on_window_should_close(cx, move |window, cx| hook(window, cx));
}

/// The title bar's close button: closes the window unless its guard holds it open.
pub(crate) fn request_close(window: &mut Window, cx: &mut App) {
    let hook = HOOKS.with(|h| h.borrow().get(&window.window_handle().window_id()).cloned());
    if hook.is_none_or(|hook| hook(window, cx)) {
        window.remove_window();
    }
}

/// The prompt's message, detail and first answer, or `None` when nothing would be lost.
fn texts<T>(guard: &Guard<T>, state: &T) -> Option<[&'static str; 3]> {
    (guard.dirty)(state).then(|| [(guard.message)(state), (guard.detail)(state), (guard.keep)(state)])
}

/// True when the window may close now. Otherwise it shows the prompt and acts on the answer later.
fn ask<T: 'static>(
    prompt: Option<[&'static str; 3]>,
    on_keep: fn(&mut T, &mut Window, &mut Context<T>),
    view: WeakEntity<T>,
    window: &mut Window,
    cx: &mut App,
) -> bool {
    let Some([message, detail, keep]) = prompt else { return true };
    let answer = window.prompt(PromptLevel::Warning, message, Some(detail), &[keep, DISCARD, CANCEL], cx);
    window
        .spawn(cx, async move |cx| {
            let Ok(choice) = answer.await else { return };
            let _ = cx.update(|window, cx| match choice {
                0 => {
                    let _ = view.update(cx, |state, cx| on_keep(state, window, cx));
                }
                1 => window.remove_window(),
                _ => {}
            });
        })
        .detach();
    false
}

/// Closes `window` once the current update ends (from inside an entity update), without asking.
pub(crate) fn close<T: 'static>(window: gpui::AnyWindowHandle, cx: &mut Context<T>) {
    cx.defer(move |cx| {
        let _ = cx.update_window(window, |_, window, _| window.remove_window());
    });
}

//! Flash: the short status line a window shows after an action ("Copied the link"), or a failure
//! that stays until it is replaced or dismissed. One module, so every window gets the same timing
//! (`presenters::notice::lifetime`), the same race guard (a newer flash is never cleared by an
//! older timer) and the same look.

use gpui::{
    App, ClickEvent, Context, Div, IntoElement as _, ParentElement as _, SharedString, Styled as _, Window, div,
    prelude::FluentBuilder as _,
};
use gpui_component::ActiveTheme as _;

use crate::assets::IconName;
use crate::presenters::notice::{self, Notice};
use crate::theme::{space, text};
use crate::widgets;

/// What a window is flashing now. Hold one in the view and expose it through [`HasFlash`].
#[derive(Default)]
pub(crate) struct Flash {
    shown: Option<Notice>,
    seq: u64,
}

/// A view that holds a [`Flash`].
pub(crate) trait HasFlash: 'static + Sized {
    fn flash(&mut self) -> &mut Flash;
}

impl Flash {
    /// Shows `title` as a passing note.
    pub(crate) fn show<V: HasFlash>(view: &mut V, title: impl Into<String>, cx: &mut Context<V>) {
        Self::notice(view, Notice::info(title), cx);
    }

    /// Shows `title` as a failure, which stays until replaced or dismissed.
    pub(crate) fn fail<V: HasFlash>(view: &mut V, title: impl Into<String>, cx: &mut Context<V>) {
        Self::notice(view, Notice::error(title), cx);
    }

    /// Shows `n`; it clears itself after `notice::lifetime`, unless a newer one replaced it.
    pub(crate) fn notice<V: HasFlash>(view: &mut V, n: Notice, cx: &mut Context<V>) {
        let f = view.flash();
        f.seq += 1;
        let seq = f.seq;
        let life = notice::lifetime(&n);
        f.shown = Some(n);
        cx.notify();
        if let Some(life) = life {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(life).await;
                let _ = this.update(cx, |v, cx| {
                    let f = v.flash();
                    if f.seq == seq {
                        f.shown = None;
                        cx.notify();
                    }
                });
            })
            .detach();
        }
    }

    /// Clears whatever is shown.
    pub(crate) fn dismiss<V: HasFlash>(view: &mut V, cx: &mut Context<V>) {
        let f = view.flash();
        f.seq += 1;
        if f.shown.take().is_some() {
            cx.notify();
        }
    }

    /// The notice shown now, if any.
    pub(crate) fn current(&self) -> Option<&Notice> {
        self.shown.as_ref()
    }

    /// The flash line: muted text with a check for a note; danger text with an alert icon, the
    /// reason and a dismiss button for a failure. `None` when nothing is shown.
    pub(crate) fn render(&self, on_dismiss: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static, cx: &App) -> Option<Div> {
        let n = self.shown.as_ref()?;
        let t = cx.theme();
        let (glyph, color) = if n.error { (IconName::TriangleAlert, t.danger) } else { (IconName::Check, t.muted_foreground) };
        let title: SharedString = n.title.clone().into();
        let reason: Option<SharedString> = n.reason.clone().map(Into::into);
        Some(
            div()
                .flex()
                .items_center()
                .gap(space::S2)
                .min_w_0()
                .text_size(text::LABEL)
                .text_color(color)
                .child(widgets::icon(glyph, color))
                .child(div().min_w_0().truncate().child(title))
                .children(reason.map(|r| div().min_w_0().truncate().text_color(t.muted_foreground).child(r)))
                .when(n.error, |d| {
                    d.child(widgets::icon_button("flash-dismiss", IconName::CircleX, "Dismiss", false, on_dismiss, cx).into_any_element())
                }),
        )
    }
}

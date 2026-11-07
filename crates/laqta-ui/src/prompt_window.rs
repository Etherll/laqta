//! The `Prompt` port's dialogs: ask for text, choose from a list, show text, confirm. Each answers
//! once over a oneshot channel; closing the window answers `Cancelled`.

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowKind, WindowOptions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root};
use laqta_ports::PortError;

use crate::widgets;

pub(crate) enum PromptKind {
    Text { default: String },
    Choose { options: Vec<String> },
    Show { text: String },
    Confirm { text: String },
}

pub(crate) enum Answer {
    Text(String),
    Index(usize),
    Ok,
    Bool(bool),
}

pub(crate) type Reply = oneshot::Sender<Result<Answer, PortError>>;

pub(crate) fn open(title: String, kind: PromptKind, reply: Reply, cx: &mut App) {
    let height = match &kind {
        PromptKind::Choose { options } => 120.0 + 36.0 * options.len().min(10) as f32,
        PromptKind::Show { .. } => 360.0,
        _ => 170.0,
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(460.), px(height)), cx)),
        titlebar: Some(TitlebarOptions { title: Some(SharedString::from(title.clone())), ..Default::default() }),
        kind: WindowKind::Dialog,
        focus: true,
        app_id: Some("laqta".into()),
        ..Default::default()
    };
    let mut reply = Some(reply);
    let result = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| PromptWindow::new(title, kind, reply.take(), window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    });
    if let Err(e) = result {
        tracing::warn!("prompt window failed: {e:#}");
        if let Some(r) = reply {
            let _ = r.send(Err(PortError::Unsupported("prompt window".into())));
        }
    }
}

struct PromptWindow {
    title: String,
    kind: PromptKind,
    input: Option<Entity<InputState>>,
    reply: Option<Reply>,
}

impl PromptWindow {
    fn new(title: String, kind: PromptKind, reply: Option<Reply>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let input = match &kind {
            PromptKind::Text { default } => {
                let d = default.clone();
                Some(cx.new(|cx| InputState::new(window, cx).default_value(d)))
            }
            _ => None,
        };
        // Closing the window without answering drops this view, which cancels (see Drop).
        PromptWindow { title, kind, input, reply }
    }

    fn answer(&mut self, a: Result<Answer, PortError>, window: &mut Window) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(a);
        }
        window.remove_window();
    }
}

impl Drop for PromptWindow {
    fn drop(&mut self) {
        if let Some(r) = self.reply.take() {
            let _ = r.send(Err(PortError::Cancelled));
        }
    }
}

impl Render for PromptWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div().flex_1().flex().flex_col().gap(px(8.)).p(px(12.)).child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.title.clone()));
        let mut footer = div().flex().flex_row().gap(px(8.)).p(px(12.)).border_t_1().border_color(cx.theme().border).child(div().flex_1());
        match &self.kind {
            PromptKind::Text { .. } => {
                if let Some(i) = &self.input {
                    body = body.child(Input::new(i));
                }
                footer = footer
                    .child(Button::new("cancel").label("Cancel").on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))))
                    .child(Button::new("ok").label("OK").primary().on_click(cx.listener(|this, _, w, cx| {
                        let v = this.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
                        this.answer(Ok(Answer::Text(v)), w);
                    })));
            }
            PromptKind::Choose { options } => {
                let mut list = div().flex().flex_col().gap(px(4.));
                for (i, o) in options.iter().enumerate() {
                    list = list.child(Button::new(SharedString::from(format!("opt-{i}"))).label(o.clone()).w_full().on_click(cx.listener(
                        move |this, _, w, _| this.answer(Ok(Answer::Index(i)), w),
                    )));
                }
                body = body.child(div().flex_1().overflow_y_scrollbar().child(list));
                footer = footer.child(Button::new("cancel").label("Cancel").on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))));
            }
            PromptKind::Show { text } => {
                body = body.child(div().flex_1().overflow_y_scrollbar().p(px(6.)).rounded(px(4.)).bg(cx.theme().secondary).child(text.clone()));
                footer = footer.child(Button::new("ok").label("Close").primary().on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Ok), w))));
            }
            PromptKind::Confirm { text } => {
                body = body.child(widgets::muted(text.clone(), cx));
                footer = footer
                    .child(Button::new("no").label("No").on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Bool(false)), w))))
                    .child(Button::new("yes").label("Yes").primary().on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Bool(true)), w))));
            }
        }
        widgets::page(cx).child(body).child(footer)
    }
}

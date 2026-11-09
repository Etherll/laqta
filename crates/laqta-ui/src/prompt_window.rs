//! The `Prompt` port's dialogs: ask for text, choose from a list, show text, confirm. Each answers
//! once over a oneshot channel; closing the window answers `Cancelled`.

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds,
    WindowKind, WindowOptions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root, Selectable as _};
use laqta_ports::PortError;

use crate::widgets;

pub(crate) enum PromptKind {
    Text { default: String },
    Choose { options: Vec<String> },
    Show { text: String },
    Confirm { text: String },
    AfterCapture { image: Option<std::sync::Arc<gpui::RenderImage>>, after_capture: u32, after_upload: u32 },
    BeforeUpload { file_name: String, destinations: Vec<(String, String)>, current: String },
}

pub(crate) enum Answer {
    Text(String),
    Index(usize),
    Ok,
    Bool(bool),
    Flags(u32, u32),
}

pub(crate) type Reply = oneshot::Sender<Result<Answer, PortError>>;

pub(crate) fn open(title: String, kind: PromptKind, reply: Reply, cx: &mut App) {
    let (width, height) = match &kind {
        PromptKind::Choose { options } => (460.0, 120.0 + 36.0 * options.len().min(10) as f32),
        PromptKind::Show { .. } => (460.0, 360.0),
        PromptKind::AfterCapture { .. } => (820.0, 640.0),
        PromptKind::BeforeUpload { .. } => (560.0, 420.0),
        _ => (460.0, 170.0),
    };
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(width), px(height)), cx)),
        titlebar: Some(crate::chrome::titlebar(SharedString::from(title.clone()))),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
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
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut body = div()
            .flex_1()
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(12.))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child(self.title.clone()));
        let mut footer = div().flex().flex_row().gap(px(8.)).p(px(12.)).border_t_1().border_color(cx.theme().border).child(div().flex_1());
        match &self.kind {
            PromptKind::Text { .. } => {
                if let Some(i) = &self.input {
                    body = body.child(Input::new(i));
                }
                footer = footer
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))),
                    )
                    .child(Button::new("ok").label("OK").primary().on_click(cx.listener(|this, _, w, cx| {
                        let v = this.input.as_ref().map(|i| i.read(cx).value().to_string()).unwrap_or_default();
                        this.answer(Ok(Answer::Text(v)), w);
                    })));
            }
            PromptKind::Choose { options } => {
                let mut list = div().flex().flex_col().gap(px(4.));
                for (i, o) in options.iter().enumerate() {
                    list = list.child(
                        Button::new(SharedString::from(format!("opt-{i}")))
                            .label(o.clone())
                            .w_full()
                            .on_click(cx.listener(move |this, _, w, _| this.answer(Ok(Answer::Index(i)), w))),
                    );
                }
                body = body.child(div().flex_1().overflow_y_scrollbar().child(list));
                footer = footer.child(
                    Button::new("cancel").label("Cancel").on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))),
                );
            }
            PromptKind::Show { text } => {
                body = body.child(
                    div()
                        .flex_1()
                        .overflow_y_scrollbar()
                        .p(px(12.))
                        .rounded(crate::theme::radius::LG)
                        .bg(cx.theme().secondary)
                        .child(text.clone()),
                );
                footer = footer.child(
                    Button::new("ok").label("Close").primary().on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Ok), w))),
                );
            }
            PromptKind::AfterCapture { image, after_capture, after_upload } => {
                if let Some(img) = image {
                    body = body.child(
                        div().h(px(300.)).flex().justify_center().overflow_hidden().child(gpui::img(img.clone()).max_h_full().max_w_full()),
                    );
                }
                let (ac, au) = (*after_capture, *after_upload);
                let mut flags = div().flex().flex_wrap().gap(px(6.));
                for o in crate::presenters::settings::after_capture_options().into_iter().filter(|o| o.available) {
                    let bits = o.flag.bits();
                    flags = flags.child(
                        gpui_component::checkbox::Checkbox::new(SharedString::from(format!("acw-{bits}")))
                            .label(o.label)
                            .checked(ac & bits != 0)
                            .on_click(cx.listener(move |this, v: &bool, _, cx| {
                                if let PromptKind::AfterCapture { after_capture, .. } = &mut this.kind {
                                    if *v { *after_capture |= bits } else { *after_capture &= !bits }
                                }
                                cx.notify();
                            })),
                    );
                }
                let upload_bit = laqta_domain::workflow::AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST.bits();
                flags = flags.child(
                    gpui_component::checkbox::Checkbox::new("acw-upload")
                        .label("Upload image to host")
                        .checked(ac & upload_bit != 0)
                        .on_click(cx.listener(move |this, v: &bool, _, cx| {
                            if let PromptKind::AfterCapture { after_capture, .. } = &mut this.kind {
                                if *v { *after_capture |= upload_bit } else { *after_capture &= !upload_bit }
                            }
                            cx.notify();
                        })),
                );
                body = body.child(flags);
                footer = footer
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))),
                    )
                    .child(Button::new("ok").label("Continue").primary().on_click(cx.listener(move |this, _, w, _| {
                        let r = match &this.kind {
                            PromptKind::AfterCapture { after_capture, after_upload, .. } => (*after_capture, *after_upload),
                            _ => (ac, au),
                        };
                        this.answer(Ok(Answer::Flags(r.0, r.1)), w);
                    })));
            }
            PromptKind::BeforeUpload { file_name, destinations, current } => {
                body = body.child(widgets::muted(format!("Upload {file_name} to:"), cx));
                let mut list = div().flex().flex_col().gap(px(4.));
                for (i, (id, name)) in destinations.iter().enumerate() {
                    let id2 = id.clone();
                    list = list.child(
                        Button::new(SharedString::from(format!("bu-{i}"))).label(name.clone()).w_full().selected(id == current).on_click(
                            cx.listener(move |this, _, _, cx| {
                                if let PromptKind::BeforeUpload { current, .. } = &mut this.kind {
                                    *current = id2.clone();
                                }
                                cx.notify();
                            }),
                        ),
                    );
                }
                body = body.child(div().flex_1().overflow_y_scrollbar().child(list));
                footer = footer
                    .child(
                        Button::new("cancel")
                            .label("Cancel")
                            .on_click(cx.listener(|this, _, w, _| this.answer(Err(PortError::Cancelled), w))),
                    )
                    .child(Button::new("ok").label("Upload").primary().on_click(cx.listener(|this, _, w, _| {
                        let id = match &this.kind {
                            PromptKind::BeforeUpload { current, .. } => current.clone(),
                            _ => String::new(),
                        };
                        this.answer(Ok(Answer::Text(id)), w);
                    })));
            }
            PromptKind::Confirm { text } => {
                body = body.child(widgets::muted(text.clone(), cx));
                footer = footer
                    .child(Button::new("no").label("No").on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Bool(false)), w))))
                    .child(
                        Button::new("yes")
                            .label("Yes")
                            .primary()
                            .on_click(cx.listener(|this, _, w, _| this.answer(Ok(Answer::Bool(true)), w))),
                    );
            }
        }
        crate::chrome::rounded(widgets::page(cx), window).child(crate::chrome::TitleBar::new().title(self.title.clone()).flat()).child(body).child(footer)
    }
}

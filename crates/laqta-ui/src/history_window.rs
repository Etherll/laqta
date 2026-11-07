//! The history view: search, images-only filter, paging, and per-item actions. Reads history
//! through `laqta-library` on a background thread; rows come from the history presenter.

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Disableable as _, Root};

use crate::presenters::history::{self, HistoryRow, PAGE_SIZE};
use crate::runtime::{UiState, activate};
use crate::widgets;

pub(crate) fn open(images_only: bool, cx: &mut App) {
    if activate(cx.global::<UiState>().history, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(900.), px(680.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some("Laqta history".into()), ..Default::default() }),
        app_id: Some("laqta".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| HistoryWindow::new(images_only, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(handle) => cx.global_mut::<UiState>().history = Some(handle.into()),
        Err(e) => tracing::error!("history window failed: {e:#}"),
    }
}

struct HistoryWindow {
    search: Entity<InputState>,
    tags: Entity<InputState>,
    editing_tags: Option<i64>,
    grid: bool,
    stats: String,
    images_only: bool,
    page: u32,
    rows: Vec<HistoryRow>,
    error: Option<String>,
}

impl HistoryWindow {
    fn new(images_only: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search names, URLs and hosts; #tag filters by tag"));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("Tags, separated by commas"));
        cx.subscribe(&search, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.page = 0;
                this.reload(cx);
            }
        })
        .detach();
        let mut this = HistoryWindow {
            search,
            tags,
            editing_tags: None,
            grid: images_only,
            stats: String::new(),
            images_only,
            page: 0,
            rows: Vec::new(),
            error: None,
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = cx.global::<UiState>().services.history.clone() else {
            self.error = Some("History is turned off.".into());
            return;
        };
        let q = history::page_query(&self.search.read(cx).value(), self.images_only, self.page);
        let task = cx.background_spawn(async move {
            let rows = store.query(&q).map(|items| items.iter().map(history::row).collect::<Vec<_>>());
            let stats = store.stats().map(|s| history::stats_line(&s)).unwrap_or_default();
            rows.map(|r| (r, stats))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((rows, stats)) => {
                        this.rows = rows;
                        this.stats = stats;
                        this.error = None;
                    }
                    Err(e) => this.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn edit_tags(&mut self, id: i64, current: &[String], window: &mut Window, cx: &mut Context<Self>) {
        self.editing_tags = Some(id);
        let text = current.join(", ");
        self.tags.update(cx, |input, cx| input.set_value(text, window, cx));
        cx.notify();
    }

    fn save_tags(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.editing_tags.take() else { return };
        let tags = history::parse_tags(&self.tags.read(cx).value());
        if let Some(store) = cx.global::<UiState>().services.history.clone()
            && let Err(e) = store.set_tags(id, &tags)
        {
            self.error = Some(e.to_string());
        }
        self.reload(cx);
    }

    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        if let Some(store) = cx.global::<UiState>().services.history.clone()
            && let Err(e) = store.delete(id)
        {
            self.error = Some(e.to_string());
        }
        self.reload(cx);
    }
}

impl Render for HistoryWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.global::<UiState>().services.shell.clone();
        let toolbar = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .p(px(12.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(div().flex_1().child(Input::new(&self.search)))
            .child(Checkbox::new("grid").label("Grid").checked(self.grid).on_click(cx.listener(|this, checked: &bool, _, cx| {
                this.grid = *checked;
                cx.notify();
            })))
            .child(Checkbox::new("images-only").label("Images only").checked(self.images_only).on_click(cx.listener(
                |this, checked: &bool, _, cx| {
                    this.images_only = *checked;
                    this.page = 0;
                    this.reload(cx);
                },
            )));

        let mut list = div().flex().flex_col().gap(px(6.));
        if let Some(e) = &self.error {
            list = list.child(div().text_color(cx.theme().danger).child(e.clone()));
        }
        if self.rows.is_empty() && self.error.is_none() {
            list = list.child(widgets::muted("Nothing here yet.", cx));
        }
        if let Some(id) = self.editing_tags {
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .gap(px(8.))
                    .items_center()
                    .child(widgets::muted(format!("Tags for item {id}:"), cx))
                    .child(div().flex_1().child(Input::new(&self.tags)))
                    .child(Button::new("tags-save").label("Save tags").primary().on_click(cx.listener(|this, _, _, cx| this.save_tags(cx))))
                    .child(Button::new("tags-cancel").label("Cancel").ghost().on_click(cx.listener(|this, _, _, cx| {
                        this.editing_tags = None;
                        cx.notify();
                    }))),
            );
        }
        if self.grid {
            let mut grid = div().flex().flex_row().flex_wrap().gap(px(8.));
            for r in &self.rows {
                let mut card = div().w(px(180.)).flex().flex_col().gap(px(4.)).p(px(6.)).rounded(px(6.)).bg(cx.theme().secondary);
                let thumb = div().h(px(120.)).flex().justify_center().items_center().overflow_hidden();
                card = card.child(match &r.thumbnail {
                    Some(t) => thumb.child(img(t.clone()).max_h_full().max_w_full()),
                    None => thumb.child(widgets::muted(r.kind, cx)),
                });
                card = card.child(div().text_xs().truncate().child(r.title.clone())).child(widgets::muted(r.when.clone(), cx));
                if let Some(path) = r.path.clone() {
                    let s1 = shell.clone();
                    card = card.child(Button::new(SharedString::from(format!("g-open-{}", r.id))).label("Open").ghost().on_click(
                        move |_, _, _| {
                            let _ = s1.open_path(&path);
                        },
                    ));
                }
                grid = grid.child(card);
            }
            list = list.child(grid);
        }
        for r in self.rows.iter().filter(|_| !self.grid) {
            let id = r.id;
            let mut line = div().flex().flex_row().items_center().gap(px(10.)).p(px(6.)).rounded(px(6.)).bg(cx.theme().secondary);
            let thumb = div().w(px(96.)).h(px(64.)).flex().justify_center().items_center().overflow_hidden();
            line = line.child(match &r.thumbnail {
                Some(t) => thumb.child(img(t.clone()).max_h_full().max_w_full()),
                None => thumb.child(widgets::muted(r.kind, cx)),
            });
            let meta: Vec<String> = [r.when.clone(), r.size.clone(), r.dimensions.clone()].into_iter().filter(|s| !s.is_empty()).collect();
            let tags = if r.tags.is_empty() { String::new() } else { format!(" · #{}", r.tags.join(" #")) };
            line = line.child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(r.title.clone()))
                    .child(widgets::muted(format!("{}{tags}", meta.join(" · ")), cx)),
            );
            let current = r.tags.clone();
            line = line.child(
                Button::new(SharedString::from(format!("tags-{id}")))
                    .label("Tags")
                    .ghost()
                    .on_click(cx.listener(move |this, _, window, cx| this.edit_tags(id, &current, window, cx))),
            );
            if let Some(path) = r.path.clone() {
                let (s1, s2) = (shell.clone(), shell.clone());
                let (p1, p2, p3) = (path.clone(), path.clone(), path.clone());
                line = line
                    .child(Button::new(SharedString::from(format!("open-{id}"))).label("Open").ghost().on_click(move |_, _, _| {
                        let _ = s1.open_path(&p1);
                    }))
                    .child(Button::new(SharedString::from(format!("folder-{id}"))).label("Folder").ghost().on_click(move |_, _, _| {
                        let _ = s2.reveal(&p2);
                    }))
                    .child(Button::new(SharedString::from(format!("copy-{id}"))).label("Copy path").ghost().on_click(move |_, _, cx| {
                        cx.write_to_clipboard(gpui::ClipboardItem::new_string(p3.display().to_string()));
                    }));
            }
            if let Some(url) = r.url.clone() {
                let s = shell.clone();
                line =
                    line.child(Button::new(SharedString::from(format!("url-{id}"))).label("Open URL").ghost().on_click(move |_, _, _| {
                        let _ = s.open_url(&url);
                    }));
            }
            line = line.child(
                Button::new(SharedString::from(format!("del-{id}")))
                    .label("Remove")
                    .ghost()
                    .on_click(cx.listener(move |this, _, _, cx| this.delete(id, cx))),
            );
            list = list.child(line);
        }

        let more = self.rows.len() as u32 == PAGE_SIZE;
        let pager = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(px(8.))
            .p(px(12.))
            .border_t_1()
            .border_color(cx.theme().border)
            .child(Button::new("prev").label("Previous").disabled(self.page == 0).on_click(cx.listener(|this, _, _, cx| {
                this.page = this.page.saturating_sub(1);
                this.reload(cx);
            })))
            .child(widgets::muted(format!("Page {}", self.page + 1), cx))
            .child(div().flex_1())
            .child(widgets::muted(self.stats.clone(), cx))
            .child(Button::new("next").label("Next").disabled(!more).on_click(cx.listener(|this, _, _, cx| {
                this.page += 1;
                this.reload(cx);
            })));

        widgets::page(cx).child(toolbar).child(div().flex_1().p(px(12.)).overflow_y_scrollbar().child(list)).child(pager)
    }
}

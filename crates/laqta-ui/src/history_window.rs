//! The history view: search, images-only filter, paging, and per-item actions. Reads history
//! through `laqta-library` on a background thread; rows come from the history presenter.

use std::path::{Path, PathBuf};

use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, WindowOptions, div,
    img, prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::tooltip::Tooltip;
use gpui_component::{ActiveTheme as _, Root};

use crate::assets::IconName;
use crate::flash::{Flash, HasFlash};
use crate::presenters::history::{self, HistoryRow, PAGE_SIZE};
use crate::runtime::{UiState, activate};
use crate::theme::{self, radius, space, text};
use crate::thumbnails;
use crate::widgets::{self, Pill};

pub(crate) fn open(images_only: bool, cx: &mut App) {
    let open = if images_only { cx.global::<UiState>().image_history } else { cx.global::<UiState>().history };
    if activate(open, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(1080., 740., cx)),
        window_min_size: Some(size(px(640.), px(420.))),
        titlebar: Some(crate::chrome::titlebar(if images_only { "Laqta image history" } else { "Laqta history" })),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| HistoryWindow::new(images_only, window, cx));
        let focus = view.read(cx).focus.clone();
        let root = cx.new(|cx| Root::new(view, window, cx));
        // Focus the page after Root, which takes focus when it is built, so Tab reaches it.
        window.focus(&focus, cx);
        root
    }) {
        Ok(handle) => {
            let state = cx.global_mut::<UiState>();
            *(if images_only { &mut state.image_history } else { &mut state.history }) = Some(handle.into());
        }
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
    /// History is turned off in Settings: nothing to load, and nothing failed.
    off: bool,
    /// Whether the first query has returned, so an empty list before it reads "Loading…", not "No captures yet".
    loaded: bool,
    /// A short confirmation ("Copied", "Removed from history") shown under the title for a moment.
    flash: Flash,
    focus: FocusHandle,
}

impl HasFlash for HistoryWindow {
    fn flash(&mut self) -> &mut Flash {
        &mut self.flash
    }
}

impl HistoryWindow {
    fn new(images_only: bool, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search by name, link or #tag"));
        let tags = cx.new(|cx| InputState::new(window, cx).placeholder("Tags, separated by commas"));
        cx.subscribe(&search, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.page = 0;
                this.reload(cx);
            }
        })
        .detach();
        // Enter in the tags field saves them, as the Save tags button does.
        cx.subscribe(&tags, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::PressEnter { .. }) {
                this.save_tags(cx);
            }
        })
        .detach();
        crate::vanish::restore_on_activation(window, cx).detach();
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
            off: false,
            loaded: false,
            flash: Flash::default(),
            focus: cx.focus_handle(),
        };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = cx.global::<UiState>().services.history.clone() else {
            // Not a failure: the user turned history off. The body says so and links to the switch.
            self.off = true;
            self.loaded = true;
            return;
        };
        let q = history::page_query(&self.search.read(cx).value(), self.images_only, self.page);
        let task = cx.background_spawn(async move {
            let rows = store.query(&q).map(|items| items.iter().map(history::row).collect::<Vec<_>>());
            // A failed count leaves the list usable; the line says the totals are missing instead of going blank.
            let stats = store.stats().map(|s| history::stats_line(&s)).unwrap_or_else(|_| "Couldn't count the items".into());
            rows.map(|r| (r, stats))
        });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                this.loaded = true;
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
        // The write goes through the app (off the UI thread); the list reloads once it is in.
        let write = cx.global::<UiState>().services.app.history_set_tags(id, tags);
        let task = cx.background_spawn(async move { write.await.map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                if let Err(e) = result {
                    Flash::notice(this, crate::presenters::notice::Notice::failure_text("Couldn't save the tags", &e), cx);
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    /// Removes the history entry only; the file stays where it is.
    fn delete(&mut self, id: i64, cx: &mut Context<Self>) {
        let write = cx.global::<UiState>().services.app.history_delete(id);
        let task = cx.background_spawn(async move { write.await.map_err(|e| e.to_string()) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(()) => this.flash("Removed from history", cx),
                    Err(e) => {
                        Flash::notice(this, crate::presenters::notice::Notice::failure_text("Couldn't remove it from history", &e), cx)
                    }
                }
                this.reload(cx);
            });
        })
        .detach();
    }

    /// Shows `text` under the title for a moment.
    fn flash(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        Flash::show(self, text, cx);
    }

    /// Copies the image itself to the clipboard; other files copy their path. The file is read off
    /// the UI thread.
    fn copy_item(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        if !is_picture(&path) {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.display().to_string()));
            self.flash("Copied the file path", cx);
            return;
        }
        let app = cx.global::<UiState>().services.app.clone();
        cx.spawn(async move |this, cx| {
            let result = app.copy_image_file(path).await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(()) => this.flash("Copied", cx),
                Err(e) => {
                    Flash::notice(this, crate::presenters::notice::Notice::failure_text("Couldn't copy the picture", &e.to_string()), cx)
                }
            });
        })
        .detach();
    }
}

/// True for a file whose picture can be put on the clipboard.
pub(crate) fn is_picture(path: &Path) -> bool {
    let ext = path.extension().and_then(|ext| ext.to_str()).unwrap_or_default().to_ascii_lowercase();
    matches!(ext.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp" | "tif" | "tiff")
}

/// A 32px round ghost icon button with a tooltip. `danger` turns it red on hover.
fn icon_button(
    id: impl Into<SharedString>,
    glyph: IconName,
    label: &'static str,
    danger: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
    cx: &App,
) -> AnyElement {
    let t = cx.theme();
    let (hover_bg, hover_fg) = if danger { (t.danger.opacity(0.12), t.danger) } else { (theme::extra(cx).wash, t.foreground) };
    let el = div()
        .id(id.into())
        .size(px(32.))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .border_1()
        .border_color(gpui::transparent_black())
        .text_color(t.muted_foreground)
        .hover(move |s| s.bg(hover_bg).text_color(hover_fg))
        .child(gpui_component::Icon::new(glyph).size(px(16.)));
    widgets::clickable(el, on_click, cx).tooltip(move |window, cx| Tooltip::new(label).build(window, cx)).into_any_element()
}

impl Render for HistoryWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let shell = cx.global::<UiState>().services.shell.clone();
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        let wash = x.wash;

        let view_seg = |label: &'static str, grid: bool, this: &Self, cx: &mut Context<Self>| widgets::Segment {
            id: SharedString::from(format!("view-{label}")),
            label: label.into(),
            selected: this.grid == grid,
            on_click: Box::new(cx.listener(move |this, _, _, cx| {
                this.grid = grid;
                cx.notify();
            })),
        };
        let kind_seg = |label: &'static str, images: bool, this: &Self, cx: &mut Context<Self>| widgets::Segment {
            id: SharedString::from(format!("kind-{label}")),
            label: label.into(),
            selected: this.images_only == images,
            on_click: Box::new(cx.listener(move |this, _, _, cx| {
                this.images_only = images;
                this.page = 0;
                this.reload(cx);
            })),
        };
        let views = vec![view_seg("List", false, self, cx), view_seg("Grid", true, self, cx)];
        let kinds = vec![kind_seg("All", false, self, cx), kind_seg("Images", true, self, cx)];

        // The stats line doubles as the place a short confirmation appears; a failure gets its own
        // line below the title, in the danger tone with its reason and Dismiss.
        let failed = self.flash.current().is_some_and(|n| n.error);
        let subtitle: SharedString = match self.flash.current() {
            Some(n) if !n.error => n.title.clone().into(),
            _ => self.stats.clone().into(),
        };
        let failure = if failed { self.flash.render(cx.listener(|this, _, _, cx| Flash::dismiss(this, cx)), cx) } else { None };
        let header = div()
            .flex()
            .flex_col()
            .gap(space::S5)
            .px(space::S8)
            .pt(space::S4)
            .pb(space::S4)
            .child(widgets::page_title("History", subtitle, cx))
            .children(failure)
            // Search and filters have nothing to act on while history is off.
            .children((!self.off).then(|| {
                div()
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .max_w(px(480.))
                            .child(Input::new(&self.search).prefix(widgets::icon(IconName::Search, t.muted_foreground))),
                    )
                    .child(div().flex_1())
                    .child(widgets::segmented(kinds, cx))
                    .child(widgets::segmented(views, cx))
            }));

        let mut body = div().flex().flex_col().gap(space::S3);
        let query = self.search.read(cx).value().to_string();
        let state = if self.off {
            // Shown below instead of any list state.
            crate::presenters::library::ListState::Rows
        } else {
            crate::presenters::library::list_state(self.loaded, self.rows.len(), &query, self.error.as_deref(), None)
        };
        if self.off {
            let dispatch = cx.global::<UiState>().services.dispatch.clone();
            let off = widgets::empty_state(
                IconName::Database,
                "History is off",
                "Turn on Keep history in Settings → General. It starts with Laqta's next launch.",
                cx,
            )
            .child(div().pt(space::S3).child(widgets::pill(
                "history-open-settings",
                "Open settings",
                Pill::Default,
                false,
                move |_, _, _| dispatch(laqta_domain::Action::OpenSettings),
                cx,
            )));
            body = body.child(widgets::card(cx).child(off));
        }
        if !matches!(state, crate::presenters::library::ListState::Empty { .. } | crate::presenters::library::ListState::Rows) {
            let search = self.search.clone();
            let shown = widgets::list_state(
                &state,
                "No captures yet",
                cx.listener(move |_, _, window, cx| search.update(cx, |s, cx| s.set_value("", window, cx))),
                cx.listener(|this, _, _, cx| {
                    this.error = None;
                    this.reload(cx);
                }),
                cx,
            );
            body = body.children(shown.map(|s| widgets::card(cx).child(s)));
        }
        if matches!(state, crate::presenters::library::ListState::Empty { .. }) {
            let empty = {
                let dispatch = cx.global::<UiState>().services.dispatch.clone();
                widgets::empty_state(IconName::Images, "No captures yet", "Captures, recordings and uploads land here.", cx).child(
                    div().pt(space::S3).child(widgets::pill(
                        "empty-capture",
                        "Capture a region",
                        Pill::Primary,
                        false,
                        move |_, window, cx| {
                            crate::vanish::then_dispatch(laqta_domain::Action::RectangleRegion, dispatch.clone(), window, cx);
                        },
                        cx,
                    )),
                )
            };
            body = body.child(widgets::card(cx).child(empty));
        }
        if let Some(id) = self.editing_tags {
            let title = self.rows.iter().find(|r| r.id == id).map(|r| history::tags_title(&r.title)).unwrap_or_else(|| "Tags".into());
            body = body.child(
                widgets::card(cx)
                    .id("tags-editor")
                    // Escape in the tags field cancels the edit.
                    .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, _, cx| {
                        if ev.keystroke.key == "escape" {
                            this.editing_tags = None;
                            cx.notify();
                            cx.stop_propagation();
                        }
                    }))
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .p(space::S3)
                    .pl(space::S5)
                    .child(widgets::icon(IconName::Tag, t.muted_foreground))
                    .child(widgets::muted(title, cx))
                    .child(div().flex_1().child(Input::new(&self.tags)))
                    .child(widgets::pill(
                        "tags-cancel",
                        "Cancel",
                        Pill::Ghost,
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.editing_tags = None;
                            cx.notify();
                        }),
                        cx,
                    ))
                    .child(widgets::pill(
                        "tags-save",
                        "Save tags",
                        Pill::Dark,
                        false,
                        cx.listener(|this, _, _, cx| this.save_tags(cx)),
                        cx,
                    )),
            );
        }

        // A thumbnail with a hairline ring; `r` is the corner radius.
        let thumb = |row: &HistoryRow, image: Option<std::sync::Arc<gpui::RenderImage>>, w: Option<f32>, h: f32, r: gpui::Pixels| {
            let mut frame = div()
                .h(px(h))
                .rounded(r)
                .bg(t.muted)
                .border_1()
                .border_color(x.ring)
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center();
            frame = match w {
                Some(w) => frame.w(px(w)).flex_none(),
                None => frame.w_full(),
            };
            match image {
                Some(image) => frame.child(img(image).size_full().rounded(r).object_fit(gpui::ObjectFit::Cover)),
                None => frame.child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(row.kind)),
            }
        };

        // Rows only under the Rows state: an error replaces them, so stale rows never sit under it.
        let show_rows = matches!(state, crate::presenters::library::ListState::Rows) && !self.off;
        if self.grid && show_rows {
            let mut grid = div().grid().grid_cols(4).gap(space::S5);
            for r in &self.rows {
                let title = r.title.clone();
                let mut tile = div()
                    .id(SharedString::from(format!("g-{}", r.id)))
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .p(space::S1)
                    .rounded(radius::LG)
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .hover(move |s| s.bg(wash))
                    .child(thumb(r, r.thumbnail.as_deref().and_then(|p| thumbnails::get(p, cx)), None, 132., radius::MD))
                    .child(
                        div()
                            .px(space::S1)
                            .flex()
                            .flex_col()
                            .gap(px(1.))
                            .child(div().text_size(text::LABEL).font_weight(FontWeight::MEDIUM).child(r.when.clone()))
                            .child(
                                div().truncate().text_size(text::SMALL).text_color(t.muted_foreground).child(
                                    [r.dimensions.clone(), r.size.clone()]
                                        .into_iter()
                                        .filter(|s| !s.is_empty())
                                        .collect::<Vec<_>>()
                                        .join(" · "),
                                ),
                            ),
                    );
                if let Some(path) = r.path.clone() {
                    let s1 = shell.clone();
                    tile = widgets::clickable(
                        tile,
                        move |_, _, _| {
                            let _ = s1.open_path(&path);
                        },
                        cx,
                    );
                }
                // The full file name lives in a tooltip; the time is the readable title.
                grid = grid.child(tile.tooltip(move |window, cx| Tooltip::new(title.clone()).build(window, cx)));
            }
            body = body.child(grid);
        }

        if !self.grid && show_rows {
            let mut list = widgets::card(cx).p(space::S1).flex().flex_col().gap(px(2.));
            for r in &self.rows {
                let id = r.id;
                let meta: Vec<String> =
                    [r.when.clone(), r.size.clone(), r.dimensions.clone()].into_iter().filter(|s| !s.is_empty()).collect();
                let group = SharedString::from(format!("row-{id}"));
                // Copy, Copy link and Open stay visible; the icon actions appear while the row is hovered.
                let mut hover_actions = div()
                    .flex()
                    .items_center()
                    .gap(px(2.))
                    .opacity(crate::theme::QUIET_OPACITY)
                    .group_hover(group.clone(), |s| s.opacity(1.));
                let mut actions = div().flex().flex_none().items_center().gap(space::S1);
                let line = div()
                    .id(SharedString::from(format!("row-{id}")))
                    .group(group.clone())
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .min_h(px(60.))
                    .px(space::S2)
                    .py(px(10.))
                    .rounded(radius::XL)
                    .hover(move |s| s.bg(wash))
                    .child(thumb(r, r.thumbnail.as_deref().and_then(|p| thumbnails::get(p, cx)), Some(64.), 40., radius::SM))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .child(div().truncate().text_size(text::ROW).font_weight(FontWeight::MEDIUM).child(r.title.clone()))
                            .child(div().truncate().text_size(text::LABEL).text_color(t.muted_foreground).child(meta.join(" · ")))
                            .when(!r.tags.is_empty(), |d| {
                                let mut chips = div().flex().gap(space::S1).pt(px(2.));
                                for tag in &r.tags {
                                    chips = chips.child(widgets::badge(format!("#{tag}"), widgets::Tone::Neutral, cx));
                                }
                                d.child(chips)
                            }),
                    );
                if let Some(path) = r.path.clone() {
                    let (s1, s2) = (shell.clone(), shell.clone());
                    let (p1, p2, p3, p4) = (path.clone(), path.clone(), path.clone(), path);
                    hover_actions = hover_actions
                        .child(icon_button(
                            format!("folder-{id}"),
                            IconName::FolderOpen,
                            "Show in folder",
                            false,
                            move |_, _, _| {
                                let _ = s2.reveal(&p2);
                            },
                            cx,
                        ))
                        .child(icon_button(
                            format!("path-{id}"),
                            IconName::Clipboard,
                            "Copy file path",
                            false,
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(p3.display().to_string()));
                                this.flash("Copied the file path", cx);
                            }),
                            cx,
                        ));
                    actions = actions.child(widgets::pill(
                        SharedString::from(format!("copy-{id}")),
                        "Copy",
                        Pill::Default,
                        true,
                        cx.listener(move |this, _, _, cx| this.copy_item(p4.clone(), cx)),
                        cx,
                    ));
                    if let Some(url) = r.url.clone() {
                        actions = actions.child(widgets::pill(
                            SharedString::from(format!("link-{id}")),
                            "Copy link",
                            Pill::Default,
                            true,
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                                this.flash("Copied the link", cx);
                            }),
                            cx,
                        ));
                    }
                    actions = actions.child(widgets::pill(
                        SharedString::from(format!("open-{id}")),
                        "Open",
                        Pill::Default,
                        true,
                        move |_, _, _| {
                            let _ = s1.open_path(&p1);
                        },
                        cx,
                    ));
                } else if let Some(url) = r.url.clone() {
                    // An upload with no local file (text, URL): the link is the item.
                    let (u1, s) = (url.clone(), shell.clone());
                    actions = actions
                        .child(widgets::pill(
                            SharedString::from(format!("link-{id}")),
                            "Copy link",
                            Pill::Default,
                            true,
                            cx.listener(move |this, _, _, cx| {
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(u1.clone()));
                                this.flash("Copied the link", cx);
                            }),
                            cx,
                        ))
                        .child(widgets::pill(
                            SharedString::from(format!("url-{id}")),
                            "Open",
                            Pill::Default,
                            true,
                            move |_, _, _| {
                                let _ = s.open_url(&url);
                            },
                            cx,
                        ));
                }
                if let Some(short) = r.short_url.clone() {
                    actions = actions.child(widgets::pill(
                        SharedString::from(format!("short-{id}")),
                        "Copy short link",
                        Pill::Default,
                        true,
                        cx.listener(move |this, _, _, cx| {
                            cx.write_to_clipboard(gpui::ClipboardItem::new_string(short.clone()));
                            this.flash("Copied the short link", cx);
                        }),
                        cx,
                    ));
                }
                let current = r.tags.clone();
                hover_actions = hover_actions
                    .child(icon_button(
                        format!("tags-{id}"),
                        IconName::Tag,
                        "Edit tags",
                        false,
                        cx.listener(move |this, _, window, cx| this.edit_tags(id, &current, window, cx)),
                        cx,
                    ))
                    .child(icon_button(
                        format!("del-{id}"),
                        IconName::Trash,
                        "Remove from history (keeps the file)",
                        true,
                        cx.listener(move |this, _, _, cx| this.delete(id, cx)),
                        cx,
                    ));
                list = list.child(line.child(hover_actions).child(actions));
            }
            body = body.child(list);
        }

        // Paging: hidden when everything fits on the first page.
        let more = self.rows.len() as u32 == PAGE_SIZE;
        let first = self.page == 0;
        let pager = (!first || more).then(|| {
            let (can_prev, can_next) = history::page_reason(first, more);
            let prev = widgets::icon_button_with(
                "prev",
                IconName::ChevronLeft,
                "Previous page",
                false,
                can_prev.map_err(SharedString::from),
                cx.listener(|this, _, _, cx| {
                    this.page = this.page.saturating_sub(1);
                    this.reload(cx);
                }),
                cx,
            );
            let next = widgets::icon_button_with(
                "next",
                IconName::ChevronRight,
                "Next page",
                false,
                can_next.map_err(SharedString::from),
                cx.listener(|this, _, _, cx| {
                    this.page += 1;
                    this.reload(cx);
                }),
                cx,
            );
            div()
                .flex()
                .items_center()
                .justify_end()
                .gap(space::S1)
                .px(space::S8)
                .py(space::S2)
                .border_t_1()
                .border_color(t.border)
                .child(div().pr(space::S2).text_size(text::LABEL).text_color(t.muted_foreground).child(format!("Page {}", self.page + 1)))
                .child(prev)
                .child(next)
        });

        widgets::tab_keys(crate::chrome::rounded(widgets::page(cx), window), &self.focus)
            .child(crate::chrome::TitleBar::new().flat())
            .child(header)
            .child(div().id("history-scroll").flex_1().overflow_y_scrollbar().child(div().px(space::S8).pb(space::S6).child(body)))
            .children(pager)
    }
}

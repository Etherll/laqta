//! The main window: a sidebar, the quick capture bar, running tasks and the library of recent
//! captures. Opened on demand and dropped on close (ARCHITECTURE §1.4).

use gpui::{
    AnyElement, App, AppContext as _, Context, Div, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement,
    ParentElement as _, Render, SharedString, StatefulInteractiveElement as _, Styled as _, StyledImage as _, Window, WindowBounds,
    WindowOptions, div, img, prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::Action;
use laqta_domain::hotkey::KeyChord;

use crate::assets::IconName;
use crate::chrome;
use crate::presenters::after_capture;
use crate::presenters::hotkeys::hotkey_label;
use crate::presenters::library::{self, CardGlyph, LibraryCard, LibraryTab};
use crate::presenters::task_list::{StatusTone, TaskList, status_label, status_tone};
use crate::runtime::{UiState, activate};
use crate::theme::{self, radius, space, text};
use crate::widgets::{self, Pill, Tone};

/// How many items the library shows; "See all" opens History for the rest.
const LIBRARY_LIMIT: u32 = 24;

pub(crate) fn open(cx: &mut App) {
    if activate(cx.global::<UiState>().main, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1180.), px(760.)), cx)),
        titlebar: Some(crate::chrome::titlebar("Laqta")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some("laqta".into()),
        window_min_size: Some(size(px(720.), px(480.))),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| MainWindow::new(window, cx));
        let focus = view.read(cx).focus.clone();
        let root = cx.new(|cx| Root::new(view, window, cx));
        // Focus the page after Root, which takes focus when it is built, so Tab reaches it.
        window.focus(&focus, cx);
        root
    }) {
        Ok(handle) => cx.global_mut::<UiState>().main = Some(handle.into()),
        Err(e) => tracing::error!("main window failed: {e:#}"),
    }
}

struct MainWindow {
    tasks: Entity<TaskList>,
    library: Vec<LibraryCard>,
    tab: LibraryTab,
    grid: bool,
    search: Entity<InputState>,
    /// A short confirmation, such as "Copied", shown in the library header for a moment.
    notice: Option<SharedString>,
    focus: FocusHandle,
}

impl MainWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let tasks = cx.global::<UiState>().tasks.clone();
        // Repaint on task events and reload the library (a small indexed query).
        cx.observe(&tasks, |this, _, cx| {
            this.reload(cx);
            cx.notify();
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search"));
        cx.subscribe(&search, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.reload(cx);
            }
        })
        .detach();
        let focus = cx.focus_handle();
        let mut this = MainWindow { tasks, library: Vec::new(), tab: LibraryTab::Recent, grid: true, search, notice: None, focus };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let services = &cx.global::<UiState>().services;
        let Some(history) = services.history.clone() else { return };
        let now = services.clock.now();
        let tab = self.tab;
        let q = library::query(tab, &self.search.read(cx).value(), LIBRARY_LIMIT);
        let task = cx.background_spawn(async move {
            history.query(&q).map(|items| {
                items
                    .iter()
                    .filter(|i| library::keep(tab, i))
                    .take(LIBRARY_LIMIT as usize)
                    .map(|i| library::card(i, now))
                    .collect::<Vec<_>>()
            })
        });
        cx.spawn(async move |this, cx| {
            if let Ok(cards) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.library = cards;
                    cx.notify();
                });
            }
        })
        .detach();
    }

    /// Shows `text` in the library header for a moment.
    fn flash(&mut self, text: &'static str, cx: &mut Context<Self>) {
        self.notice = Some(text.into());
        cx.notify();
        cx.spawn(async move |this, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(2000)).await;
            let _ = this.update(cx, |this, cx| {
                this.notice = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// Copies an image to the clipboard (read off the UI thread); other files copy their path.
    fn copy_item(&mut self, path: std::path::PathBuf, cx: &mut Context<Self>) {
        let Some(format) = crate::history_window::clipboard_format(&path) else {
            cx.write_to_clipboard(gpui::ClipboardItem::new_string(path.display().to_string()));
            self.flash("Copied the file path", cx);
            return;
        };
        let task = cx.background_spawn(async move { std::fs::read(&path) });
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| match result {
                Ok(bytes) => {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_image(&gpui::Image::from_bytes(format, bytes)));
                    this.flash("Copied", cx);
                }
                Err(e) => tracing::warn!("copy: {e}"),
            });
        })
        .detach();
    }
}

/// Actions that select something on screen: the main window minimizes first so it stays out of
/// the capture.
fn hides_window(action: Action) -> bool {
    matches!(
        action,
        Action::PrintScreen
            | Action::RectangleRegion
            | Action::ActiveWindow
            | Action::ActiveMonitor
            | Action::LastRegion
            | Action::ScrollingCapture
            | Action::ScreenRecorder
            | Action::ScreenRecorderGif
    )
}

fn on_dispatch(action: Action, cx: &App) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let dispatch = cx.global::<UiState>().services.dispatch.clone();
    move |_, window, _| {
        if hides_window(action) {
            window.minimize_window();
        }
        dispatch(action);
    }
}

fn open_path(path: std::path::PathBuf, reveal: bool, cx: &App) -> impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static {
    let shell = cx.global::<UiState>().services.shell.clone();
    move |_, _, _| {
        let r = if reveal { shell.reveal(&path) } else { shell.open_path(&path) };
        if let Err(e) = r {
            tracing::warn!("open {}: {e}", path.display());
        }
    }
}

fn sidebar(cx: &App) -> Div {
    let t = cx.theme();
    let hover = t.secondary_hover;
    let new_capture = widgets::clickable(
        div()
            .id("new-capture")
            .h(theme::CONTROL_H)
            .px(space::S4)
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(space::S2)
            .rounded(radius::PILL)
            .border_1()
            .border_color(gpui::transparent_black())
            .bg(t.secondary)
            .text_color(t.secondary_foreground)
            .font_weight(FontWeight::MEDIUM)
            .text_size(text::LABEL)
            .hover(move |s| s.bg(hover))
            .child(widgets::icon(IconName::Plus, t.secondary_foreground))
            .child("New capture"),
        on_dispatch(Action::RectangleRegion, cx),
        cx,
    );

    let nav = |id: &'static str, glyph, label: &'static str, action: Action| {
        widgets::nav_row(id, glyph, label, false, on_dispatch(action, cx), cx)
    };

    div()
        .w(theme::SIDEBAR_W)
        .h_full()
        .flex_none()
        .flex()
        .flex_col()
        .gap(px(2.))
        .p(space::S3)
        .pt(space::S1)
        .child(new_capture)
        .child(div().h(space::S3))
        .child(widgets::nav_row("nav-home", IconName::House, "Home", true, |_, _, _| {}, cx))
        .child(nav("nav-history", IconName::LayoutGrid, "History", Action::OpenHistory))
        .child(nav("nav-folder", IconName::FolderOpen, "Screenshots", Action::OpenScreenshotsFolder))
        .child(widgets::nav_group("Edit", cx))
        .child(nav("nav-editor", IconName::Image, "Image editor", Action::ImageEditor))
        .child(nav("nav-effects", IconName::Sparkles, "Effects", Action::ImageEffects))
        .child(nav("nav-picker", IconName::Pipette, "Color picker", Action::ColorPicker))
        .child(div().flex_1())
        .child(nav("nav-settings", IconName::Settings, "Settings", Action::OpenSettings))
}

struct CaptureCard {
    id: &'static str,
    glyph: IconName,
    title: &'static str,
    hint: &'static str,
    action: Action,
}

const CAPTURES: [CaptureCard; 8] = [
    CaptureCard {
        id: "cap-region",
        glyph: IconName::Crop,
        title: "Region",
        hint: "Drag to select an area",
        action: Action::RectangleRegion,
    },
    CaptureCard {
        id: "cap-screen",
        glyph: IconName::Monitor,
        title: "Full screen",
        hint: "Every monitor at once",
        action: Action::PrintScreen,
    },
    CaptureCard { id: "cap-window", glyph: IconName::AppWindow, title: "Window", hint: "The active window", action: Action::ActiveWindow },
    CaptureCard {
        id: "cap-last",
        glyph: IconName::SquareDashed,
        title: "Last region",
        hint: "Repeat the previous area",
        action: Action::LastRegion,
    },
    CaptureCard {
        id: "cap-monitor",
        glyph: IconName::SquareDashedMousePointer,
        title: "Monitor",
        hint: "The monitor under the cursor",
        action: Action::ActiveMonitor,
    },
    CaptureCard {
        id: "cap-scroll",
        glyph: IconName::ScanLine,
        title: "Scrolling",
        hint: "A long page, stitched",
        action: Action::ScrollingCapture,
    },
    CaptureCard {
        id: "rec-video",
        glyph: IconName::Video,
        title: "Record video",
        hint: "Pick an area, then record",
        action: Action::ScreenRecorder,
    },
    CaptureCard {
        id: "rec-gif",
        glyph: IconName::Film,
        title: "Record GIF",
        hint: "Pick an area, then record",
        action: Action::ScreenRecorderGif,
    },
];

fn capture_card(c: &CaptureCard, featured: bool, hotkeys: &[(KeyChord, Action)], cx: &App) -> AnyElement {
    let t = cx.theme();
    let wash = theme::extra(cx).wash;
    // A 32px tile: the shared 36px tile, shrunk for these compact cards.
    let tile = widgets::icon_tile(c.glyph, featured, cx).size(px(32.)).rounded(radius::SM + px(2.));
    let mut top = div().flex().items_start().justify_between().child(tile);
    if let Some(keys) = hotkey_label(hotkeys, c.action) {
        top = top.child(widgets::kbd(keys, cx));
    }
    let el = widgets::card(cx).id(c.id).flex().flex_col().gap(space::S3).p(space::S4).hover(move |s| s.bg(wash)).child(top).child(
        div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(div().text_size(text::BODY).font_weight(FontWeight::MEDIUM).child(c.title))
            .child(div().truncate().text_size(text::LABEL).text_color(t.muted_foreground).child(c.hint)),
    );
    widgets::clickable(el, on_dispatch(c.action, cx), cx).into_any_element()
}

fn tone(status: laqta_ports::TaskStatus) -> Tone {
    match status_tone(status) {
        StatusTone::Neutral => Tone::Neutral,
        StatusTone::Busy => Tone::Busy,
        StatusTone::Success => Tone::Success,
        StatusTone::Danger => Tone::Danger,
    }
}

/// The card surface of the library: a step above the page.
fn library_surface(cx: &App) -> gpui::Hsla {
    if cx.theme().is_dark() { gpui::rgb(0x232323).into() } else { theme::extra(cx).card }
}

/// The icon for an item with no thumbnail.
fn glyph_icon(g: CardGlyph) -> IconName {
    match g {
        CardGlyph::Image => IconName::Image,
        CardGlyph::Text => IconName::Type,
        CardGlyph::File => IconName::File,
        CardGlyph::Link => IconName::Link,
    }
}

/// A round icon button that sits on a thumbnail: dark glass, white glyph.
fn overlay_button(
    id: SharedString,
    glyph: IconName,
    tip: &'static str,
    on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    div()
        .id(id)
        .size(px(30.))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .bg(gpui::black().opacity(0.62))
        .hover(|s| s.bg(gpui::black().opacity(0.8)))
        .cursor_pointer()
        .on_click(move |e, w, cx| {
            cx.stop_propagation();
            on_click(e, w, cx)
        })
        .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
        .child(widgets::icon(glyph, gpui::white()).size(px(14.)))
        .into_any_element()
}

impl MainWindow {
    /// Tasks still running, or that failed; finished ones show up in the library instead.
    fn activity(&self, cx: &App) -> Option<Div> {
        let t = cx.theme();
        let rows: Vec<_> = self.tasks.read(cx).rows().iter().filter(|r| status_tone(r.status) != StatusTone::Success).cloned().collect();
        if rows.is_empty() {
            return None;
        }
        let mut list = widgets::card(cx).p(space::S1).flex().flex_col().gap(px(2.));
        for (i, r) in rows.iter().enumerate() {
            let label = status_label(r.status);
            let detail = r.error.clone().unwrap_or_else(|| r.detail.clone());
            let mut text_col = div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(r.title.clone()));
            // The badge already says the status; repeat the detail only when it adds something.
            if detail != label && !detail.is_empty() {
                let color = if r.error.is_some() { t.danger } else { t.muted_foreground };
                text_col = text_col.child(div().truncate().text_size(text::CAPTION).text_color(color).child(detail));
            }
            list = list.child(
                div()
                    .id(SharedString::from(format!("task-{i}")))
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .px(space::S4)
                    .py(space::S3)
                    .child(text_col)
                    .child(widgets::badge(label, tone(r.status), cx)),
            );
        }
        Some(div().flex().flex_col().child(widgets::group_title("In progress", cx).pb(space::S3)).child(list))
    }

    fn library_header(&self, cx: &mut Context<Self>) -> Div {
        let t = cx.theme().clone();
        let wash = theme::extra(cx).wash;
        let view_button = |id: &'static str, glyph: IconName, grid: bool, tip: &'static str, this: &Self, cx: &mut Context<Self>| {
            let on = this.grid == grid;
            div()
                .id(id)
                .size(px(32.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::PILL)
                .when(on, |d| d.bg(t.accent))
                .hover(move |s| s.bg(wash))
                .cursor_pointer()
                .tooltip(move |window, cx| gpui_component::tooltip::Tooltip::new(tip).build(window, cx))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.grid = grid;
                    cx.notify();
                }))
                .child(widgets::icon(glyph, if on { t.foreground } else { t.muted_foreground }))
        };
        let mut tabs = div().flex().items_center().gap(px(2.));
        let fg = t.foreground;
        for tab in LibraryTab::ALL {
            let selected = self.tab == tab;
            tabs = tabs.child(widgets::clickable(
                div()
                    .id(SharedString::from(format!("lib-tab-{}", tab.label())))
                    .h(px(34.))
                    .px(space::S4)
                    .flex()
                    .items_center()
                    .rounded(radius::PILL)
                    .border_1()
                    .border_color(gpui::transparent_black())
                    .text_size(px(14.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(if selected { t.foreground } else { t.muted_foreground })
                    .when(selected, |d| d.bg(t.accent))
                    .hover(move |s| s.text_color(fg))
                    .child(tab.label()),
                cx.listener(move |this, _, _, cx| {
                    this.tab = tab;
                    this.reload(cx);
                }),
                cx,
            ));
        }
        let see_all = widgets::pill("see-all", "See all", Pill::Ghost, false, on_dispatch(Action::OpenHistory, cx), cx);
        div()
            .flex()
            .flex_col()
            .gap(space::S4)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(div().font_family(text::HEADING_FONT).font_weight(FontWeight::MEDIUM).text_size(px(24.)).child("Library"))
                    .children(self.notice.clone().map(|n| div().pl(space::S3).text_size(text::LABEL).text_color(t.primary).child(n)))
                    .child(div().flex_1())
                    .child(div().w(px(220.)).child(Input::new(&self.search).prefix(widgets::icon(IconName::Search, t.muted_foreground))))
                    .child(view_button("view-grid", IconName::LayoutGrid, true, "Grid", self, cx))
                    .child(view_button("view-list", IconName::List, false, "List", self, cx))
                    .child(see_all),
            )
            .child(tabs)
    }

    fn library(&self, cols: u16, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        if self.library.is_empty() {
            let (title, body) = if self.search.read(cx).value().is_empty() {
                self.tab.empty_text()
            } else {
                ("Nothing matches", "Try another name, or a #tag.")
            };
            return widgets::card(cx).child(widgets::empty_state(IconName::Images, title, body, cx)).into_any_element();
        }
        let surface = library_surface(cx);
        let wash = theme::extra(cx).wash;
        if !self.grid {
            let mut list = div().flex().flex_col().gap(px(2.)).p(space::S1).rounded(radius::CARD).bg(surface);
            for c in &self.library {
                list = list.child(self.list_row(c, cx));
            }
            return list.into_any_element();
        }
        let mut grid = div().grid().grid_cols(cols).gap(space::S4);
        for c in &self.library {
            let group = SharedString::from(format!("lib-{}", c.id));
            let thumb = div()
                .relative()
                .w_full()
                .h(px(150.))
                .rounded(radius::LG)
                .overflow_hidden()
                .flex()
                .items_center()
                .justify_center()
                .when(c.thumbnail.is_none(), |d| d.bg(t.foreground.opacity(0.04)))
                .child(match &c.thumbnail {
                    Some(path) => img(path.clone()).size_full().rounded(radius::LG).object_fit(gpui::ObjectFit::Cover).into_any_element(),
                    None => widgets::icon(glyph_icon(c.glyph), t.muted_foreground).size(px(28.)).into_any_element(),
                })
                .child(self.overlay_actions(c, &group, cx));
            let mut card = div()
                .id(SharedString::from(format!("lib-card-{}", c.id)))
                .group(group.clone())
                .flex()
                .flex_col()
                .p(space::S2)
                .rounded(radius::CARD)
                .bg(surface)
                .border_1()
                .border_color(gpui::transparent_black())
                .hover(move |s| s.bg(surface.blend(wash)))
                .child(thumb)
                .child(
                    div()
                        .px(space::S2)
                        .pt(px(10.))
                        .pb(space::S1)
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                        .child(div().truncate().text_size(px(12.)).text_color(t.muted_foreground).child(when_meta(c))),
                );
            if let Some(open) = open_card(c, cx) {
                card = widgets::clickable(card, open, cx);
            }
            grid = grid.child(card);
        }
        grid.into_any_element()
    }

    /// Hover actions in the corner of a thumbnail.
    fn overlay_actions(&self, c: &LibraryCard, group: &SharedString, cx: &mut Context<Self>) -> Div {
        let mut bar = div()
            .absolute()
            .top(space::S2)
            .right(space::S2)
            .flex()
            .gap(space::S1)
            .opacity(0.)
            .group_hover(group.clone(), |s| s.opacity(1.));
        if let Some(path) = c.path.clone() {
            let p = path.clone();
            bar = bar.child(overlay_button(
                SharedString::from(format!("lib-copy-{}", c.id)),
                IconName::Copy,
                if c.is_image { "Copy image" } else { "Copy path" },
                cx.listener(move |this, _, _, cx| this.copy_item(p.clone(), cx)),
            ));
            bar = bar.child(overlay_button(
                SharedString::from(format!("lib-folder-{}", c.id)),
                IconName::FolderOpen,
                "Show in folder",
                open_path(path, true, cx),
            ));
        }
        if let Some(url) = c.url.clone() {
            bar = bar.child(overlay_button(
                SharedString::from(format!("lib-link-{}", c.id)),
                IconName::Link,
                "Copy link",
                cx.listener(move |this, _, _, cx| {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(url.clone()));
                    this.flash("Copied the link", cx);
                }),
            ));
        }
        bar
    }

    fn list_row(&self, c: &LibraryCard, cx: &mut Context<Self>) -> AnyElement {
        let t = cx.theme().clone();
        let wash = theme::extra(cx).wash;
        let group = SharedString::from(format!("lib-row-{}", c.id));
        let thumb = div()
            .w(px(64.))
            .h(px(40.))
            .flex_none()
            .rounded(radius::SM)
            .overflow_hidden()
            .bg(t.foreground.opacity(0.04))
            .flex()
            .items_center()
            .justify_center()
            .child(match &c.thumbnail {
                Some(path) => img(path.clone()).size_full().rounded(radius::SM).object_fit(gpui::ObjectFit::Cover).into_any_element(),
                None => widgets::icon(glyph_icon(c.glyph), t.muted_foreground).into_any_element(),
            });
        let mut row = div()
            .id(SharedString::from(format!("lib-li-{}", c.id)))
            .group(group.clone())
            .relative()
            .flex()
            .items_center()
            .gap(space::S3)
            .p(space::S2)
            .pr(space::S3)
            .rounded(radius::XL)
            .border_1()
            .border_color(gpui::transparent_black())
            .hover(move |s| s.bg(wash))
            .child(thumb)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap(px(2.))
                    .child(div().truncate().text_size(px(14.)).font_weight(FontWeight::MEDIUM).child(c.name.clone()))
                    .child(div().truncate().text_size(px(12.)).text_color(t.muted_foreground).child(when_meta(c))),
            )
            .child(self.overlay_actions(c, &group, cx).relative().top(px(0.)).right(px(0.)));
        if let Some(open) = open_card(c, cx) {
            row = widgets::clickable(row, open, cx);
        }
        row.into_any_element()
    }
}

/// "Today 21:35 · 1192×488 · 43.0 KB".
fn when_meta(c: &LibraryCard) -> String {
    if c.meta.is_empty() { c.when.clone() } else { format!("{} · {}", c.when, c.meta) }
}

/// Clicking a card opens the file, or the link for link-only items.
fn open_card(c: &LibraryCard, cx: &App) -> Option<widgets::OnClick> {
    if let Some(path) = c.path.clone() {
        return Some(Box::new(open_path(path, false, cx)));
    }
    let url = c.url.clone()?;
    let shell = cx.global::<UiState>().services.shell.clone();
    Some(Box::new(move |_, _, _| {
        let _ = shell.open_url(&url);
    }))
}

impl Render for MainWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        // Content width decides the grid columns, so a narrow window reflows instead of squeezing.
        let content_w = f32::from(window.viewport_size().width) - f32::from(theme::SIDEBAR_W) - 2. * f32::from(space::S10);
        let capture_cols = if content_w >= 700. { 4 } else { 2 };
        // Library cards are at least 200px wide.
        let library_cols = ((content_w.min(1040.) + 16.) / 216.).floor().clamp(2., 5.) as u16;

        let settings = cx.global::<UiState>().services.app.settings();
        let hotkeys = settings.hotkey_bindings();
        // Where captures go and what runs after them, with a way to change it.
        let home = std::env::var_os("HOME").map(std::path::PathBuf::from);
        let status = div()
            .pt(space::S3)
            .flex()
            .items_center()
            .gap(space::S2)
            .text_size(text::LABEL)
            .text_color(t.muted_foreground)
            .child(div().flex_none().font_weight(FontWeight::MEDIUM).text_color(t.foreground).child("After capture"))
            .child(div().min_w_0().truncate().child(after_capture::summary(&settings, home.as_deref())))
            .child(widgets::pill("after-change", "Change", Pill::Ghost, true, on_dispatch(Action::OpenSettings, cx), cx));
        let mut grid = div().grid().grid_cols(capture_cols).gap(space::S3);
        for (i, c) in CAPTURES.iter().enumerate() {
            grid = grid.child(capture_card(c, i == 0, &hotkeys, cx));
        }

        // Upload strip: a drop target with two one-line actions. Light: white pills on a grey strip;
        // dark: the usual grey pills on a ring-only strip.
        let (strip_bg, pill_bg) = if t.is_dark() { (gpui::transparent_black(), t.secondary) } else { (t.secondary, x.card) };
        let upload = div()
            .flex()
            .items_center()
            .gap(space::S3)
            .pl(space::S5)
            .pr(px(10.))
            .h(px(56.))
            .rounded(radius::CARD)
            .bg(strip_bg)
            .border_1()
            .border_color(x.ring)
            .child(widgets::icon(IconName::CloudUpload, t.muted_foreground))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .truncate()
                    .text_size(text::LABEL)
                    .text_color(t.muted_foreground)
                    .child("Drop files anywhere in this window to upload them"),
            )
            .child(widgets::pill("up-file", "Upload file", Pill::Default, false, on_dispatch(Action::FileUpload, cx), cx).bg(pill_bg))
            .child(
                widgets::pill("up-clip", "From clipboard", Pill::Default, false, on_dispatch(Action::ClipboardUpload, cx), cx).bg(pill_bg),
            );

        let activity = self.activity(cx);
        let header = self.library_header(cx);
        let library = self.library(library_cols, cx);
        let lower = div()
            .flex()
            .flex_col()
            .gap(space::S10)
            .children(activity)
            .child(div().flex().flex_col().gap(space::S5).child(header).child(library));

        let content = div().id("main-content").flex_1().min_w_0().h_full().overflow_y_scrollbar().child(
            div()
                .max_w(px(1120.))
                .mx_auto()
                .px(space::S10)
                .pt(space::S8)
                .pb(space::S10)
                .flex()
                .flex_col()
                .child(widgets::page_title("Capture", "Grab a region, a window or the whole screen. Every capture lands in History.", cx))
                .child(status)
                .child(div().pt(space::S8).child(grid))
                .child(div().pt(space::S4).child(upload))
                .child(div().pt(space::S10).child(lower)),
        );

        let app = cx.global::<UiState>().services.app.clone();
        let drop_tint = t.drop_target;
        widgets::tab_keys(crate::chrome::rounded(widgets::page(cx), window).bg(t.sidebar), &self.focus)
            .text_size(text::BODY)
            .id("main-drop")
            .drag_over::<gpui::ExternalPaths>(move |style, _, _, _| style.bg(drop_tint))
            .on_drop(move |paths: &gpui::ExternalPaths, _, _| {
                let ids = app.upload_files(paths.paths().to_vec(), None);
                tracing::info!(count = ids.len(), "dropped files submitted for upload");
            })
            .child(chrome::TitleBar::new().leading(chrome::wordmark(), theme::SIDEBAR_W))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sidebar(cx))
                    .child(chrome::sheet(window, cx).flex_1().min_w_0().h_full().flex().child(content)),
            )
    }
}

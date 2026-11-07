//! The main window: quick capture buttons, the task list and recent captures. Opened on demand
//! and dropped on close (ARCHITECTURE §1.4).

use gpui::{
    App, AppContext as _, InteractiveElement as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::Action;

use crate::presenters::history::{self, HistoryRow};
use crate::presenters::task_list::{TaskList, status_label};
use crate::runtime::{UiState, activate};
use crate::widgets;

const RECENT: u32 = 12;

pub(crate) fn open(cx: &mut App) {
    if activate(cx.global::<UiState>().main, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(980.), px(660.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some("Laqta".into()), ..Default::default() }),
        app_id: Some("laqta".into()),
        window_min_size: Some(size(px(640.), px(420.))),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(MainWindow::new);
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(handle) => cx.global_mut::<UiState>().main = Some(handle.into()),
        Err(e) => tracing::error!("main window failed: {e:#}"),
    }
}

struct MainWindow {
    tasks: Entity<TaskList>,
    recent: Vec<HistoryRow>,
}

impl MainWindow {
    fn new(cx: &mut Context<Self>) -> Self {
        let tasks = cx.global::<UiState>().tasks.clone();
        // Repaint on task events and reload recent captures (a small indexed query).
        cx.observe(&tasks, |this, _, cx| {
            this.reload(cx);
            cx.notify();
        })
        .detach();
        let mut this = MainWindow { tasks, recent: Vec::new() };
        this.reload(cx);
        this
    }

    fn reload(&mut self, cx: &mut Context<Self>) {
        let Some(history) = cx.global::<UiState>().services.history.clone() else { return };
        let task = cx.background_spawn(async move {
            let q = laqta_domain::history::HistoryQuery { limit: RECENT, ..Default::default() };
            history.query(&q).map(|items| items.iter().map(history::row).collect::<Vec<_>>())
        });
        cx.spawn(async move |this, cx| {
            if let Ok(rows) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.recent = rows;
                    cx.notify();
                });
            }
        })
        .detach();
    }
}

fn dispatch_button(id: &'static str, label: &'static str, action: Action, cx: &App) -> Button {
    let dispatch = cx.global::<UiState>().services.dispatch.clone();
    Button::new(id).label(label).on_click(move |_, window, _| {
        // Keep the main window out of the capture.
        if matches!(
            action,
            Action::PrintScreen | Action::RectangleRegion | Action::ActiveWindow | Action::ActiveMonitor | Action::LastRegion
        ) {
            window.minimize_window();
        }
        dispatch(action);
    })
}

fn open_path_button(id: SharedString, label: &'static str, path: std::path::PathBuf, reveal: bool, cx: &App) -> Button {
    let shell = cx.global::<UiState>().services.shell.clone();
    Button::new(id).label(label).ghost().on_click(move |_, _, _| {
        let r = if reveal { shell.reveal(&path) } else { shell.open_path(&path) };
        if let Err(e) = r {
            tracing::warn!("open {}: {e}", path.display());
        }
    })
}

impl Render for MainWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let header = div()
            .flex()
            .flex_row()
            .flex_wrap()
            .gap(px(8.))
            .p(px(12.))
            .border_b_1()
            .border_color(cx.theme().border)
            .child(dispatch_button("cap-region", "Capture region", Action::RectangleRegion, cx).primary())
            .child(dispatch_button("cap-screen", "Capture screen", Action::PrintScreen, cx))
            .child(dispatch_button("cap-window", "Capture window", Action::ActiveWindow, cx))
            .child(dispatch_button("cap-monitor", "Capture monitor", Action::ActiveMonitor, cx))
            .child(dispatch_button("cap-last", "Last region", Action::LastRegion, cx))
            .child(div().flex_1())
            .child(dispatch_button("open-folder", "Screenshots folder", Action::OpenScreenshotsFolder, cx).ghost())
            .child(dispatch_button("open-history", "History", Action::OpenHistory, cx).ghost())
            .child(dispatch_button("open-settings", "Settings", Action::OpenSettings, cx).ghost());

        let rows = self.tasks.read(cx).rows().to_vec();
        let mut task_list = div().flex().flex_col().gap(px(6.));
        if rows.is_empty() {
            task_list = task_list.child(widgets::muted("No captures yet. Press PrintScreen or use the buttons above.", cx));
        }
        for (i, r) in rows.iter().enumerate() {
            let mut line = div().flex().flex_row().items_center().gap(px(8.)).p(px(8.)).rounded(px(6.)).bg(cx.theme().secondary).child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .child(div().truncate().child(r.title.clone()))
                    .child(widgets::muted(format!("{} · {}", status_label(r.status), r.detail), cx)),
            );
            if let Some(path) = &r.path {
                line = line
                    .child(open_path_button(format!("task-open-{i}").into(), "Open", path.clone(), false, cx))
                    .child(open_path_button(format!("task-folder-{i}").into(), "Folder", path.clone(), true, cx));
            }
            task_list = task_list.child(line);
        }

        let mut recent = div().flex().flex_row().flex_wrap().gap(px(8.));
        if self.recent.is_empty() {
            recent = recent.child(widgets::muted("History is empty.", cx));
        }
        for (i, r) in self.recent.iter().enumerate() {
            let mut card =
                div().w(px(150.)).flex().flex_col().gap(px(4.)).p(px(6.)).rounded(px(6.)).border_1().border_color(cx.theme().border);
            if let Some(thumb) = &r.thumbnail {
                card = card
                    .child(div().h(px(90.)).flex().justify_center().overflow_hidden().child(img(thumb.clone()).max_h_full().max_w_full()));
            }
            card = card.child(div().text_xs().truncate().child(r.title.clone())).child(widgets::muted(r.when.clone(), cx));
            if let Some(path) = &r.path {
                card = card.child(open_path_button(format!("recent-{i}").into(), "Open", path.clone(), false, cx));
            }
            recent = recent.child(card);
        }

        let app = cx.global::<UiState>().services.app.clone();
        let drop_hint = widgets::muted("Drop files here to upload them.", cx);
        widgets::page(cx)
            .id("main-drop")
            .drag_over::<gpui::ExternalPaths>(|style, _, _, cx| style.bg(cx.theme().secondary))
            .on_drop(move |paths: &gpui::ExternalPaths, _, _| {
                let ids = app.upload_files(paths.paths().to_vec(), None);
                tracing::info!(count = ids.len(), "dropped files submitted for upload");
            })
            .child(header)
            .child(div().px(px(12.)).child(drop_hint))
            .child(
            div()
                .flex_1()
                .flex()
                .flex_row()
                .gap(px(12.))
                .p(px(12.))
                .overflow_hidden()
                .child(widgets::section("Tasks", cx).w(px(420.)).child(task_list).overflow_y_scrollbar())
                .child(widgets::section("Recent captures", cx).flex_1().child(recent).overflow_y_scrollbar()),
        )
    }
}

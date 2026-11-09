//! The remote storage browser (M7 D11): the S3, FTP and SFTP accounts from the Uploads settings,
//! listed, uploaded to, downloaded from, renamed and deleted through the `RemoteStorage` port.

use std::sync::Arc;

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, Window, WindowBounds,
    WindowOptions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Disableable as _, Icon, Root, Selectable as _, Sizable as _};
use laqta_ports::{RemoteEntry, RemoteStorage};

use crate::assets::IconName;
use crate::runtime::UiState;
use crate::widgets;

pub(crate) fn open(cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(820.), px(620.)), cx)),
        titlebar: Some(crate::chrome::titlebar("Remote storage")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        focus: true,
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| RemoteWindow::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::error!("remote storage window failed: {e:#}");
    }
}

struct RemoteWindow {
    stores: Vec<(String, Arc<dyn RemoteStorage>)>,
    current: usize,
    /// "" is the account's start folder.
    dir: String,
    entries: Vec<RemoteEntry>,
    name: Entity<InputState>,
    message: Option<String>,
    busy: bool,
}

fn parent(dir: &str) -> String {
    let t = dir.trim_end_matches('/');
    match t.rfind('/') {
        Some(i) => t[..i].to_owned(),
        None => String::new(),
    }
}

fn join(dir: &str, name: &str) -> String {
    if dir.is_empty() { name.to_owned() } else { format!("{}/{name}", dir.trim_end_matches('/')) }
}

impl RemoteWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let stores = (cx.global::<UiState>().services.remote_storages)();
        let name = cx.new(|cx| InputState::new(window, cx).placeholder("New name or folder name"));
        let mut this = RemoteWindow { stores, current: 0, dir: String::new(), entries: Vec::new(), name, message: None, busy: false };
        this.refresh(cx);
        this
    }

    fn store(&self) -> Option<Arc<dyn RemoteStorage>> {
        self.stores.get(self.current).map(|(_, s)| s.clone())
    }

    /// Runs `job` on the background executor, then lists the folder again.
    fn run(
        &mut self,
        label: &'static str,
        job: impl std::future::Future<Output = Result<String, String>> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.busy = true;
        self.message = Some(format!("{label}…"));
        cx.notify();
        let task = cx.background_spawn(job);
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                this.busy = false;
                this.message = Some(r.unwrap_or_else(|e| format!("{label} failed: {e}")));
                this.refresh(cx);
            });
        })
        .detach();
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store() else {
            self.message = Some("Add an S3, FTP or SFTP account under Settings → Uploads first.".into());
            return cx.notify();
        };
        let dir = self.dir.clone();
        let task = cx.background_spawn(async move { store.list(dir).await });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(mut entries) => {
                        entries.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.to_lowercase().cmp(&b.name.to_lowercase())));
                        this.entries = entries;
                    }
                    Err(e) => {
                        this.entries.clear();
                        this.message = Some(format!("Couldn't list the folder: {e}"));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn upload(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        let dir = self.dir.clone();
        self.run(
            "Upload",
            async move {
                let Some(files) = rfd::AsyncFileDialog::new().pick_files().await else { return Ok("Nothing uploaded.".into()) };
                let mut n = 0;
                for f in files {
                    store.upload(f.path().to_path_buf(), dir.clone()).await.map_err(|e| e.to_string())?;
                    n += 1;
                }
                Ok(format!("Uploaded {n} file(s)."))
            },
            cx,
        );
    }

    fn download(&mut self, entry: RemoteEntry, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        self.run(
            "Download",
            async move {
                let Some(f) = rfd::AsyncFileDialog::new().set_file_name(entry.name.clone()).save_file().await else {
                    return Ok(String::new());
                };
                let dest = f.path().to_path_buf();
                store.download(entry.path, dest.clone()).await.map_err(|e| e.to_string())?;
                Ok(format!("Saved {}", dest.display()))
            },
            cx,
        );
    }

    fn rename(&mut self, entry: RemoteEntry, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() || name.contains('/') {
            self.message = Some("Type the new name in the box first.".into());
            return cx.notify();
        }
        let to = join(&parent(&entry.path), &name);
        self.run("Rename", async move { store.rename(entry.path, to).await.map(|()| "Renamed.".into()).map_err(|e| e.to_string()) }, cx);
    }

    fn delete(&mut self, entry: RemoteEntry, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        self.run("Delete", async move { store.delete(entry.path).await.map(|()| "Deleted.".into()).map_err(|e| e.to_string()) }, cx);
    }

    fn mkdir(&mut self, cx: &mut Context<Self>) {
        let Some(store) = self.store() else { return };
        let name = self.name.read(cx).value().trim().to_string();
        if name.is_empty() {
            self.message = Some("Type the folder name in the box first.".into());
            return cx.notify();
        }
        let path = join(&self.dir, &name);
        self.run("New folder", async move { store.mkdir(path).await.map(|()| "Folder created.".into()).map_err(|e| e.to_string()) }, cx);
    }
}

impl Render for RemoteWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut accounts = div().flex().flex_wrap().gap(px(4.));
        for (i, (label, _)) in self.stores.iter().enumerate() {
            accounts = accounts.child(
                Button::new(SharedString::from(format!("acct-{i}"))).label(label.clone()).small().selected(self.current == i).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.current = i;
                        this.dir.clear();
                        this.refresh(cx);
                    }),
                ),
            );
        }
        let bar = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .child(Button::new("up").label("Up").small().disabled(self.dir.is_empty()).on_click(cx.listener(|this, _, _, cx| {
                this.dir = parent(&this.dir);
                this.refresh(cx);
            })))
            .child(Button::new("refresh").label("Refresh").small().on_click(cx.listener(|this, _, _, cx| this.refresh(cx))))
            .child(
                Button::new("upload")
                    .label("Upload…")
                    .small()
                    .primary()
                    .disabled(self.busy)
                    .on_click(cx.listener(|this, _, _, cx| this.upload(cx))),
            )
            .child(div().w(px(200.)).child(Input::new(&self.name).small()))
            .child(
                Button::new("mkdir").label("New folder").small().disabled(self.busy).on_click(cx.listener(|this, _, _, cx| this.mkdir(cx))),
            )
            .child(div().flex_1())
            .child(widgets::muted(format!("/{}", self.dir), cx));
        let mut list = div().flex().flex_col();
        if self.entries.is_empty() {
            list = list.child(widgets::muted("Empty folder.", cx));
        }
        for (i, e) in self.entries.clone().into_iter().enumerate() {
            let size = e.size.map(crate::presenters::history::format_size).unwrap_or_default();
            let (open, dl, rn, del) = (e.clone(), e.clone(), e.clone(), e.clone());
            let mut row = div().flex().items_center().gap(px(6.)).py(px(2.)).border_b_1().border_color(cx.theme().border);
            row = if e.is_dir {
                row.child(
                    Button::new(SharedString::from(format!("open-{i}")))
                        .label(e.name.clone())
                        .icon(Icon::new(IconName::Folder))
                        .small()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.dir = open.path.clone();
                            this.refresh(cx);
                        })),
                )
            } else {
                row.child(
                    div()
                        .px(px(8.))
                        .flex()
                        .items_center()
                        .gap(px(6.))
                        .child(widgets::icon(IconName::File, cx.theme().muted_foreground))
                        .child(e.name.clone()),
                )
            };
            row =
                row.child(div().flex_1()).child(widgets::muted(size, cx)).child(widgets::muted(e.modified.clone().unwrap_or_default(), cx));
            if !e.is_dir {
                row = row.child(
                    Button::new(SharedString::from(format!("dl-{i}")))
                        .label("Download")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.download(dl.clone(), cx))),
                );
            }
            row = row
                .child(
                    Button::new(SharedString::from(format!("rn-{i}")))
                        .label("Rename")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.rename(rn.clone(), cx))),
                )
                .child(
                    Button::new(SharedString::from(format!("rm-{i}")))
                        .label("Delete")
                        .xsmall()
                        .ghost()
                        .on_click(cx.listener(move |this, _, _, cx| this.delete(del.clone(), cx))),
                );
            list = list.child(row);
        }
        crate::chrome::rounded(widgets::page(cx), window).child(crate::chrome::TitleBar::new().title("Remote storage").flat()).child(
            div()
                .flex_1()
                .flex()
                .flex_col()
                .gap(px(8.))
                .p(px(12.))
                .child(accounts)
                .child(bar)
                .child(div().flex_1().overflow_y_scrollbar().child(list))
                .child(widgets::muted(self.message.clone().unwrap_or_default(), cx)),
        )
    }
}

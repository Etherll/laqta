//! The settings window. It edits a draft copy of `Settings`; Save validates, then hands the value
//! to `main` (store, `App::set_settings`, hotkey re-registration). Workflow overrides are shown as
//! differences from the defaults (ADR 008).

use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, SharedString, Styled as _, TitlebarOptions, Window,
    WindowBounds, WindowOptions, div, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Disableable as _, Root, Selectable as _};
use laqta_domain::hotkey::KeyChord;
use laqta_domain::settings::Theme;
use laqta_domain::workflow::{AfterCaptureTasks, FileExistsAction, ToastPlacement, Workflow};
use laqta_domain::{Action, ImageFormat, Settings, WorkflowId};

use crate::presenters::settings::{self as present, TaskOption};
use crate::runtime::{UiState, activate};
use crate::widgets;

pub(crate) fn open(cx: &mut App) {
    if activate(cx.global::<UiState>().settings, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(920.), px(700.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some("Laqta settings".into()), ..Default::default() }),
        app_id: Some("laqta".into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SettingsWindow::new(window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        Ok(handle) => cx.global_mut::<UiState>().settings = Some(handle.into()),
        Err(e) => tracing::error!("settings window failed: {e:#}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    AfterCapture,
    Image,
    Naming,
    Capture,
    Workflows,
}

const PAGES: [(Page, &str); 6] = [
    (Page::General, "General"),
    (Page::AfterCapture, "After capture"),
    (Page::Image, "Image"),
    (Page::Naming, "File naming"),
    (Page::Capture, "Capture"),
    (Page::Workflows, "Hotkeys and workflows"),
];

struct WorkflowInputs {
    name: Entity<InputState>,
    hotkey: Entity<InputState>,
}

struct SettingsWindow {
    draft: Settings,
    page: Page,
    message: Option<(bool, String)>,
    dir: Entity<InputState>,
    pattern: Entity<InputState>,
    window_pattern: Entity<InputState>,
    subfolder: Entity<InputState>,
    jpeg_quality: Entity<InputState>,
    auto_jpeg: Entity<InputState>,
    thumb_w: Entity<InputState>,
    thumb_h: Entity<InputState>,
    delay: Entity<InputState>,
    toast_ms: Entity<InputState>,
    workflows: Vec<WorkflowInputs>,
}

fn text_input(value: impl Into<SharedString>, window: &mut Window, cx: &mut Context<SettingsWindow>) -> Entity<InputState> {
    let value = value.into();
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.global::<UiState>().services.app.settings();
        let d = &draft.defaults;
        let mut this = SettingsWindow {
            dir: text_input(draft.screenshots_dir.display().to_string(), window, cx),
            pattern: text_input(d.naming.pattern.clone(), window, cx),
            window_pattern: text_input(d.naming.window_pattern.clone(), window, cx),
            subfolder: text_input(d.naming.subfolder.clone(), window, cx),
            jpeg_quality: text_input(d.image.jpeg_quality.to_string(), window, cx),
            auto_jpeg: text_input(d.image.auto_jpeg_above_bytes.map(|b| (b / 1024).to_string()).unwrap_or_default(), window, cx),
            thumb_w: text_input(d.image.thumbnail_width.to_string(), window, cx),
            thumb_h: text_input(d.image.thumbnail_height.to_string(), window, cx),
            delay: text_input(d.capture.delay_ms.to_string(), window, cx),
            toast_ms: text_input(d.general.toast.duration_ms.to_string(), window, cx),
            workflows: Vec::new(),
            draft,
            page: Page::General,
            message: None,
        };
        this.rebuild_workflow_inputs(window, cx);
        this
    }

    fn rebuild_workflow_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let wf = self.draft.workflows.clone();
        self.workflows = wf
            .iter()
            .map(|w| WorkflowInputs {
                name: text_input(w.name.clone(), window, cx),
                hotkey: text_input(w.hotkey.map(|h| h.to_string()).unwrap_or_default(), window, cx),
            })
            .collect();
    }

    fn value(input: &Entity<InputState>, cx: &App) -> String {
        input.read(cx).value().trim().to_string()
    }

    /// Copies text inputs into the draft. Returns the first validation error.
    fn collect(&mut self, cx: &App) -> Result<(), String> {
        let num = |input: &Entity<InputState>, what: &str| -> Result<u32, String> {
            let v = Self::value(input, cx);
            if v.is_empty() {
                return Ok(0);
            }
            v.parse::<u32>().map_err(|_| format!("{what} must be a whole number"))
        };
        let dir = Self::value(&self.dir, cx);
        if dir.is_empty() {
            return Err("The screenshots folder can't be empty".into());
        }
        self.draft.screenshots_dir = dir.into();
        let d = &mut self.draft.defaults;
        d.naming.pattern = Self::value(&self.pattern, cx);
        d.naming.window_pattern = Self::value(&self.window_pattern, cx);
        d.naming.subfolder = Self::value(&self.subfolder, cx);
        let q = num(&self.jpeg_quality, "JPEG quality")?;
        if !(1..=100).contains(&q) {
            return Err("JPEG quality must be between 1 and 100".into());
        }
        d.image.jpeg_quality = q as u8;
        let kb = num(&self.auto_jpeg, "The JPEG size threshold")?;
        d.image.auto_jpeg_above_bytes = (kb > 0).then_some(kb as u64 * 1024);
        d.image.thumbnail_width = num(&self.thumb_w, "Thumbnail width")?;
        d.image.thumbnail_height = num(&self.thumb_h, "Thumbnail height")?;
        d.capture.delay_ms = num(&self.delay, "Capture delay")?;
        d.general.toast.duration_ms = num(&self.toast_ms, "Toast duration")?.max(500);
        for (w, inputs) in self.draft.workflows.iter_mut().zip(&self.workflows) {
            w.name = Self::value(&inputs.name, cx);
            let hk = Self::value(&inputs.hotkey, cx);
            w.hotkey = if hk.is_empty() {
                None
            } else {
                Some(hk.parse::<KeyChord>().map_err(|e| format!("Hotkey \"{hk}\" for {}: {e}", w.name))?)
            };
        }
        Ok(())
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        self.message = Some(match self.collect(cx) {
            Err(e) => (false, e),
            Ok(()) => {
                let conflicts = present::hotkey_conflicts(&self.draft);
                let save = cx.global::<UiState>().services.save_settings.clone();
                match save(self.draft.clone()) {
                    Ok(()) => {
                        crate::theme::apply(self.draft.theme, cx);
                        if conflicts.is_empty() { (true, "Settings saved.".into()) } else { (true, format!("Saved. {}", conflicts.join("; "))) }
                    }
                    Err(e) => (false, format!("Couldn't save settings: {e}")),
                }
            }
        });
        cx.notify();
    }

    fn after_capture_list(
        &self,
        id_prefix: &str,
        flags: AfterCaptureTasks,
        on_toggle: impl Fn(&mut Self, AfterCaptureTasks, bool) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut list = div().flex().flex_col().gap(px(6.));
        for TaskOption { flag, label, available } in present::after_capture_options() {
            let f = on_toggle.clone();
            let label = if available { label.to_string() } else { format!("{label} (later milestone)") };
            list = list.child(
                Checkbox::new(SharedString::from(format!("{id_prefix}-{}", flag.bits())))
                    .label(label)
                    .checked(flags.contains(flag))
                    .disabled(!available)
                    .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                        f(this, flag, *checked);
                        cx.notify();
                    })),
            );
        }
        list
    }

    fn general(&self, cx: &mut Context<Self>) -> gpui::Div {
        let data_dir = cx.global::<UiState>().services.data_dir.display().to_string();
        let theme_button = |id: &'static str, label: &'static str, t: Theme, this: &Self, cx: &mut Context<Self>| {
            Button::new(id).label(label).selected(this.draft.theme == t).on_click(cx.listener(move |this, _, _, cx| {
                this.draft.theme = t;
                cx.notify();
            }))
        };
        let placement_button = |id: &'static str, label: &'static str, p: ToastPlacement, this: &Self, cx: &mut Context<Self>| {
            Button::new(id).label(label).selected(this.draft.defaults.general.toast.placement == p).on_click(cx.listener(move |this, _, _, cx| {
                this.draft.defaults.general.toast.placement = p;
                cx.notify();
            }))
        };
        widgets::section("General", cx)
            .child(widgets::row("Screenshots folder", Input::new(&self.dir), cx))
            .child(widgets::row(
                "Theme",
                div()
                    .flex()
                    .gap(px(6.))
                    .child(theme_button("theme-system", "System", Theme::System, self, cx))
                    .child(theme_button("theme-light", "Light", Theme::Light, self, cx))
                    .child(theme_button("theme-dark", "Dark", Theme::Dark, self, cx)),
                cx,
            ))
            .child(Checkbox::new("show-toast").label("Show a toast after capture").checked(self.draft.defaults.general.show_toast).on_click(
                cx.listener(|this, c: &bool, _, cx| {
                    this.draft.defaults.general.show_toast = *c;
                    cx.notify();
                }),
            ))
            .child(widgets::row("Toast duration (ms)", Input::new(&self.toast_ms), cx))
            .child(widgets::row(
                "Toast position",
                div()
                    .flex()
                    .gap(px(6.))
                    .child(placement_button("tp-tl", "Top left", ToastPlacement::TopLeft, self, cx))
                    .child(placement_button("tp-tr", "Top right", ToastPlacement::TopRight, self, cx))
                    .child(placement_button("tp-bl", "Bottom left", ToastPlacement::BottomLeft, self, cx))
                    .child(placement_button("tp-br", "Bottom right", ToastPlacement::BottomRight, self, cx)),
                cx,
            ))
            .child(Checkbox::new("history").label("Keep history").checked(self.draft.history.enabled).on_click(cx.listener(
                |this, c: &bool, _, cx| {
                    this.draft.history.enabled = *c;
                    cx.notify();
                },
            )))
            .child(widgets::muted(format!("Data folder: {data_dir}"), cx))
    }

    fn image(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut formats = div().flex().flex_wrap().gap(px(6.));
        for f in ImageFormat::ALL {
            formats = formats.child(
                Button::new(SharedString::from(format!("fmt-{}", f.extension())))
                    .label(present::format_label(f))
                    .selected(self.draft.defaults.image.format == f)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.defaults.image.format = f;
                        cx.notify();
                    })),
            );
        }
        let mut exists = div().flex().gap(px(6.));
        for (a, label) in [
            (FileExistsAction::AppendNumber, "Add a number"),
            (FileExistsAction::Overwrite, "Overwrite"),
            (FileExistsAction::Skip, "Skip"),
        ] {
            exists = exists.child(
                Button::new(SharedString::from(format!("exists-{label}")))
                    .label(label)
                    .selected(self.draft.defaults.image.file_exists == a)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.defaults.image.file_exists = a;
                        cx.notify();
                    })),
            );
        }
        widgets::section("Image", cx)
            .child(widgets::row("Format", formats, cx))
            .child(widgets::row("JPEG quality (1–100)", Input::new(&self.jpeg_quality), cx))
            .child(widgets::row("Use JPEG above (KB, empty = never)", Input::new(&self.auto_jpeg), cx))
            .child(widgets::row("If the file exists", exists, cx))
            .child(widgets::row("Thumbnail width (px)", Input::new(&self.thumb_w), cx))
            .child(widgets::row("Thumbnail height (px, 0 = keep ratio)", Input::new(&self.thumb_h), cx))
    }

    fn naming(&self, cx: &mut Context<Self>) -> gpui::Div {
        let services = &cx.global::<UiState>().services;
        let (now, host) = (services.clock.now(), services.host.clone());
        let preview = |input: &Entity<InputState>, cx: &App| present::name_preview(&Self::value(input, cx), now, &host);
        let (p1, p2, p3) = (preview(&self.pattern, cx), preview(&self.window_pattern, cx), preview(&self.subfolder, cx));
        widgets::section("File naming", cx)
            .child(widgets::row("Name pattern", Input::new(&self.pattern), cx))
            .child(widgets::muted(format!("Preview: {p1}"), cx))
            .child(widgets::row("Name pattern for windows", Input::new(&self.window_pattern), cx))
            .child(widgets::muted(format!("Preview: {p2}"), cx))
            .child(widgets::row("Subfolder pattern", Input::new(&self.subfolder), cx))
            .child(widgets::muted(format!("Preview: {p3}"), cx))
            .child(widgets::muted(
                "Tokens: %y %yy %mo %mon %mon2 %d %h %mi %s %ms %pm %w %w2 %wy %unix %t %pn %i %ia %iAa %ib %ix %rn %ra %rna %rx %guid %radjective %ranimal %remoji %rf %width %height %un %uln %cn %n. Add {n} to repeat random tokens or pad counters.",
                cx,
            ))
    }

    fn capture(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = &self.draft.defaults.capture;
        let r = &c.region;
        let check = |id: &'static str, label: &'static str, on: bool, set: fn(&mut Settings, bool), cx: &mut Context<Self>| {
            Checkbox::new(id).label(label).checked(on).on_click(cx.listener(move |this, v: &bool, _, cx| {
                set(&mut this.draft, *v);
                cx.notify();
            }))
        };
        widgets::section("Capture", cx)
            .child(widgets::row("Delay before capture (ms)", Input::new(&self.delay), cx))
            .child(check("cursor", "Show the mouse cursor", c.show_cursor, |s, v| s.defaults.capture.show_cursor = v, cx))
            .child(check("multi", "Allow several regions", r.multi_region, |s, v| s.defaults.capture.region.multi_region = v, cx))
            .child(check("snap", "Snap to windows", r.snap_to_windows, |s, v| s.defaults.capture.region.snap_to_windows = v, cx))
            .child(check("magnifier", "Show the magnifier", r.show_magnifier, |s, v| s.defaults.capture.region.show_magnifier = v, cx))
            .child(check("crosshair", "Show crosshair lines", r.show_crosshair, |s, v| s.defaults.capture.region.show_crosshair = v, cx))
            .child(check("info", "Show position and colour", r.show_info, |s, v| s.defaults.capture.region.show_info = v, cx))
    }

    fn workflows_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut page = widgets::section("Hotkeys and workflows", cx).child(widgets::muted(
            "Each workflow runs an action, optionally with its own settings. Type a hotkey such as Ctrl+Shift+PrintScreen; leave it empty for none.",
            cx,
        ));
        if self.draft.disable_hotkeys {
            page = page.child(div().text_color(cx.theme().danger).child("Hotkeys are turned off."));
        }
        page = page.child(Checkbox::new("disable-hotkeys").label("Turn off all hotkeys").checked(self.draft.disable_hotkeys).on_click(
            cx.listener(|this, v: &bool, _, cx| {
                this.draft.disable_hotkeys = *v;
                cx.notify();
            }),
        ));
        for c in present::hotkey_conflicts(&self.draft) {
            page = page.child(div().text_color(cx.theme().danger).child(c));
        }
        for (i, (w, inputs)) in self.draft.workflows.iter().zip(&self.workflows).enumerate() {
            let overrides = present::overridden_labels(&w.patch);
            let mut card = div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .p(px(8.))
                .rounded(px(6.))
                .bg(cx.theme().secondary)
                .child(
                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .gap(px(8.))
                        .child(div().w(px(220.)).child(Input::new(&inputs.name)))
                        .child(div().w(px(200.)).child(Input::new(&inputs.hotkey)))
                        .child(Checkbox::new(SharedString::from(format!("wf-en-{i}"))).label("On").checked(w.enabled).on_click(cx.listener(
                            move |this, v: &bool, _, cx| {
                                if let Some(w) = this.draft.workflows.get_mut(i) {
                                    w.enabled = *v;
                                }
                                cx.notify();
                            },
                        )))
                        .child(div().flex_1())
                        .child(Button::new(SharedString::from(format!("wf-rm-{i}"))).label("Remove").ghost().on_click(cx.listener(
                            move |this, _, window, cx| {
                                if this.collect(cx).is_ok() && i < this.draft.workflows.len() {
                                    this.draft.workflows.remove(i);
                                    this.rebuild_workflow_inputs(window, cx);
                                }
                                cx.notify();
                            },
                        ))),
                )
                .child(widgets::muted(
                    format!(
                        "Action: {}{}",
                        w.action.label(),
                        if overrides.is_empty() { String::new() } else { format!(" · Overrides: {}", overrides.join(", ")) }
                    ),
                    cx,
                ));
            let overriding = w.patch.after_capture.is_some();
            card = card.child(
                Checkbox::new(SharedString::from(format!("wf-ov-{i}")))
                    .label("Use its own after-capture tasks")
                    .checked(overriding)
                    .on_click(cx.listener(move |this, v: &bool, _, cx| {
                        let defaults = this.draft.defaults.after_capture;
                        if let Some(w) = this.draft.workflows.get_mut(i) {
                            w.patch.after_capture = v.then_some(defaults);
                        }
                        cx.notify();
                    })),
            );
            if let Some(flags) = w.patch.after_capture {
                card = card.child(div().pl(px(24.)).child(self.after_capture_list(
                    &format!("wf-{i}"),
                    flags,
                    move |this, flag, on| {
                        if let Some(Some(f)) = this.draft.workflows.get_mut(i).map(|w| w.patch.after_capture.as_mut()) {
                            f.set(flag, on);
                        }
                    },
                    cx,
                )));
            }
            page = page.child(card);
        }
        let mut add = div().flex().flex_wrap().gap(px(6.)).child(widgets::muted("Add:", cx));
        for action in [Action::RectangleRegion, Action::PrintScreen, Action::ActiveWindow, Action::ActiveMonitor, Action::LastRegion, Action::CustomRegion]
        {
            add = add.child(Button::new(SharedString::from(format!("add-{action:?}"))).label(action.label()).ghost().on_click(cx.listener(
                move |this, _, window, cx| {
                    if this.collect(cx).is_ok() {
                        let id = this.draft.workflows.iter().map(|w| w.id.0).max().unwrap_or(0) + 1;
                        this.draft.workflows.push(Workflow {
                            id: WorkflowId(id),
                            name: action.label().to_string(),
                            action,
                            hotkey: None,
                            enabled: true,
                            patch: Default::default(),
                        });
                        this.rebuild_workflow_inputs(window, cx);
                    }
                    cx.notify();
                },
            )));
        }
        page.child(add)
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().w(px(200.)).flex().flex_col().gap(px(4.)).p(px(12.)).border_r_1().border_color(cx.theme().border);
        for (page, label) in PAGES {
            nav = nav.child(Button::new(SharedString::from(format!("nav-{label}"))).label(label).ghost().selected(self.page == page).on_click(
                cx.listener(move |this, _, _, cx| {
                    this.page = page;
                    cx.notify();
                }),
            ));
        }
        let content = match self.page {
            Page::General => self.general(cx),
            Page::AfterCapture => {
                let flags = self.draft.defaults.after_capture;
                widgets::section("After capture (defaults)", cx).child(self.after_capture_list("ac", flags, |this, f, on| this.draft.defaults.after_capture.set(f, on), cx))
            }
            Page::Image => self.image(cx),
            Page::Naming => self.naming(cx),
            Page::Capture => self.capture(cx),
            Page::Workflows => self.workflows_page(cx),
        };
        let mut footer = div().flex().flex_row().items_center().gap(px(8.)).p(px(12.)).border_t_1().border_color(cx.theme().border);
        if let Some((ok, msg)) = &self.message {
            footer = footer.child(div().text_color(if *ok { cx.theme().foreground } else { cx.theme().danger }).child(msg.clone()));
        }
        footer = footer
            .child(div().flex_1())
            .child(Button::new("close").label("Close").on_click(|_, window, _| window.remove_window()))
            .child(Button::new("save").label("Save").primary().on_click(cx.listener(|this, _, _, cx| this.save(cx))));
        widgets::page(cx)
            .child(div().flex_1().flex().flex_row().overflow_hidden().child(nav).child(div().flex_1().p(px(12.)).overflow_y_scrollbar().child(content)))
            .child(footer)
    }
}

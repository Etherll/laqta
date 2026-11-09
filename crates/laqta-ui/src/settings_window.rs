//! The settings window. It edits a draft copy of `Settings`; Save validates, then hands the value
//! to `main` (store, `App::set_settings`, hotkey re-registration). Workflow overrides are shown as
//! differences from the defaults (ADR 008).

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, WindowBounds, WindowOptions, div,
    prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Input, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::capture::RegionMouseAction;
use laqta_domain::geometry::PhysRect;
use laqta_domain::hotkey::KeyChord;
use laqta_domain::settings::Theme;
use laqta_domain::workflow::{AfterCaptureTasks, FileExistsAction, ToastPlacement, Workflow};
use laqta_domain::{Action, ImageFormat, Settings, WorkflowId};

use crate::assets::IconName;
use crate::presenters::settings::{self as present, TaskOption};
use crate::runtime::{UiState, activate};
use crate::theme::{space, text};
use crate::widgets::{self, Pill, Slot, Tone};

pub(crate) fn open(cx: &mut App) {
    if activate(cx.global::<UiState>().settings, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1040.), px(760.)), cx)),
        window_min_size: Some(size(px(720.), px(480.))),
        titlebar: Some(crate::chrome::titlebar("Laqta settings")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    match cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| SettingsWindow::new(window, cx));
        let focus = view.read(cx).focus.clone();
        let root = cx.new(|cx| Root::new(view, window, cx));
        // Focus the page after Root, which takes focus when it is built, so Tab reaches it.
        window.focus(&focus, cx);
        root
    }) {
        Ok(handle) => cx.global_mut::<UiState>().settings = Some(handle.into()),
        Err(e) => tracing::error!("settings window failed: {e:#}"),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum DestKind {
    Image,
    Text,
    File,
    Shortener,
    Sharer,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Page {
    General,
    AfterCapture,
    Image,
    Naming,
    Capture,
    Workflows,
    Uploads,
    Recording,
    Automation,
    Models,
    Integration,
    Data,
}

const PAGES: [(Page, &str, IconName); 12] = [
    (Page::General, "General", IconName::SlidersHorizontal),
    (Page::AfterCapture, "After capture", IconName::ListChecks),
    (Page::Image, "Image format", IconName::Image),
    (Page::Naming, "File naming", IconName::Type),
    (Page::Capture, "Capture", IconName::Crop),
    (Page::Workflows, "Shortcuts", IconName::Keyboard),
    (Page::Uploads, "Uploads", IconName::CloudUpload),
    (Page::Recording, "Recording", IconName::Video),
    (Page::Automation, "Automation", IconName::Timer),
    (Page::Models, "AI features", IconName::Sparkles),
    (Page::Integration, "Integration", IconName::Plug),
    (Page::Data, "Backup and import", IconName::Database),
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
    custom_region: Entity<InputState>,
    window_title: Entity<InputState>,
    sound_capture: Entity<InputState>,
    sound_task: Entity<InputState>,
    sound_error: Entity<InputState>,
    workflows: Vec<WorkflowInputs>,
    clipboard_format: Entity<InputState>,
    retries: Entity<InputState>,
    hosts: Vec<Vec<(&'static str, bool, Entity<InputState>)>>,
    rec: RecordingInputs,
    auto_interval: Entity<InputState>,
    auto_count: Entity<InputState>,
    scroll: [Entity<InputState>; 6],
    /// Per watch folder: folder and filter inputs, parallel to `draft.watch_folders`.
    watch: Vec<(Entity<InputState>, Entity<InputState>)>,
    model_rows: Vec<crate::runtime::ModelRow>,
    /// Download progress or the last error, per model id.
    model_status: std::collections::HashMap<String, String>,
    custom_family: usize,
    custom_name: Entity<InputState>,
    /// Analyzer base URL, model, prompt, key; remove.bg key (keys go to the secret store on save).
    cloud: [Entity<InputState>; 5],
    /// The last update check's message, and whether it can be installed from here.
    update: Option<(String, bool)>,
    /// Run-at-login and file-manager menu state, read off the UI thread (`refresh_integration`).
    /// `None` until the first read lands.
    integration_status: Option<(Result<bool, String>, Result<bool, String>)>,
    focus: FocusHandle,
    language: Entity<SelectState<Vec<SharedString>>>,
    /// The shortcut row listening for a key combination, and the focus that receives its keys.
    recording: Option<usize>,
    rec_focus: FocusHandle,
    /// Why the last key press was refused while recording.
    rec_error: Option<&'static str>,
}

struct RecordingInputs {
    fps: Entity<InputState>,
    gif_fps: Entity<InputState>,
    crf: Entity<InputState>,
    countdown: Entity<InputState>,
    duration: Entity<InputState>,
    microphone: Entity<InputState>,
    args: Entity<InputState>,
    ffmpeg: Entity<InputState>,
    camera_device: Entity<InputState>,
    camera_size: Entity<InputState>,
}

fn text_input(value: impl Into<SharedString>, window: &mut Window, cx: &mut Context<SettingsWindow>) -> Entity<InputState> {
    let value = value.into();
    cx.new(|cx| InputState::new(window, cx).default_value(value))
}

impl SettingsWindow {
    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.global::<UiState>().services.app.settings();
        let language = Self::language_select(draft.language.as_deref(), window, cx);
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
            custom_region: text_input(
                d.capture.custom_region.map(|r| format!("{}, {}, {}, {}", r.x, r.y, r.width, r.height)).unwrap_or_default(),
                window,
                cx,
            ),
            window_title: text_input(d.capture.custom_window_title.clone(), window, cx),
            sound_capture: text_input(path_text(&d.general.custom_capture_sound), window, cx),
            sound_task: text_input(path_text(&d.general.custom_task_sound), window, cx),
            sound_error: text_input(path_text(&d.general.custom_error_sound), window, cx),
            workflows: Vec::new(),
            clipboard_format: text_input(d.upload.clipboard_format.clone(), window, cx),
            retries: text_input(d.upload.retries.to_string(), window, cx),
            hosts: Vec::new(),
            update: None,
            focus: cx.focus_handle(),
            language,
            recording: None,
            rec_focus: cx.focus_handle(),
            rec_error: None,
            rec: RecordingInputs {
                fps: text_input(d.recording.fps.to_string(), window, cx),
                gif_fps: text_input(d.recording.gif_fps.to_string(), window, cx),
                crf: text_input(
                    match d.recording.quality {
                        laqta_domain::record::Quality::Crf(c) => c.to_string(),
                        _ => String::new(),
                    },
                    window,
                    cx,
                ),
                countdown: text_input(d.recording.countdown_secs.to_string(), window, cx),
                duration: text_input(d.recording.fixed_duration_secs.to_string(), window, cx),
                microphone: text_input(d.recording.microphone.clone().unwrap_or_default(), window, cx),
                args: text_input(d.recording.custom_args.clone(), window, cx),
                ffmpeg: text_input(d.recording.ffmpeg_path.clone(), window, cx),
                camera_device: text_input(d.recording.camera.device.clone(), window, cx),
                camera_size: text_input(d.recording.camera.size_percent.to_string(), window, cx),
            },
            auto_interval: text_input((draft.auto_capture.interval_ms / 1000).to_string(), window, cx),
            auto_count: text_input(draft.auto_capture.count.to_string(), window, cx),
            scroll: {
                let o = &d.capture.scrolling;
                [o.start_delay_ms, o.scroll_delay_ms, o.max_frames, o.scroll_amount, o.ignore_top, o.ignore_bottom]
                    .map(|v| text_input(v.to_string(), window, cx))
            },
            watch: Vec::new(),
            // Filled by `refresh_model_rows`: building the rows parses the catalog and checks files.
            model_rows: Vec::new(),
            model_status: Default::default(),
            custom_family: 0,
            custom_name: text_input("My model", window, cx),
            cloud: {
                let a = &draft.cloud.analyzer;
                let (url, model, prompt) = (a.base_url.clone(), a.model.clone(), a.prompt.clone());
                let mut key = || cx.new(|cx| InputState::new(window, cx).placeholder("(stored in the system keyring; type to replace)"));
                let (k1, k2) = (key(), key());
                [text_input(url, window, cx), text_input(model, window, cx), text_input(prompt, window, cx), k1, k2]
            },
            draft,
            page: Page::General,
            message: None,
            integration_status: None,
        };
        this.refresh_model_rows(cx);
        this.refresh_integration(cx);
        this.rebuild_workflow_inputs(window, cx);
        this.rebuild_host_inputs(window, cx);
        this.rebuild_watch_inputs(window, cx);
        this
    }

    /// Reloads the model rows on a background thread.
    fn refresh_model_rows(&mut self, cx: &mut Context<Self>) {
        let models = cx.global::<UiState>().services.models.clone();
        let task = cx.background_spawn(async move { models.rows() });
        cx.spawn(async move |this, cx| {
            let rows = task.await;
            let _ = this.update(cx, |this, cx| {
                this.model_rows = rows;
                cx.notify();
            });
        })
        .detach();
    }

    /// Reads the run-at-login and file-manager menu state on a background thread: both read files.
    fn refresh_integration(&mut self, cx: &mut Context<Self>) {
        let services = &cx.global::<UiState>().services;
        let (autostart, integration) = (services.autostart.clone(), services.integration.clone());
        let task = cx.background_spawn(async move {
            (autostart.is_enabled().map_err(|e| e.to_string()), integration.is_installed().map_err(|e| e.to_string()))
        });
        cx.spawn(async move |this, cx| {
            let status = task.await;
            let _ = this.update(cx, |this, cx| {
                this.integration_status = Some(status);
                cx.notify();
            });
        })
        .detach();
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
        d.capture.custom_window_title = Self::value(&self.window_title, cx);
        d.capture.custom_region = parse_region(&Self::value(&self.custom_region, cx))?;
        let path = |input: &Entity<InputState>| {
            let v = Self::value(input, cx);
            (!v.is_empty()).then(|| std::path::PathBuf::from(v))
        };
        d.general.custom_capture_sound = path(&self.sound_capture);
        d.general.custom_task_sound = path(&self.sound_task);
        d.general.custom_error_sound = path(&self.sound_error);
        d.general.toast.duration_ms = num(&self.toast_ms, "Toast duration")?.max(500);
        {
            let r = &mut self.draft.defaults.recording;
            r.fps = num(&self.rec.fps, "Frame rate")?.clamp(1, 240);
            r.gif_fps = num(&self.rec.gif_fps, "GIF frame rate")?.clamp(1, 60);
            let crf = Self::value(&self.rec.crf, cx);
            r.quality = if crf.is_empty() {
                laqta_domain::record::Quality::Default
            } else {
                laqta_domain::record::Quality::Crf(crf.parse::<u8>().ok().filter(|c| *c <= 63).ok_or("Quality (CRF) must be 0–63")?)
            };
            r.countdown_secs = num(&self.rec.countdown, "Countdown")?;
            r.fixed_duration_secs = num(&self.rec.duration, "Fixed duration")?;
            let mic = Self::value(&self.rec.microphone, cx);
            r.microphone = (!mic.is_empty()).then_some(mic);
            r.custom_args = Self::value(&self.rec.args, cx);
            r.ffmpeg_path = Self::value(&self.rec.ffmpeg, cx);
            r.camera.device = Self::value(&self.rec.camera_device, cx);
            r.camera.size_percent = num(&self.rec.camera_size, "Camera size")?.clamp(5, 100) as u8;
        }
        {
            let o = &mut self.draft.defaults.capture.scrolling;
            o.start_delay_ms = num(&self.scroll[0], "Start delay")?;
            o.scroll_delay_ms = num(&self.scroll[1], "Scroll delay")?.max(50);
            o.max_frames = num(&self.scroll[2], "Maximum frames")?.clamp(2, 500);
            o.scroll_amount = num(&self.scroll[3], "Scroll amount")?.clamp(1, 50);
            o.ignore_top = num(&self.scroll[4], "Fixed rows at the top")?;
            o.ignore_bottom = num(&self.scroll[5], "Fixed rows at the bottom")?;
        }
        self.draft.cloud.analyzer.base_url = Self::value(&self.cloud[0], cx);
        self.draft.cloud.analyzer.model = Self::value(&self.cloud[1], cx);
        self.draft.cloud.analyzer.prompt = Self::value(&self.cloud[2], cx);
        {
            let secrets = cx.global::<UiState>().services.secrets.clone();
            for (input, key) in [(&self.cloud[3], "cloud:analyzer"), (&self.cloud[4], "cloud:remove.bg")] {
                let v = Self::value(input, cx);
                if !v.is_empty() {
                    secrets.set(key, &v).map_err(|e| format!("Couldn't store the key: {e}"))?;
                }
            }
        }
        self.draft.auto_capture.interval_ms = num(&self.auto_interval, "Auto-capture interval")?.max(1) * 1000;
        self.draft.auto_capture.count = num(&self.auto_count, "Auto-capture count")?;
        for (folder, (dir, filter)) in self.draft.watch_folders.iter_mut().zip(&self.watch) {
            folder.folder = Self::value(dir, cx).into();
            let f = Self::value(filter, cx);
            folder.filter = if f.is_empty() { "*".into() } else { f };
        }
        self.draft.defaults.upload.clipboard_format = Self::value(&self.clipboard_format, cx);
        self.draft.defaults.upload.retries = num(&self.retries, "Retries")?.min(10);
        let secrets = cx.global::<UiState>().services.secrets.clone();
        for (account, inputs) in self.draft.hosts.iter_mut().zip(&self.hosts) {
            for (key, secret, input) in inputs {
                let v = Self::value(input, cx);
                if *secret {
                    if !v.is_empty() {
                        secrets.set(&account.secret_key(key), &v).map_err(|e| format!("Couldn't store the {key}: {e}"))?;
                    }
                } else {
                    account.values.insert(key.to_string(), v);
                }
            }
        }
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
                        if conflicts.is_empty() {
                            (true, "Settings saved.".into())
                        } else {
                            (true, format!("Saved. {}", conflicts.join("; ")))
                        }
                    }
                    Err(e) => (false, format!("Couldn't save settings: {e}")),
                }
            }
        });
        cx.notify();
    }

    /// A switch row that writes one draft field.
    fn sw(
        &self,
        id: &'static str,
        label: &'static str,
        description: Option<&str>,
        on: bool,
        set: impl Fn(&mut Self, bool) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let description = description.map(|d| SharedString::from(d.to_string()));
        widgets::switch_row(
            id,
            label,
            description,
            on,
            cx.listener(move |this, v: &bool, _, cx| {
                set(this, *v);
                cx.notify();
            }),
            cx,
        )
    }

    /// One option of a [`widgets::choice`] that writes one draft field.
    fn opt(
        &self,
        label: impl Into<SharedString>,
        selected: bool,
        set: impl Fn(&mut Self) + 'static,
        cx: &mut Context<Self>,
    ) -> widgets::Choice {
        widgets::Choice::new(
            label,
            selected,
            cx.listener(move |this, _, _, cx| {
                set(this);
                cx.notify();
            }),
        )
    }

    fn after_capture_list(
        &self,
        id_prefix: &str,
        flags: AfterCaptureTasks,
        on_toggle: impl Fn(&mut Self, AfterCaptureTasks, bool) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut list = div().flex().flex_col().gap(px(36.));
        let mut group: Option<(&'static str, gpui::Div)> = None;
        for TaskOption { flag, label, available, group: name } in present::after_capture_options() {
            if group.as_ref().map(|(g, _)| *g) != Some(name) {
                if let Some((_, g)) = group.take() {
                    list = list.child(g);
                }
                group = Some((name, widgets::section(name, cx)));
            }
            let f = on_toggle.clone();
            let label = if available { label.to_string() } else { format!("{label} (later milestone)") };
            let row = widgets::switch_row(
                SharedString::from(format!("{id_prefix}-{}", flag.bits())),
                label,
                None::<&str>,
                flags.contains(flag),
                cx.listener(move |this, checked: &bool, _, cx| {
                    f(this, flag, *checked);
                    cx.notify();
                }),
                cx,
            );
            group = group.map(|(g, d)| (g, d.child(row)));
        }
        if let Some((_, g)) = group {
            list = list.child(g);
        }
        list
    }
    fn general(&self, cx: &mut Context<Self>) -> gpui::Div {
        let data_dir = cx.global::<UiState>().services.data_dir.display().to_string();
        let theme = [("System", Theme::System), ("Light", Theme::Light), ("Dark", Theme::Dark)]
            .into_iter()
            .map(|(label, t)| self.opt(label, self.draft.theme == t, move |this| this.draft.theme = t, cx))
            .collect();
        let placement = [
            ("Top left", ToastPlacement::TopLeft),
            ("Top right", ToastPlacement::TopRight),
            ("Bottom left", ToastPlacement::BottomLeft),
            ("Bottom right", ToastPlacement::BottomRight),
        ]
        .into_iter()
        .map(|(label, p)| {
            let on = self.draft.defaults.general.toast.placement == p;
            self.opt(label, on, move |this| this.draft.defaults.general.toast.placement = p, cx)
        })
        .collect();
        let g = &self.draft.defaults.general;
        let update_note = self.update.as_ref().map(|(m, _)| m.clone()).unwrap_or_else(|| "Laqta checks GitHub for a newer version.".into());
        page()
            .child(
                widgets::section("Appearance", cx)
                    .child(widgets::row_with("Theme", None::<&str>, Slot::Hug, widgets::choice("theme", theme, cx), cx))
                    .child(widgets::row_with(
                        "Language",
                        Some("Some text may still be in English. The tray menu changes after a restart."),
                        Slot::Field,
                        self.language_picker(cx),
                        cx,
                    )),
            )
            .child(
                widgets::section("Storage", cx)
                    .child(widgets::row_with(
                        "Screenshots folder",
                        Some("Where captures are saved."),
                        Slot::Field,
                        Input::new(&self.dir),
                        cx,
                    ))
                    .child(self.sw(
                        "history",
                        "Keep history",
                        Some(&format!("A list of your captures and uploads, stored in {data_dir}.")),
                        self.draft.history.enabled,
                        |this, v| this.draft.history.enabled = v,
                        cx,
                    )),
            )
            .child(
                widgets::section("Notifications", cx)
                    .child(self.sw(
                        "show-toast",
                        "Show a notification after capture",
                        Some("A small preview in the corner of the screen."),
                        g.show_toast,
                        |this, v| this.draft.defaults.general.show_toast = v,
                        cx,
                    ))
                    .child(widgets::row_with("Show it for", Some("Milliseconds."), Slot::Number, Input::new(&self.toast_ms), cx))
                    .child(widgets::row_with("Position", None::<&str>, Slot::Hug, widgets::choice("toast-pos", placement, cx), cx)),
            )
            .child(
                widgets::section("Sounds", cx)
                    .child(self.sw(
                        "snd-capture",
                        "Play a sound after capture",
                        None,
                        g.play_sound_after_capture,
                        |this, v| this.draft.defaults.general.play_sound_after_capture = v,
                        cx,
                    ))
                    .child(self.sw(
                        "snd-task",
                        "Play a sound when a task finishes or fails",
                        None,
                        g.play_sound_after_task,
                        |this, v| this.draft.defaults.general.play_sound_after_task = v,
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Capture sound",
                        Some("A sound file. Leave empty for the built-in one."),
                        Slot::Field,
                        Input::new(&self.sound_capture),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Task finished sound",
                        Some("Leave empty for the built-in one."),
                        Slot::Field,
                        Input::new(&self.sound_task),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Error sound",
                        Some("Leave empty for the built-in one."),
                        Slot::Field,
                        Input::new(&self.sound_error),
                        cx,
                    )),
            )
            .child(widgets::section("Updates", cx).child(widgets::action_row("Updates", Some(update_note), self.updates_row(cx), cx)))
    }
    fn updates_row(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().items_center().gap(space::S2).child(widgets::pill(
            "update-check",
            "Check now",
            Pill::Default,
            false,
            cx.listener(|this, _, _, cx| {
                this.update = Some(("Checking…".into(), false));
                cx.notify();
                let check = cx.global::<UiState>().services.check_update.clone();
                let task = cx.background_spawn(async move { check().await });
                cx.spawn(async move |this, cx| {
                    let r = task.await;
                    let _ = this.update(cx, |this, cx| {
                        this.update = Some(r.unwrap_or_else(|e| (format!("Couldn't check for updates: {e}"), false)));
                        cx.notify();
                    });
                })
                .detach();
            }),
            cx,
        ));
        if let Some((_, true)) = &self.update {
            row = row.child(widgets::pill(
                "update-install",
                "Install and restart",
                Pill::Primary,
                false,
                cx.listener(|this, _, _, cx| {
                    this.update = Some(("Downloading the update…".into(), false));
                    cx.notify();
                    let install = cx.global::<UiState>().services.install_update.clone();
                    let task = cx.background_spawn(async move { install().await });
                    cx.spawn(async move |this, cx| {
                        let r = task.await;
                        let _ = this.update(cx, |this, cx| {
                            if let Err(e) = r {
                                this.update = Some((format!("Update failed: {e}"), false));
                                cx.notify();
                            }
                        });
                    })
                    .detach();
                }),
                cx,
            ));
        }
        row
    }
    /// A searchable language dropdown; "System" follows the OS locale.
    fn language_select(current: Option<&str>, window: &mut Window, cx: &mut Context<Self>) -> Entity<SelectState<Vec<SharedString>>> {
        let names: Vec<SharedString> = std::iter::once(SharedString::from("System"))
            .chain(crate::i18n::LOCALES.iter().map(|(_, name, _)| SharedString::from(*name)))
            .collect();
        let current_name = current
            .and_then(|tag| crate::i18n::LOCALES.iter().find(|(t, _, _)| *t == tag))
            .map(|(_, name, _)| SharedString::from(*name))
            .unwrap_or_else(|| "System".into());
        let state = cx.new(|cx| {
            let mut state = SelectState::new(names, None, window, cx).searchable(true);
            state.set_selected_value(&current_name, window, cx);
            state
        });
        cx.subscribe_in(&state, window, |this, _, ev: &SelectEvent<Vec<SharedString>>, _, cx| {
            if let SelectEvent::Confirm(Some(name)) = ev {
                this.draft.language = crate::i18n::LOCALES.iter().find(|(_, n, _)| *n == name.as_ref()).map(|(tag, _, _)| tag.to_string());
                cx.notify();
            }
        })
        .detach();
        state
    }

    fn language_picker(&self, _cx: &mut Context<Self>) -> gpui::Div {
        div().w_full().child(Select::new(&self.language).search_placeholder("Search languages"))
    }
    fn rebuild_watch_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let folders = self.draft.watch_folders.clone();
        self.watch = folders
            .iter()
            .map(|f| (text_input(f.folder.display().to_string(), window, cx), text_input(f.filter.clone(), window, cx)))
            .collect();
    }

    fn recording(&self, cx: &mut Context<Self>) -> gpui::Div {
        use laqta_domain::record::VideoCodec;
        let r = &self.draft.defaults.recording;
        let codecs = |gif: bool, this: &Self, cx: &mut Context<Self>| {
            let current = if gif { this.draft.defaults.recording.gif_codec } else { this.draft.defaults.recording.codec };
            let opts = VideoCodec::ALL
                .into_iter()
                .filter(|c| matches!(c, VideoCodec::Gif | VideoCodec::WebP | VideoCodec::Apng) == gif)
                .map(|c| {
                    this.opt(
                        c.label(),
                        current == c,
                        move |this| {
                            if gif {
                                this.draft.defaults.recording.gif_codec = c;
                            } else {
                                this.draft.defaults.recording.codec = c;
                            }
                        },
                        cx,
                    )
                })
                .collect();
            widgets::choice(if gif { "gif-codec" } else { "video-codec" }, opts, cx)
        };
        use laqta_domain::record::{CameraShape as S, Corner as C, RecorderBackend as B};
        let cam = &r.camera;
        let shapes = [(S::Circle, "Circle"), (S::RoundedRectangle, "Rounded"), (S::Rectangle, "Rectangle")]
            .into_iter()
            .map(|(v, l)| self.opt(l, cam.shape == v, move |this| this.draft.defaults.recording.camera.shape = v, cx))
            .collect();
        let corners =
            [(C::TopLeft, "Top left"), (C::TopRight, "Top right"), (C::BottomLeft, "Bottom left"), (C::BottomRight, "Bottom right")]
                .into_iter()
                .map(|(v, l)| self.opt(l, cam.corner == v, move |this| this.draft.defaults.recording.camera.corner = v, cx))
                .collect();
        let backends = [(B::Ffmpeg, "FFmpeg"), (B::Native, "Native (Windows)")]
            .into_iter()
            .map(|(v, l)| self.opt(l, r.backend == v, move |this| this.draft.defaults.recording.backend = v, cx))
            .collect();
        let rec = |f: fn(&mut laqta_domain::record::RecordingSettings, bool)| {
            move |this: &mut Self, v: bool| f(&mut this.draft.defaults.recording, v)
        };
        page()
            .child(
                widgets::section("Video", cx)
                    .child(widgets::row_with("Format", None::<&str>, Slot::Hug, codecs(false, self, cx), cx))
                    .child(widgets::row_with("Frame rate", Some("Frames per second."), Slot::Number, Input::new(&self.rec.fps), cx))
                    .child(widgets::row_with(
                        "Quality",
                        Some("CRF: lower means better quality and bigger files. Leave empty for the format's default."),
                        Slot::Number,
                        Input::new(&self.rec.crf),
                        cx,
                    )),
            )
            .child(
                widgets::section("GIF and animated images", cx)
                    .child(widgets::row_with("Format", None::<&str>, Slot::Hug, codecs(true, self, cx), cx))
                    .child(widgets::row_with("Frame rate", Some("Frames per second."), Slot::Number, Input::new(&self.rec.gif_fps), cx)),
            )
            .child(
                widgets::section("While recording", cx)
                    .child(self.sw("rec-cursor", "Record the mouse cursor", None, r.show_cursor, rec(|r, c| r.show_cursor = c), cx))
                    .child(self.sw(
                        "rec-border",
                        "Show a border around the recorded area",
                        None,
                        r.show_border,
                        rec(|r, c| r.show_border = c),
                        cx,
                    ))
                    .child(self.sw(
                        "rec-highlight",
                        "Highlight the mouse",
                        Some("X11 and Windows."),
                        r.highlight_mouse,
                        rec(|r, c| r.highlight_mouse = c),
                        cx,
                    ))
                    .child(self.sw(
                        "rec-system-audio",
                        "Record system audio",
                        None,
                        r.record_system_audio,
                        rec(|r, c| r.record_system_audio = c),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Microphone",
                        Some("Device name. Leave empty to record no microphone."),
                        Slot::Field,
                        Input::new(&self.rec.microphone),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Countdown",
                        Some("Seconds before recording starts."),
                        Slot::Number,
                        Input::new(&self.rec.countdown),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Stop after",
                        Some("Seconds. 0 records until you stop it."),
                        Slot::Number,
                        Input::new(&self.rec.duration),
                        cx,
                    )),
            )
            .child(
                widgets::section("Camera", cx)
                    .child(self.sw(
                        "rec-camera",
                        "Show the camera over the recording",
                        None,
                        cam.enabled,
                        rec(|r, c| r.camera.enabled = c),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Camera",
                        Some("Device name. Leave empty for the first camera (Linux)."),
                        Slot::Field,
                        Input::new(&self.rec.camera_device),
                        cx,
                    ))
                    .child(widgets::row_with("Shape", None::<&str>, Slot::Hug, widgets::choice("cam-shape", shapes, cx), cx))
                    .child(widgets::row_with("Corner", None::<&str>, Slot::Hug, widgets::choice("cam-corner", corners, cx), cx))
                    .child(widgets::row_with(
                        "Size",
                        Some("Percent of the recording's width."),
                        Slot::Number,
                        Input::new(&self.rec.camera_size),
                        cx,
                    )),
            )
            .child(
                widgets::section("Advanced", cx)
                    .child(widgets::row_with(
                        "Recorder",
                        Some("Native uses Windows' own recorder; FFmpeg works everywhere."),
                        Slot::Hug,
                        widgets::choice("backend", backends, cx),
                        cx,
                    ))
                    .child(widgets::row_with("Extra FFmpeg arguments", None::<&str>, Slot::Field, Input::new(&self.rec.args), cx))
                    .child(widgets::row_with(
                        "FFmpeg location",
                        Some("Leave empty to find it, or download it on first use."),
                        Slot::Field,
                        Input::new(&self.rec.ffmpeg),
                        cx,
                    )),
            )
    }

    fn automation(&self, cx: &mut Context<Self>) -> gpui::Div {
        use laqta_domain::capture::ScrollMethod;
        let running = cx.global::<UiState>().services.app.auto_capture_running();
        let actions = [
            (Action::PrintScreen, "Entire screen"),
            (Action::ActiveMonitor, "Active monitor"),
            (Action::ActiveWindow, "Active window"),
            (Action::CustomRegion, "Custom area"),
            (Action::LastRegion, "Last area"),
        ]
        .into_iter()
        .map(|(a, l)| self.opt(l, self.draft.auto_capture.action == a, move |this| this.draft.auto_capture.action = a, cx))
        .collect();
        let methods = [
            (ScrollMethod::MouseWheel, "Mouse wheel"),
            (ScrollMethod::DownArrow, "Down arrow"),
            (ScrollMethod::PageDown, "Page Down"),
            (ScrollMethod::ScrollMessage, "Scroll command (Windows only)"),
        ]
        .into_iter()
        .map(|(m, l)| {
            let on = self.draft.defaults.capture.scrolling.method == m;
            self.opt(l, on, move |this| this.draft.defaults.capture.scrolling.method = m, cx)
        })
        .collect();
        let toggle = widgets::pill(
            "auto-toggle",
            if running { "Stop" } else { "Start" },
            if running { Pill::Default } else { Pill::Dark },
            false,
            cx.listener(move |this, _, _, cx| {
                if !running {
                    this.save(cx);
                }
                let services = cx.global::<UiState>().services.clone();
                (services.dispatch)(if running { Action::StopAutoCapture } else { Action::StartAutoCapture });
                cx.notify();
            }),
            cx,
        );
        let mut watch = div().flex().flex_col().gap(space::S3);
        for (i, (folder, (dir, filter))) in self.draft.watch_folders.iter().zip(&self.watch).enumerate() {
            watch = watch.child(
                entity_card(cx)
                    .child(widgets::row_with("Folder", None::<&str>, Slot::Field, Input::new(dir), cx))
                    .child(widgets::row_with("Files", Some("Patterns such as *.png;*.jpg."), Slot::Field, Input::new(filter), cx))
                    .child(self.sw_dyn(
                        format!("wf-on-{i}"),
                        "Watch this folder",
                        folder.enabled,
                        move |this, v| this.draft.watch_folders[i].enabled = v,
                        cx,
                    ))
                    .child(self.sw_dyn(
                        format!("wf-sub-{i}"),
                        "Include subfolders",
                        folder.include_subfolders,
                        move |this, v| this.draft.watch_folders[i].include_subfolders = v,
                        cx,
                    ))
                    .child(self.sw_dyn(
                        format!("wf-move-{i}"),
                        "Move files into the screenshots folder",
                        folder.move_to_screenshots,
                        move |this, v| this.draft.watch_folders[i].move_to_screenshots = v,
                        cx,
                    ))
                    .child(div().flex().justify_end().child(widgets::pill(
                        SharedString::from(format!("wf-rm-{i}")),
                        "Remove folder",
                        Pill::Ghost,
                        true,
                        cx.listener(move |this, _, window, cx| {
                            let _ = this.collect(cx);
                            this.draft.watch_folders.remove(i);
                            this.rebuild_watch_inputs(window, cx);
                            cx.notify();
                        }),
                        cx,
                    ))),
            );
        }
        let add_folder = widgets::pill(
            "wf-add",
            "Add folder…",
            Pill::Default,
            false,
            cx.listener(|_this, _, _, cx| {
                let task =
                    cx.background_spawn(async move { rfd::AsyncFileDialog::new().pick_folder().await.map(|f| f.path().to_path_buf()) });
                cx.spawn(async move |this, cx| {
                    if let Some(folder) = task.await {
                        let _ = this.update_in(cx, |this, window, cx| {
                            let _ = this.collect(cx);
                            this.draft.watch_folders.push(laqta_domain::settings::WatchFolder { folder, ..Default::default() });
                            this.rebuild_watch_inputs(window, cx);
                            cx.notify();
                        });
                    }
                })
                .detach();
            }),
            cx,
        );
        page()
            .child(
                widgets::section("Timed capture", cx)
                    .child(widgets::row_with("Capture", None::<&str>, Slot::Hug, widgets::choice("auto-action", actions, cx), cx))
                    .child(widgets::row_with("Every", Some("Seconds between captures."), Slot::Number, Input::new(&self.auto_interval), cx))
                    .child(widgets::row_with(
                        "Stop after",
                        Some("Number of captures. 0 keeps going until you stop it."),
                        Slot::Number,
                        Input::new(&self.auto_count),
                        cx,
                    ))
                    .child(widgets::action_row(
                        if running { "Timed capture is running" } else { "Timed capture is off" },
                        Some(if running { "Stop it here, from the tray or with its hotkey." } else { "Start saves these settings first." }),
                        toggle,
                        cx,
                    )),
            )
            .child(
                widgets::section("Scrolling capture", cx)
                    .child(
                        widgets::muted(
                            "Select the area to scroll; Laqta scrolls it and stitches the frames. On Wayland you scroll yourself.",
                            cx,
                        )
                        .pb(space::S2),
                    )
                    .child(widgets::row_with("Scroll with", None::<&str>, Slot::Hug, widgets::choice("scroll-method", methods, cx), cx))
                    .child(widgets::row_with(
                        "Wait before the first frame",
                        Some("Milliseconds."),
                        Slot::Number,
                        Input::new(&self.scroll[0]),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Wait after each scroll",
                        Some("Milliseconds."),
                        Slot::Number,
                        Input::new(&self.scroll[1]),
                        cx,
                    ))
                    .child(widgets::row_with("Stop after", Some("Number of scrolls."), Slot::Number, Input::new(&self.scroll[2]), cx))
                    .child(widgets::row_with("Steps per scroll", None::<&str>, Slot::Number, Input::new(&self.scroll[3]), cx))
                    .child(widgets::row_with(
                        "Ignore a fixed header",
                        Some("Pixels at the top that don't scroll, like a sticky menu bar."),
                        Slot::Number,
                        Input::new(&self.scroll[4]),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Ignore a fixed footer",
                        Some("Pixels at the bottom that don't scroll."),
                        Slot::Number,
                        Input::new(&self.scroll[5]),
                        cx,
                    )),
            )
            .child(
                widgets::section("Watch folders", cx)
                    .child(widgets::muted("New files in these folders are uploaded with your upload settings.", cx).pb(space::S2))
                    .child(watch)
                    .child(div().pt(space::S3).child(add_folder)),
            )
    }

    /// A switch row whose id is built at run time (one per list item).
    fn sw_dyn(
        &self,
        id: String,
        label: &'static str,
        on: bool,
        set: impl Fn(&mut Self, bool) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        widgets::switch_row(
            SharedString::from(id),
            label,
            None::<&str>,
            on,
            cx.listener(move |this, v: &bool, _, cx| {
                set(this, *v);
                cx.notify();
            }),
            cx,
        )
    }
    fn models_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        use laqta_domain::settings::{BackgroundProvider, ModelProvider};
        let providers = [(ModelProvider::Auto, "Automatic"), (ModelProvider::Cpu, "CPU"), (ModelProvider::Gpu, "GPU")]
            .into_iter()
            .map(|(p, l)| self.opt(l, self.draft.models.provider == p, move |this| this.draft.models.provider = p, cx))
            .collect();
        let mut page = page().child(widgets::section("Processing", cx).child(widgets::row_with(
            "Run models on",
            Some("This build ships the CPU version of ONNX Runtime, so every model runs on the CPU for now."),
            Slot::Hug,
            widgets::choice("provider", providers, cx),
            cx,
        )));
        for (task, label) in [
            ("ocr-det", "Text detection"),
            ("ocr-rec", "Text recognition"),
            ("background", "Background removal"),
            ("inpaint", "Object removal (photo editor)"),
        ] {
            let rows: Vec<_> = self.model_rows.iter().filter(|r| r.task == task).cloned().collect();
            let mut section = widgets::section(label, cx);
            if rows.is_empty() {
                section = section.child(widgets::muted("No models for this in this build.", cx));
            }
            let chosen = match task {
                "ocr-det" => self.draft.models.ocr_det.clone(),
                "ocr-rec" => self.draft.models.ocr_rec.clone(),
                "background" => self.draft.models.background.clone(),
                _ => None,
            };
            let choosable = rows.len() > 1;
            for (i, row) in rows.into_iter().enumerate() {
                let id = row.id.clone();
                let selected = chosen.as_deref() == Some(id.as_str()) || (chosen.is_none() && i == 0);
                let size = crate::presenters::history::format_size(row.size_bytes);
                let meta = format!(
                    "{size}{}{}",
                    if row.licence.is_empty() { String::new() } else { format!(" · {}", row.licence) },
                    if row.commercial { "" } else { " · non-commercial" }
                );
                let mut controls = div().flex().items_center().gap(space::S2);
                if let Some(st) = self.model_status.get(&id) {
                    controls = controls.child(widgets::muted(st.clone(), cx));
                }
                if selected && choosable {
                    controls = controls.child(widgets::badge("In use", Tone::Success, cx));
                } else if choosable {
                    controls = controls.child(widgets::pill(
                        SharedString::from(format!("use-{id}")),
                        "Use",
                        Pill::Ghost,
                        true,
                        cx.listener({
                            let id = id.clone();
                            let task = task.to_owned();
                            move |this, _, _, cx| {
                                let slot = match task.as_str() {
                                    "ocr-det" => &mut this.draft.models.ocr_det,
                                    "ocr-rec" => &mut this.draft.models.ocr_rec,
                                    "background" => &mut this.draft.models.background,
                                    // One object-removal model: nothing to choose.
                                    _ => return,
                                };
                                *slot = Some(id.clone());
                                cx.notify();
                            }
                        }),
                        cx,
                    ));
                }
                controls = if row.installed {
                    controls.child(widgets::pill(
                        SharedString::from(format!("rm-{id}")),
                        "Remove",
                        Pill::Default,
                        true,
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| {
                                let services = cx.global::<UiState>().services.clone();
                                if let Err(e) = services.models.remove(&id) {
                                    this.model_status.insert(id.clone(), e);
                                }
                                if row.custom {
                                    this.draft.models.custom.retain(|c| c.id != id);
                                }
                                this.model_rows = services.models.rows();
                                cx.notify();
                            }
                        }),
                        cx,
                    ))
                } else if row.available {
                    controls.child(widgets::pill(
                        SharedString::from(format!("dl-{id}")),
                        format!("Download ({size})"),
                        Pill::Default,
                        true,
                        cx.listener({
                            let id = id.clone();
                            move |this, _, _, cx| this.download_model(id.clone(), cx)
                        }),
                        cx,
                    ))
                } else {
                    controls.child(widgets::muted("Not available yet", cx))
                };
                section = section.child(widgets::row_with(row.name.clone(), Some(meta), Slot::Hug, controls, cx));
            }
            page = page.child(section);
        }
        let families = cx.global::<UiState>().services.models.families();
        let fam = families
            .iter()
            .enumerate()
            .map(|(i, (task, family))| {
                self.opt(format!("{family} ({task})"), self.custom_family == i, move |this| this.custom_family = i, cx)
            })
            .collect();
        let bg = [(BackgroundProvider::Local, "On this computer"), (BackgroundProvider::RemoveBg, "remove.bg")]
            .into_iter()
            .map(|(b, l)| self.opt(l, self.draft.cloud.background == b, move |this| this.draft.cloud.background = b, cx))
            .collect();
        page.child(
            widgets::section("Your own model", cx)
                .child(widgets::muted("Add an ONNX file for a supported model family. Laqta checks its inputs and outputs before using it.", cx).pb(space::S2))
                .child(widgets::row_with("Family", None::<&str>, Slot::Field, widgets::choice("family", fam, cx), cx))
                .child(widgets::row_with("Name", None::<&str>, Slot::Field, Input::new(&self.custom_name), cx))
                .child(widgets::action_row(
                    "Model file",
                    Some("Pick the .onnx file."),
                    widgets::pill("custom-add", "Choose file…", Pill::Default, false, cx.listener(move |this, _, _, cx| this.add_custom_model(cx)), cx),
                    cx,
                )),
        )
        .child(
            widgets::section("Background removal service", cx)
                .child(widgets::row_with("Remove backgrounds", None::<&str>, Slot::Hug, widgets::choice("bg-provider", bg, cx), cx))
                .child(widgets::row_with("remove.bg API key", Some("Only needed for remove.bg."), Slot::Field, Input::new(&self.cloud[4]), cx)),
        )
        .child(
            widgets::section("Describe image with AI", cx)
                .child(
                    widgets::muted(
                        "Sends the capture to an OpenAI-compatible service: OpenAI, Gemini, OpenRouter, or a local Ollama or LM Studio server.",
                        cx,
                    )
                    .pb(space::S2),
                )
                .child(widgets::row_with("Service address", Some("The base URL, such as http://localhost:11434/v1."), Slot::Field, Input::new(&self.cloud[0]), cx))
                .child(widgets::row_with("Model", None::<&str>, Slot::Field, Input::new(&self.cloud[1]), cx))
                .child(widgets::row_with("Prompt", None::<&str>, Slot::Field, Input::new(&self.cloud[2]), cx))
                .child(widgets::row_with("API key", Some("Leave empty for local servers."), Slot::Field, Input::new(&self.cloud[3]), cx)),
        )
    }

    fn download_model(&mut self, id: String, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<(u64, Option<u64>)>();
        let progress: crate::runtime::ModelProgress = std::sync::Arc::new(move |a, b| {
            let _ = tx.unbounded_send((a, b));
        });
        let task = services.models.install(id.clone(), progress);
        self.model_status.insert(id.clone(), "Starting…".into());
        cx.notify();
        let pid = id.clone();
        cx.spawn(async move |this, cx| {
            use futures_lite::StreamExt as _;
            while let Some((a, b)) = rx.next().await {
                let msg = match b {
                    Some(t) if t > 0 => format!("{}%", a * 100 / t),
                    _ => crate::presenters::history::format_size(a),
                };
                if this
                    .update(cx, |this, cx| {
                        this.model_status.insert(pid.clone(), msg);
                        cx.notify();
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
        .detach();
        let bg = cx.background_spawn(task);
        cx.spawn(async move |this, cx| {
            let r = bg.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(()) => {
                        this.model_status.remove(&id);
                    }
                    Err(e) => {
                        this.model_status.insert(id.clone(), format!("Failed: {e}"));
                    }
                }
                this.model_rows = cx.global::<UiState>().services.models.rows();
                cx.notify();
            });
        })
        .detach();
    }

    fn add_custom_model(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let families = services.models.families();
        let Some((_, family)) = families.get(self.custom_family).cloned() else { return };
        let name = Self::value(&self.custom_name, cx);
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter("ONNX model", &["onnx"]).pick_file().await else { return Ok(None) };
            services.models.add_custom(file.path().to_path_buf(), family, name).await.map(Some)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(model)) => {
                        this.draft.models.custom.retain(|c| c.id != model.id);
                        this.draft.models.custom.push(model);
                        this.report(Ok("Custom model added. Save to keep it.".into()), cx);
                    }
                    Ok(None) => {}
                    Err(e) => this.report(Err(format!("Custom model rejected: {e}")), cx),
                }
                this.model_rows = cx.global::<UiState>().services.models.rows();
                cx.notify();
            });
        })
        .detach();
    }

    fn integration(&self, cx: &mut Context<Self>) -> gpui::Div {
        let Some((autostart, shell)) = self.integration_status.clone() else {
            return page().child(widgets::section("Startup", cx).child(widgets::row_with(
                "Checking…",
                None::<String>,
                Slot::Hug,
                div(),
                cx,
            )));
        };
        let startup = match autostart {
            Ok(on) => widgets::switch_row(
                "autostart",
                "Start Laqta when I sign in",
                Some("Laqta starts in the tray. Applies now."),
                on,
                cx.listener(|this, c: &bool, _, cx| {
                    let services = cx.global::<UiState>().services.clone();
                    let r = std::env::current_exe()
                        .map_err(|e| e.to_string())
                        .and_then(|exe| services.autostart.set_enabled(*c, &exe).map_err(|e| e.to_string()));
                    this.report(
                        r.map(
                            |()| if *c { "Laqta will start when you sign in.".into() } else { "Laqta will not start at sign-in.".into() },
                        ),
                        cx,
                    );
                    this.refresh_integration(cx);
                }),
                cx,
            ),
            Err(e) => widgets::row_with("Start Laqta when I sign in", Some(format!("Not available here: {e}")), Slot::Hug, div(), cx),
        };
        let file_manager = match shell {
            Ok(installed) => widgets::action_row(
                "“Upload with Laqta” menu item",
                Some("Adds it to the file manager's right-click menu and to Send To. Applies now."),
                widgets::pill(
                    "shell-toggle",
                    if installed { "Remove" } else { "Add" },
                    Pill::Default,
                    false,
                    cx.listener(move |this, _, _, cx| {
                        let services = cx.global::<UiState>().services.clone();
                        if installed {
                            let r = services.integration.uninstall().map_err(|e| e.to_string());
                            this.report(r.map(|()| "Removed from the file manager.".into()), cx);
                        } else {
                            let r = std::env::current_exe()
                                .map_err(|e| e.to_string())
                                .and_then(|exe| services.integration.install(&exe).map_err(|e| e.to_string()));
                            this.report(r.map(|()| "Added to the file manager. Some file managers show it after a restart.".into()), cx);
                        }
                        this.refresh_integration(cx);
                    }),
                    cx,
                ),
                cx,
            ),
            Err(e) => widgets::row_with("“Upload with Laqta” menu item", Some(format!("Not available here: {e}")), Slot::Hug, div(), cx),
        };
        page().child(widgets::section("Startup", cx).child(startup)).child(widgets::section("File manager", cx).child(file_manager)).child(
            widgets::section("Browser", cx).child(widgets::action_row(
                "Browser extension",
                Some("Lets the ShareX browser extension (Chrome, Firefox) send pages, images and links to Laqta."),
                widgets::pill(
                    "nmh",
                    "Connect",
                    Pill::Default,
                    false,
                    cx.listener(|this, _, _, cx| {
                        let services = cx.global::<UiState>().services.clone();
                        let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                            let nmh = exe.with_file_name(if cfg!(windows) { "laqta-nmh.exe" } else { "laqta-nmh" });
                            if !nmh.exists() {
                                return Err(format!("{} is missing", nmh.display()));
                            }
                            services.integration.install_native_messaging(&nmh).map_err(|e| e.to_string())
                        });
                        this.report(r.map(|()| "Browser extension connected.".into()), cx);
                    }),
                    cx,
                ),
                cx,
            )),
        )
    }
    fn data(&self, cx: &mut Context<Self>) -> gpui::Div {
        let two = |a: AnyElement, b: AnyElement| div().flex().gap(space::S2).child(a).child(b);
        page()
            .child(
                widgets::section("Backup", cx).child(widgets::action_row(
                    "Settings backup",
                    Some("Your settings, history and custom uploaders in one archive."),
                    two(
                        widgets::pill("export", "Export…", Pill::Default, false, cx.listener(|this, _, _, cx| this.export_archive(cx)), cx)
                            .into_any_element(),
                        widgets::pill("import", "Import…", Pill::Default, false, cx.listener(|this, _, _, cx| this.import_archive(cx)), cx)
                            .into_any_element(),
                    ),
                    cx,
                )),
            )
            .child(
                widgets::section("Import from ShareX", cx)
                    .child(widgets::action_row(
                        "ShareX setup",
                        Some("Shortcuts, custom uploaders, accounts, effect presets and history. Pick the ShareX folder (Documents\\ShareX) or a .sxb backup."),
                        two(
                            widgets::pill("sharex-folder", "Folder…", Pill::Default, false, cx.listener(|this, _, _, cx| this.import_sharex_config(true, cx)), cx)
                                .into_any_element(),
                            widgets::pill("sharex-backup", "Backup…", Pill::Default, false, cx.listener(|this, _, _, cx| this.import_sharex_config(false, cx)), cx)
                                .into_any_element(),
                        ),
                        cx,
                    ))
                    .child(widgets::action_row(
                        "ShareX history only",
                        Some("The capture history file (History.json)."),
                        widgets::pill("sharex-history", "Choose file…", Pill::Default, false, cx.listener(|this, _, _, cx| this.import_sharex(cx)), cx),
                        cx,
                    )),
            )
    }
    fn import_sharex_config(&mut self, folder: bool, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let task = cx.background_spawn(async move {
            let picked = if folder {
                rfd::AsyncFileDialog::new().set_title("ShareX folder").pick_folder().await
            } else {
                rfd::AsyncFileDialog::new().add_filter("ShareX backup", &["sxb"]).pick_file().await
            };
            let Some(path) = picked else { return Ok(None) };
            (services.import_sharex)(path.path().to_path_buf()).map(Some)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(summary)) => {
                    this.draft = cx.global::<UiState>().services.app.settings();
                    this.report(Ok(summary), cx);
                }
                Ok(None) => {}
                Err(e) => this.report(Err(format!("Import failed: {e}")), cx),
            });
        })
        .detach();
    }

    fn rebuild_host_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let accounts = self.draft.hosts.clone();
        self.hosts = accounts
            .iter()
            .map(|a| {
                a.kind
                    .fields()
                    .iter()
                    .map(|f| {
                        let v = if f.secret { String::new() } else { a.get(f.key).to_string() };
                        let input = cx.new(|cx| {
                            let st = InputState::new(window, cx).default_value(v);
                            if f.secret { st.placeholder("(stored in the system keyring; type to replace)") } else { st }
                        });
                        (f.key, f.secret, input)
                    })
                    .collect()
            })
            .collect();
    }

    fn uploads(&self, _window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        let services = cx.global::<UiState>().services.clone();
        let u = &self.draft.defaults.upload;
        let destinations = services.uploaders.destination_list();
        let mut dest = widgets::section("Destinations", cx);
        for (kind, label, description) in [
            (DestKind::Image, "Images", "Captures and image files."),
            (DestKind::Text, "Text", "Text and code you upload."),
            (DestKind::File, "Other files", "Everything else."),
            (DestKind::Shortener, "Link shortener", "Used when “Shorten the link” is on."),
            (DestKind::Sharer, "Share to", "Used when “Share the link” is on."),
        ] {
            let current = match kind {
                DestKind::Image => u.image_destination.clone(),
                DestKind::Text => u.text_destination.clone(),
                DestKind::File => u.file_destination.clone(),
                DestKind::Shortener => u.url_shortener.clone(),
                DestKind::Sharer => u.url_sharer.clone(),
            };
            let choices: Vec<(String, String)> = match kind {
                DestKind::Shortener => present::shortener_choices(&destinations, &self.draft.hosts),
                DestKind::Sharer => present::SHARERS.iter().map(|(id, n)| (id.to_string(), n.to_string())).collect(),
                _ => destinations.clone(),
            };
            let opts = choices
                .into_iter()
                .map(|(id, name)| {
                    let selected = id == current;
                    self.opt(
                        name,
                        selected,
                        move |this| {
                            let u = &mut this.draft.defaults.upload;
                            match kind {
                                DestKind::Image => u.image_destination = id.clone(),
                                DestKind::Text => u.text_destination = id.clone(),
                                DestKind::File => u.file_destination = id.clone(),
                                DestKind::Shortener => u.url_shortener = id.clone(),
                                DestKind::Sharer => u.url_sharer = id.clone(),
                            }
                        },
                        cx,
                    )
                })
                .collect();
            dest = dest.child(widgets::row_with(
                label,
                Some(description),
                Slot::Field,
                widgets::dropdown(format!("dest-{label}"), opts, cx),
                cx,
            ));
        }
        let mut after = widgets::section("After upload", cx);
        for (flag, label) in present::after_upload_options() {
            after = after.child(self.sw_dyn(
                format!("au-{}", flag.bits()),
                label,
                self.draft.defaults.after_upload.contains(flag),
                move |this, v| this.draft.defaults.after_upload.set(flag, v),
                cx,
            ));
        }
        after = after
            .child(widgets::row_with(
                "Copied text",
                Some("What “Copy the link” puts on the clipboard. $url is the link; $shorturl and $deletionurl also work."),
                Slot::Field,
                Input::new(&self.clipboard_format),
                cx,
            ))
            .child(widgets::row_with("Format", None::<&str>, Slot::Hug, self.clipboard_presets(cx), cx))
            .child(widgets::row_with(
                "Retries",
                Some("How many times to retry after a network error."),
                Slot::Number,
                Input::new(&self.retries),
                cx,
            ));
        let mut accounts = widgets::section("Accounts", cx);
        if self.draft.hosts.is_empty() {
            accounts =
                accounts.child(widgets::muted("Add an account for hosts that need a sign-in, such as S3, FTP or Imgur.", cx).pb(space::S2));
        }
        for (i, (account, inputs)) in self.draft.hosts.iter().zip(&self.hosts).enumerate() {
            let mut card = entity_card(cx).child(
                div()
                    .flex()
                    .items_center()
                    .pb(space::S1)
                    .child(div().flex_1().text_size(text::BODY).font_weight(FontWeight::MEDIUM).child(account.kind.label()))
                    .child(widgets::pill(
                        SharedString::from(format!("host-rm-{i}")),
                        "Remove",
                        Pill::Ghost,
                        true,
                        cx.listener(move |this, _, window, cx| {
                            if i < this.draft.hosts.len() {
                                this.draft.hosts.remove(i);
                                this.rebuild_host_inputs(window, cx);
                            }
                            cx.notify();
                        }),
                        cx,
                    )),
            );
            for ((_key, _secret, input), field) in inputs.iter().zip(account.kind.fields()) {
                card = card.child(widgets::row_with(field.label, None::<&str>, Slot::Field, Input::new(input), cx));
            }
            accounts = accounts.child(card);
        }
        let add_opts = laqta_domain::hosts::HostKind::ALL
            .into_iter()
            .map(|kind| {
                widgets::Choice::new(
                    kind.label(),
                    false,
                    cx.listener(move |this, _, window, cx| {
                        if this.collect(cx).is_ok() {
                            this.draft.hosts.push(laqta_domain::hosts::HostAccount::new(kind));
                            this.rebuild_host_inputs(window, cx);
                        }
                        cx.notify();
                    }),
                )
            })
            .collect();
        accounts = accounts.child(div().pt(space::S3).child(widgets::menu_button("host-add", "Add account", add_opts)));
        page()
            .child(widgets::section("Upload", cx).child(self.sw(
                "upload-after-capture",
                "Upload after capture",
                Some("Sends each capture to the image destination below."),
                self.draft.defaults.after_capture.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST),
                |this, v| this.draft.defaults.after_capture.set(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST, v),
                cx,
            )))
            .child(dest)
            .child(after)
            .child(accounts)
            .child(
                widgets::section("More", cx)
                    .child(widgets::action_row(
                        "Custom uploader",
                        Some("Add a host from a .sxcu file."),
                        widgets::pill(
                            "import-sxcu",
                            "Import…",
                            Pill::Default,
                            false,
                            cx.listener(|this, _, _, cx| this.import_sxcu(cx)),
                            cx,
                        ),
                        cx,
                    ))
                    .child(widgets::action_row(
                        "Remote storage",
                        Some("Browse and manage files on S3, FTP or SFTP."),
                        widgets::pill("remote-browser", "Browse…", Pill::Default, false, |_, _, cx| crate::remote_window::open(cx), cx),
                        cx,
                    )),
            )
    }
    fn clipboard_presets(&self, cx: &mut Context<Self>) -> AnyElement {
        let current = Self::value(&self.clipboard_format, cx);
        let opts = present::CLIPBOARD_FORMATS
            .into_iter()
            .map(|(label, template)| {
                widgets::Choice::new(
                    label,
                    current == template,
                    cx.listener(move |this, _, window, cx| {
                        this.clipboard_format.update(cx, |input, cx| input.set_value(template, window, cx));
                        cx.notify();
                    }),
                )
            })
            .collect();
        widgets::dropdown("clip-format", opts, cx)
    }
    fn import_sxcu(&mut self, cx: &mut Context<Self>) {
        let add = cx.global::<UiState>().services.add_custom_uploader.clone();
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter("Custom uploader", &["sxcu", "json"]).pick_file().await else {
                return Ok(None);
            };
            let json = std::fs::read_to_string(file.path()).map_err(|e| e.to_string())?;
            add(json).map(Some)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(id)) => this.report(Ok(format!("Added {id}. Pick it under Uploads.")), cx),
                Ok(None) => {}
                Err(e) => this.report(Err(format!("Couldn't add the uploader: {e}")), cx),
            });
        })
        .detach();
    }

    fn report(&mut self, result: Result<String, String>, cx: &mut Context<Self>) {
        self.message = Some(match result {
            Ok(m) => (true, m),
            Err(e) => (false, e),
        });
        cx.notify();
    }

    fn export_archive(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().set_file_name("Laqta-backup.zip").add_filter("Zip", &["zip"]).save_file().await
            else {
                return Ok(None);
            };
            laqta_library::export_archive(&services.data_dir, file.path(), services.history.as_deref())
                .map(|_| Some(file.path().display().to_string()))
                .map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(p)) => this.report(Ok(format!("Exported to {p}.")), cx),
                Ok(None) => {}
                Err(e) => this.report(Err(format!("Export failed: {e}")), cx),
            });
        })
        .detach();
    }

    fn import_archive(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter("Zip", &["zip"]).pick_file().await else {
                return Ok(None);
            };
            laqta_library::import_archive(file.path(), &services.data_dir).map(|written| Some(written.len())).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(n)) => this.report(Ok(format!("Imported {n} files. Restart Laqta to use them.")), cx),
                Ok(None) => {}
                Err(e) => this.report(Err(format!("Import failed: {e}")), cx),
            });
        })
        .detach();
    }

    fn import_sharex(&mut self, cx: &mut Context<Self>) {
        let Some(history) = cx.global::<UiState>().services.history.clone() else {
            self.report(Err("History is turned off.".into()), cx);
            return;
        };
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter("ShareX history", &["json"]).pick_file().await else {
                return Ok(None);
            };
            laqta_library::import_sharex_history_file(&history, file.path()).map(Some).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(rep)) => this
                    .report(Ok(format!("Imported {} items ({} already there, {} unreadable).", rep.imported, rep.skipped, rep.failed)), cx),
                Ok(None) => {}
                Err(e) => this.report(Err(format!("Import failed: {e}")), cx),
            });
        })
        .detach();
    }

    fn image(&self, cx: &mut Context<Self>) -> gpui::Div {
        let formats = ImageFormat::ALL
            .into_iter()
            .map(|f| {
                self.opt(
                    present::format_label(f),
                    self.draft.defaults.image.format == f,
                    move |this| this.draft.defaults.image.format = f,
                    cx,
                )
            })
            .collect();
        let exists = [
            (FileExistsAction::AppendNumber, "Add a number"),
            (FileExistsAction::Overwrite, "Overwrite"),
            (FileExistsAction::Skip, "Skip"),
        ]
        .into_iter()
        .map(|(a, l)| self.opt(l, self.draft.defaults.image.file_exists == a, move |this| this.draft.defaults.image.file_exists = a, cx))
        .collect();
        page()
            .child(
                widgets::section("Format", cx)
                    .child(widgets::row_with("Save as", None::<&str>, Slot::Field, widgets::dropdown("format", formats, cx), cx))
                    .child(widgets::row_with(
                        "JPEG quality",
                        Some("1 to 100. Higher looks better and makes bigger files."),
                        Slot::Number,
                        Input::new(&self.jpeg_quality),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Switch to JPEG above",
                        Some("Kilobytes. A PNG bigger than this is saved as JPEG instead. Leave empty to never switch."),
                        Slot::Number,
                        Input::new(&self.auto_jpeg),
                        cx,
                    ))
                    .child(widgets::row_with("If the file exists", None::<&str>, Slot::Hug, widgets::choice("exists", exists, cx), cx)),
            )
            .child(
                widgets::section("Thumbnails", cx)
                    .child(widgets::row_with("Width", Some("Pixels."), Slot::Number, Input::new(&self.thumb_w), cx))
                    .child(widgets::row_with(
                        "Height",
                        Some("Pixels. 0 keeps the aspect ratio."),
                        Slot::Number,
                        Input::new(&self.thumb_h),
                        cx,
                    )),
            )
    }
    fn naming(&self, cx: &mut Context<Self>) -> gpui::Div {
        let services = &cx.global::<UiState>().services;
        let (now, host) = (services.clock.now(), services.host.clone());
        let preview = |input: &Entity<InputState>, cx: &App| present::name_preview(&Self::value(input, cx), now, &host);
        let (p1, p2, p3) = (preview(&self.pattern, cx), preview(&self.window_pattern, cx), preview(&self.subfolder, cx));
        let token = |group: &'static str, items: &'static str| {
            div()
                .flex()
                .gap(space::S3)
                .py(px(6.))
                .child(div().w(px(140.)).flex_none().text_size(text::LABEL).font_weight(FontWeight::MEDIUM).child(group))
                .child(div().flex_1().text_size(text::LABEL).text_color(cx.theme().muted_foreground).child(items))
        };
        page()
            .child(
                widgets::section("Names", cx)
                    .child(widgets::row_with("File name", Some(format!("Preview: {p1}")), Slot::Field, Input::new(&self.pattern), cx))
                    .child(widgets::row_with("File name for window captures", Some(format!("Preview: {p2}")), Slot::Field, Input::new(&self.window_pattern), cx))
                    .child(widgets::row_with("Subfolder", Some(format!("Preview: {p3}")), Slot::Field, Input::new(&self.subfolder), cx)),
            )
            .child(
                widgets::section("Tokens you can use", cx)
                    .child(token("Date and time", "%y year · %mo month · %d day · %h hour · %mi minute · %s second · %ms millisecond · %unix Unix time · also %yy %mon %mon2 %pm %w %w2 %wy"))
                    .child(token("Window", "%t window title · %pn process name"))
                    .child(token("Counter", "%i · also %ia %iAa %ib %ix"))
                    .child(token("Random", "%rn number · %ra letters and digits · %guid · %radjective · %ranimal · %remoji · also %rna %rx %rf"))
                    .child(token("Image and system", "%width · %height · %un user name · %cn computer name · also %uln %n"))
                    .child(widgets::muted("Add {n} to repeat a random token or pad a counter, for example %rn{4}.", cx).pt(space::S2)),
            )
    }
    fn capture(&self, cx: &mut Context<Self>) -> gpui::Div {
        let c = &self.draft.defaults.capture;
        let r = &c.region;
        page()
            .child(
                widgets::section("Capture", cx)
                    .child(widgets::row_with(
                        "Delay",
                        Some("Milliseconds to wait before capturing."),
                        Slot::Number,
                        Input::new(&self.delay),
                        cx,
                    ))
                    .child(self.sw(
                        "cursor",
                        "Show the mouse cursor",
                        None,
                        c.show_cursor,
                        |this, v| this.draft.defaults.capture.show_cursor = v,
                        cx,
                    ))
                    .child(self.sw(
                        "transparent",
                        "Keep window transparency",
                        Some("Keeps rounded corners and shadows when capturing a window."),
                        c.transparent_window,
                        |this, v| this.draft.defaults.capture.transparent_window = v,
                        cx,
                    )),
            )
            .child(
                widgets::section("Selection screen", cx)
                    .child(self.sw(
                        "tools-after-select",
                        "Show tools after selecting",
                        Some("Keep the selection open with drawing tools and Copy, Save, Pin and Copy text. Off: capture as soon as you release the mouse. Space hides the tools."),
                        r.show_tools_after_select,
                        |this, v| this.draft.defaults.capture.region.show_tools_after_select = v,
                        cx,
                    ))
                    .child(self.sw(
                        "mode-bar",
                        "Show the mode bar",
                        Some("The bar at the top of the screen: screenshot or video, full screen, monitor, edit after capture, colour picker and text capture."),
                        r.show_mode_bar,
                        |this, v| this.draft.defaults.capture.region.show_mode_bar = v,
                        cx,
                    ))
                    .child(self.sw(
                        "multi",
                        "Allow several regions",
                        Some("Select more than one area in one capture."),
                        r.multi_region,
                        |this, v| this.draft.defaults.capture.region.multi_region = v,
                        cx,
                    ))
                    .child(self.sw(
                        "snap",
                        "Snap to windows",
                        None,
                        r.snap_to_windows,
                        |this, v| this.draft.defaults.capture.region.snap_to_windows = v,
                        cx,
                    ))
                    .child(self.sw(
                        "regions",
                        "Detect regions inside windows",
                        Some("Hover a sidebar, panel or text box to select just that part. Scroll to grow to the enclosing one."),
                        r.detect_regions,
                        |this, v| this.draft.defaults.capture.region.detect_regions = v,
                        cx,
                    ))
                    .child(self.sw(
                        "magnifier",
                        "Show the magnifier",
                        None,
                        r.show_magnifier,
                        |this, v| this.draft.defaults.capture.region.show_magnifier = v,
                        cx,
                    ))
                    .child(self.sw(
                        "crosshair",
                        "Show crosshair lines",
                        None,
                        r.show_crosshair,
                        |this, v| this.draft.defaults.capture.region.show_crosshair = v,
                        cx,
                    ))
                    .child(self.sw(
                        "info",
                        "Show position and colour",
                        None,
                        r.show_info,
                        |this, v| this.draft.defaults.capture.region.show_info = v,
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Right-click",
                        None::<&str>,
                        Slot::Field,
                        widgets::dropdown(
                            "rc",
                            self.mouse_actions(r.right_click, |s, a| s.defaults.capture.region.right_click = a, cx),
                            cx,
                        ),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Middle-click",
                        None::<&str>,
                        Slot::Field,
                        widgets::dropdown(
                            "mc",
                            self.mouse_actions(r.middle_click, |s, a| s.defaults.capture.region.middle_click = a, cx),
                            cx,
                        ),
                        cx,
                    )),
            )
            .child(
                widgets::section("Fixed areas", cx)
                    .child(widgets::row_with(
                        "Custom area",
                        Some("x, y, width, height in pixels, used by “Capture custom region”."),
                        Slot::Field,
                        Input::new(&self.custom_region),
                        cx,
                    ))
                    .child(widgets::row_with(
                        "Window title",
                        Some("Used by “Capture window by title”."),
                        Slot::Field,
                        Input::new(&self.window_title),
                        cx,
                    )),
            )
    }
    fn mouse_actions(
        &self,
        current: RegionMouseAction,
        set: fn(&mut Settings, RegionMouseAction),
        cx: &mut Context<Self>,
    ) -> Vec<widgets::Choice> {
        [
            (RegionMouseAction::None, "Do nothing"),
            (RegionMouseAction::CancelCapture, "Cancel the capture"),
            (RegionMouseAction::RemoveShapeOrCancel, "Remove the region, or cancel"),
            (RegionMouseAction::RemoveShape, "Remove the region"),
            (RegionMouseAction::CaptureFullscreen, "Capture the whole screen"),
            (RegionMouseAction::CaptureActiveMonitor, "Capture the monitor"),
            (RegionMouseAction::ConfirmSelection, "Confirm the selection"),
        ]
        .into_iter()
        .map(|(a, l)| self.opt(l, current == a, move |this| set(&mut this.draft, a), cx))
        .collect()
    }
    /// Sets row `i`'s shortcut in the draft and in its text input (which `collect` reads).
    fn set_hotkey(&mut self, i: usize, chord: Option<KeyChord>, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(w) = self.draft.workflows.get_mut(i) {
            w.hotkey = chord;
        }
        if let Some(inputs) = self.workflows.get(i) {
            let text = chord.map(|c| c.to_string()).unwrap_or_default();
            inputs.hotkey.update(cx, |input, cx| input.set_value(text, window, cx));
        }
    }

    /// One key press while row `recording` listens.
    fn record_key(&mut self, ev: &gpui::KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        use crate::presenters::hotkeys::{Mods, Recorded, record};
        let Some(i) = self.recording else { return };
        let m = &ev.keystroke.modifiers;
        let mods = Mods { ctrl: m.control, alt: m.alt, shift: m.shift, super_: m.platform };
        match record(&ev.keystroke.key, mods) {
            Recorded::Chord(chord) => {
                self.set_hotkey(i, Some(chord), window, cx);
                self.recording = None;
                self.rec_error = None;
            }
            Recorded::Clear => {
                self.set_hotkey(i, None, window, cx);
                self.recording = None;
                self.rec_error = None;
            }
            Recorded::Cancel => {
                self.recording = None;
                self.rec_error = None;
            }
            Recorded::Waiting => {}
            Recorded::Unusable => self.rec_error = Some("That key can't be a global shortcut. Try a letter, a number, F1–F12 or PrtSc."),
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// The shortcut chip: keycaps, "Set shortcut", or "Press keys…" while recording.
    fn shortcut_chip(
        &self,
        i: usize,
        chord: Option<KeyChord>,
        status: &crate::presenters::hotkeys::RowStatus,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        use crate::presenters::hotkeys::RowStatus;
        let t = cx.theme().clone();
        let x = crate::theme::extra(cx);
        let recording = self.recording == Some(i);
        let bad = matches!(status, RowStatus::Conflict(_) | RowStatus::Failed(_));
        let mut chip = div()
            .id(SharedString::from(format!("wf-key-{i}")))
            .h(px(32.))
            .min_w(px(150.))
            .px(space::S2)
            .flex()
            .items_center()
            .justify_center()
            .gap(px(4.))
            .rounded(crate::theme::radius::MD)
            .border_1()
            .cursor_pointer();
        chip = if recording {
            chip.border_2()
                .border_color(t.primary)
                .bg(t.primary.opacity(0.08))
                .track_focus(&self.rec_focus)
                .on_key_down(cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| this.record_key(ev, window, cx)))
                .child(div().text_size(text::SMALL).text_color(t.primary).child("Press keys…"))
        } else {
            let base = chip.border_color(if bad { t.danger } else { x.ring }).bg(x.card).hover(|s| s.bg(x.wash));
            match chord {
                Some(c) => {
                    let mut caps = base;
                    for cap in crate::presenters::hotkeys::keycaps(c) {
                        caps = caps.child(
                            div()
                                .px(px(6.))
                                .h(px(22.))
                                .flex()
                                .items_center()
                                .rounded(px(5.))
                                .bg(t.secondary)
                                .border_b_2()
                                .border_color(x.ring)
                                .text_size(text::SMALL)
                                .font_weight(gpui::FontWeight::MEDIUM)
                                .child(cap),
                        );
                    }
                    caps
                }
                None => base.border_dashed().child(div().text_size(text::SMALL).text_color(t.muted_foreground).child("Set shortcut")),
            }
        };
        chip.on_click(cx.listener(move |this, _, window, cx| {
            this.recording = Some(i);
            this.rec_error = None;
            window.focus(&this.rec_focus, cx);
            cx.notify();
        }))
        .into_any_element()
    }

    fn workflows_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        use crate::presenters::hotkeys::{RowStatus, row_status};
        let t = cx.theme().clone();
        let x = crate::theme::extra(cx);
        let report = (cx.global::<UiState>().services.hotkey_report)();
        let off = self.draft.disable_hotkeys;

        // Whether shortcuts can work here at all, in one line, with the switch beside it.
        let (tone, headline, detail): (Tone, String, String) = if off {
            (Tone::Neutral, "Shortcuts are off".into(), "Laqta isn't listening for its shortcuts. Turn them back on to use them.".into())
        } else if let Some(why) = &report.unsupported {
            (
                Tone::Danger,
                "Global shortcuts aren't available on this system".into(),
                format!("{why}. You can still bind `laqta --action <name>` in your desktop's keyboard settings."),
            )
        } else if report.pending {
            (Tone::Busy, "Registering shortcuts…".into(), "The desktop may ask you to confirm them the first time.".into())
        } else {
            let n = report.registered.len();
            (
                Tone::Success,
                format!("{n} shortcut{} active", if n == 1 { "" } else { "s" }),
                "They work in every app. Changes apply when you save; KDE may ask you to confirm new ones.".into(),
            )
        };
        let status = entity_card(cx).child(
            div()
                .flex()
                .items_center()
                .gap(space::S3)
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap(px(6.))
                        .child(widgets::badge(headline, tone, cx))
                        .child(widgets::muted(detail, cx)),
                )
                .child(Switch::new("hotkeys-on").checked(!off).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.disable_hotkeys = !*v;
                    cx.notify();
                }))),
        );

        let rows: Vec<(String, Option<KeyChord>, bool)> =
            self.draft.workflows.iter().map(|w| (w.name.clone(), w.hotkey, w.enabled)).collect();
        let statuses = row_status(&rows, &report.registered, &report.failed);
        let mut list = div().flex().flex_col().rounded(crate::theme::radius::CARD).border_1().border_color(x.ring).overflow_hidden();
        if self.draft.workflows.is_empty() {
            list = list.child(widgets::empty_state(
                IconName::Keyboard,
                "No shortcuts yet",
                "Add one below, then click its key box and press the keys.",
                cx,
            ));
        }
        for (i, ((w, inputs), st)) in self.draft.workflows.iter().zip(&self.workflows).zip(statuses).enumerate() {
            let (dot, note): (Option<gpui::Hsla>, Option<String>) = match &st {
                // A working shortcut needs no marker; only problems and pending changes get one.
                RowStatus::Active => (None, None),
                RowStatus::Pending => (Some(t.muted_foreground), Some("Not active yet: save to apply.".into())),
                RowStatus::Conflict(why) => (Some(t.danger), Some(why.clone())),
                RowStatus::Failed(why) => (Some(t.danger), Some(format!("The system refused it: {why}"))),
                RowStatus::Disabled | RowStatus::Unset => (None, None),
            };
            let recording_here = self.recording == Some(i);
            let note = if recording_here {
                Some(self.rec_error.unwrap_or("Press the keys, Esc to keep the old shortcut, Backspace to remove it.").to_owned())
            } else {
                note
            };
            let overriding = w.patch.after_capture.is_some();
            let row = div()
                .flex()
                .flex_col()
                .gap(space::S2)
                .px(space::S4)
                .py(space::S3)
                .when(i > 0, |d| d.border_t_1().border_color(x.ring))
                .when(!w.enabled, |d| d.opacity(0.6))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S3)
                        .child(widgets::icon_tile(crate::actions_toolbar::action_icon(w.action), false, cx))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .flex()
                                .flex_col()
                                .gap(px(2.))
                                .child(Input::new(&inputs.name).appearance(false).cleanable(false))
                                // Only what the name doesn't already say.
                                .when_some(
                                    {
                                        let mut parts = Vec::new();
                                        if w.name != w.action.label() {
                                            parts.push(format!("Runs {}", w.action.label()));
                                        }
                                        if overriding {
                                            parts.push("Own after-capture steps".to_owned());
                                        }
                                        (!parts.is_empty()).then(|| parts.join(" · "))
                                    },
                                    |d, sub| d.child(div().pl(px(8.)).text_size(text::SMALL).text_color(t.muted_foreground).child(sub)),
                                ),
                        )
                        .when_some(dot, |d, c| d.child(div().size(px(8.)).flex_none().rounded(crate::theme::radius::PILL).bg(c)))
                        .child(self.shortcut_chip(i, w.hotkey, &st, cx))
                        .child(Switch::new(SharedString::from(format!("wf-en-{i}"))).checked(w.enabled).on_click(cx.listener(
                            move |this, v: &bool, _, cx| {
                                if let Some(w) = this.draft.workflows.get_mut(i) {
                                    w.enabled = *v;
                                }
                                cx.notify();
                            },
                        )))
                        .child(widgets::menu_button_with(
                            SharedString::from(format!("wf-more-{i}")),
                            "",
                            IconName::Ellipsis,
                            vec![
                                widgets::Choice::new(
                                    if overriding { "Use the default after-capture steps" } else { "Give it its own after-capture steps" },
                                    false,
                                    cx.listener(move |this, _, _, cx| {
                                        let defaults = this.draft.defaults.after_capture;
                                        if let Some(w) = this.draft.workflows.get_mut(i) {
                                            w.patch.after_capture = if w.patch.after_capture.is_some() { None } else { Some(defaults) };
                                        }
                                        cx.notify();
                                    }),
                                ),
                                widgets::Choice::new(
                                    "Remove shortcut",
                                    false,
                                    cx.listener(move |this, _, window, cx| {
                                        if this.collect(cx).is_ok() && i < this.draft.workflows.len() {
                                            this.draft.workflows.remove(i);
                                            this.recording = None;
                                            this.rebuild_workflow_inputs(window, cx);
                                        }
                                        cx.notify();
                                    }),
                                ),
                            ],
                        )),
                )
                .when_some(note, |d, n| {
                    let danger =
                        matches!(st, RowStatus::Conflict(_) | RowStatus::Failed(_)) || (recording_here && self.rec_error.is_some());
                    d.child(div().pl(px(48.)).child(widgets::notice(n, !danger, cx)))
                });
            let row = match w.patch.after_capture {
                Some(flags) => row.child(div().pl(px(48.)).child(self.after_capture_list(
                    &format!("wf-{i}"),
                    flags,
                    move |this, flag, on| {
                        if let Some(Some(f)) = this.draft.workflows.get_mut(i).map(|w| w.patch.after_capture.as_mut()) {
                            f.set(flag, on);
                        }
                    },
                    cx,
                ))),
                None => row,
            };
            list = list.child(row);
        }

        let add_opts = [
            Action::RectangleRegion,
            Action::PrintScreen,
            Action::ActiveWindow,
            Action::ActiveMonitor,
            Action::LastRegion,
            Action::CustomRegion,
            Action::ScrollingCapture,
            Action::ScreenRecorder,
            Action::ScreenRecorderGif,
            Action::Ocr,
            Action::ScreenColorPicker,
            Action::ImageEditor,
            Action::ClipboardUpload,
            Action::FileUpload,
            Action::OpenMainWindow,
            Action::OpenHistory,
        ]
        .into_iter()
        .map(|action| {
            widgets::Choice::new(
                crate::i18n::action_label(action),
                false,
                cx.listener(move |this, _, window, cx| {
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
                        // Go straight to recording its keys.
                        this.recording = Some(this.draft.workflows.len() - 1);
                        this.rec_error = None;
                        window.focus(&this.rec_focus, cx);
                    }
                    cx.notify();
                }),
            )
        })
        .collect();
        page().child(status).child(
            widgets::section("Your shortcuts", cx).child(list).child(div().pt(space::S3).child(widgets::menu_button(
                "wf-add",
                "Add shortcut",
                add_opts,
            ))),
        )
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().w(px(232.)).flex_none().flex().flex_col().gap(px(2.)).p(space::S3).pt(space::S1);
        for (page, label, glyph) in PAGES {
            let listener = cx.listener(move |this, _, _, cx| {
                this.page = page;
                cx.notify();
            });
            nav = nav.child(widgets::nav_row(SharedString::from(format!("nav-{label}")), glyph, label, self.page == page, listener, cx));
        }
        let (title, description) = page_header(self.page);
        let content = match self.page {
            Page::General => self.general(cx),
            Page::AfterCapture => {
                let flags = self.draft.defaults.after_capture;
                self.after_capture_list("ac", flags, |this, f, on| this.draft.defaults.after_capture.set(f, on), cx)
            }
            Page::Image => self.image(cx),
            Page::Naming => self.naming(cx),
            Page::Capture => self.capture(cx),
            Page::Workflows => self.workflows_page(cx),
            Page::Uploads => self.uploads(window, cx),
            Page::Recording => self.recording(cx),
            Page::Automation => self.automation(cx),
            Page::Models => self.models_page(cx),
            Page::Integration => self.integration(cx),
            Page::Data => self.data(cx),
        };
        let mut footer = div()
            .flex()
            .flex_row()
            .items_center()
            .gap(space::S2)
            .px(space::S6)
            .py(space::S3)
            .border_t_1()
            .border_color(cx.theme().border)
            .bg(cx.theme().background)
            .rounded_br(crate::chrome::corners(window).bottom_right);
        footer = match &self.message {
            Some((ok, msg)) => footer.child(widgets::notice(msg.clone(), *ok, cx).flex_1().min_w_0().truncate()),
            None => footer.child(div().flex_1()),
        };
        footer = footer.child(widgets::pill("save", "Save", Pill::Dark, false, cx.listener(|this, _, _, cx| this.save(cx)), cx));
        let settings_label =
            div().font_family(text::HEADING_FONT).font_weight(FontWeight::SEMIBOLD).text_size(text::SECTION).child("Settings");
        let body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                div().id("settings-scroll").flex_1().overflow_y_scrollbar().child(
                    div()
                        .max_w(px(780.))
                        .mx_auto()
                        .px(space::S8)
                        .pt(space::S6)
                        .pb(space::S10)
                        .flex()
                        .flex_col()
                        .child(widgets::settings_header(title, description, cx))
                        .child(content),
                ),
            )
            .child(footer);
        widgets::tab_keys(crate::chrome::rounded(widgets::page(cx), window).bg(cx.theme().sidebar), &self.focus)
            .child(crate::chrome::TitleBar::new().leading(settings_label, px(232.)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(nav)
                    .child(crate::chrome::sheet(window, cx).flex_1().min_w_0().h_full().flex().child(body)),
            )
    }
}

/// A settings page body: sections stacked with room between them.
fn page() -> gpui::Div {
    div().flex().flex_col().gap(px(36.))
}

/// A card for one item in a list the user builds (an account, a shortcut, a watched folder).
fn entity_card(cx: &App) -> gpui::Div {
    let x = crate::theme::extra(cx);
    div().flex().flex_col().gap(px(2.)).px(space::S5).py(space::S4).rounded(crate::theme::radius::CARD).border_1().border_color(x.ring)
}

/// Each page's title and the one line that says what it is for.
fn page_header(page: Page) -> (&'static str, &'static str) {
    match page {
        Page::General => ("General", "Appearance, where captures are saved, notifications and updates."),
        Page::AfterCapture => ("After capture", "What Laqta does with every capture. A shortcut can use its own steps instead."),
        Page::Image => ("Image format", "File format, quality and thumbnails for saved captures."),
        Page::Naming => ("File naming", "How saved files and their folders are named."),
        Page::Capture => ("Capture", "What the selection screen shows and how it responds to clicks."),
        Page::Workflows => ("Shortcuts", "Global shortcuts that work in every app. Click a key box and press the keys."),
        Page::Uploads => ("Uploads", "Where uploads go and what is copied when they finish."),
        Page::Recording => ("Recording", "Screen recording to video and GIF."),
        Page::Automation => ("Automation", "Timed captures, scrolling captures and folders that upload new files."),
        Page::Models => ("AI features", "Text recognition, background removal and image descriptions."),
        Page::Integration => ("Integration", "Start-up, the file manager menu and the browser extension."),
        Page::Data => ("Backup and import", "Back up your settings or bring them over from ShareX."),
    }
}

fn path_text(p: &Option<std::path::PathBuf>) -> String {
    p.as_ref().map(|p| p.display().to_string()).unwrap_or_default()
}

/// "x, y, width, height" → a rect; empty → None.
fn parse_region(text: &str) -> Result<Option<PhysRect>, String> {
    if text.trim().is_empty() {
        return Ok(None);
    }
    let parts: Vec<&str> = text.split(',').map(str::trim).collect();
    let bad = || "Custom region must be four numbers: x, y, width, height".to_string();
    if parts.len() != 4 {
        return Err(bad());
    }
    let x = parts[0].parse::<i32>().map_err(|_| bad())?;
    let y = parts[1].parse::<i32>().map_err(|_| bad())?;
    let width = parts[2].parse::<u32>().map_err(|_| bad())?;
    let height = parts[3].parse::<u32>().map_err(|_| bad())?;
    if width == 0 || height == 0 {
        return Err(bad());
    }
    Ok(Some(PhysRect { x, y, width, height }))
}

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
use gpui_component::{ActiveTheme as _, Disableable as _, Root, Selectable as _, Sizable as _};
use laqta_domain::capture::RegionMouseAction;
use laqta_domain::geometry::PhysRect;
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
    Integration,
    Data,
}

const PAGES: [(Page, &str); 11] = [
    (Page::General, "General"),
    (Page::AfterCapture, "After capture"),
    (Page::Image, "Image"),
    (Page::Naming, "File naming"),
    (Page::Capture, "Capture"),
    (Page::Workflows, "Hotkeys and workflows"),
    (Page::Uploads, "Uploads"),
    (Page::Recording, "Recording"),
    (Page::Automation, "Automation"),
    (Page::Integration, "Integration"),
    (Page::Data, "Data"),
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
    dest_kind: DestKind,
    hosts: Vec<Vec<(&'static str, bool, Entity<InputState>)>>,
    rec: RecordingInputs,
    auto_interval: Entity<InputState>,
    auto_count: Entity<InputState>,
    scroll: [Entity<InputState>; 6],
    /// Per watch folder: folder and filter inputs, parallel to `draft.watch_folders`.
    watch: Vec<(Entity<InputState>, Entity<InputState>)>,
    /// The last update check's message, and whether it can be installed from here.
    update: Option<(String, bool)>,
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
            dest_kind: DestKind::Image,
            hosts: Vec::new(),
            update: None,
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
            },
            auto_interval: text_input((draft.auto_capture.interval_ms / 1000).to_string(), window, cx),
            auto_count: text_input(draft.auto_capture.count.to_string(), window, cx),
            scroll: {
                let o = &d.capture.scrolling;
                [o.start_delay_ms, o.scroll_delay_ms, o.max_frames, o.scroll_amount, o.ignore_top, o.ignore_bottom]
                    .map(|v| text_input(v.to_string(), window, cx))
            },
            watch: Vec::new(),
            draft,
            page: Page::General,
            message: None,
        };
        this.rebuild_workflow_inputs(window, cx);
        this.rebuild_host_inputs(window, cx);
        this.rebuild_watch_inputs(window, cx);
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
            Button::new(id).label(label).selected(this.draft.defaults.general.toast.placement == p).on_click(cx.listener(
                move |this, _, _, cx| {
                    this.draft.defaults.general.toast.placement = p;
                    cx.notify();
                },
            ))
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
            .child(widgets::row("Language", self.language_picker(cx), cx))
            .child(widgets::row("Updates", self.updates_row(cx), cx))
            .child(widgets::muted("Translations come from ShareX's translators; text they don't cover stays in English. The tray menu changes after a restart.", cx))
            .child(
                Checkbox::new("show-toast").label("Show a toast after capture").checked(self.draft.defaults.general.show_toast).on_click(
                    cx.listener(|this, c: &bool, _, cx| {
                        this.draft.defaults.general.show_toast = *c;
                        cx.notify();
                    }),
                ),
            )
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
            .child(
                Checkbox::new("snd-capture")
                    .label("Play a sound after capture")
                    .checked(self.draft.defaults.general.play_sound_after_capture)
                    .on_click(cx.listener(|this, c: &bool, _, cx| {
                        this.draft.defaults.general.play_sound_after_capture = *c;
                        cx.notify();
                    })),
            )
            .child(
                Checkbox::new("snd-task")
                    .label("Play a sound when a task finishes or fails")
                    .checked(self.draft.defaults.general.play_sound_after_task)
                    .on_click(cx.listener(|this, c: &bool, _, cx| {
                        this.draft.defaults.general.play_sound_after_task = *c;
                        cx.notify();
                    })),
            )
            .child(widgets::row("Custom capture sound (file)", Input::new(&self.sound_capture), cx))
            .child(widgets::row("Custom task sound (file)", Input::new(&self.sound_task), cx))
            .child(widgets::row("Custom error sound (file)", Input::new(&self.sound_error), cx))
    }

    fn updates_row(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().flex_wrap().items_center().gap(px(8.)).child(
            Button::new("update-check").label("Check for updates").on_click(cx.listener(|this, _, _, cx| {
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
            })),
        );
        if let Some((msg, installable)) = &self.update {
            if *installable {
                row = row.child(Button::new("update-install").label("Install and restart").primary().on_click(cx.listener(
                    |this, _, _, cx| {
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
                    },
                )));
            }
            row = row.child(widgets::muted(msg.clone(), cx));
        }
        row
    }

    fn language_picker(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_wrap().gap(px(4.));
        let current = self.draft.language.clone();
        list = list.child(Button::new("lang-system").label("System").small().selected(current.is_none()).on_click(cx.listener(
            |this, _, _, cx| {
                this.draft.language = None;
                cx.notify();
            },
        )));
        for (tag, name, _) in crate::i18n::LOCALES {
            let tag: &'static str = tag;
            list = list.child(
                Button::new(SharedString::from(format!("lang-{tag}")))
                    .label(*name)
                    .small()
                    .selected(current.as_deref() == Some(tag))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.language = Some(tag.to_string());
                        cx.notify();
                    })),
            );
        }
        list
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
        let codec_buttons = |gif: bool, this: &Self, cx: &mut Context<Self>| {
            let mut list = div().flex().flex_wrap().gap(px(4.));
            for c in VideoCodec::ALL.into_iter().filter(|c| matches!(c, VideoCodec::Gif | VideoCodec::WebP | VideoCodec::Apng) == gif) {
                let current = if gif { this.draft.defaults.recording.gif_codec } else { this.draft.defaults.recording.codec };
                list = list.child(
                    Button::new(SharedString::from(format!("codec-{gif}-{c:?}"))).label(c.label()).small().selected(current == c).on_click(
                        cx.listener(move |this, _, _, cx| {
                            if gif {
                                this.draft.defaults.recording.gif_codec = c;
                            } else {
                                this.draft.defaults.recording.codec = c;
                            }
                            cx.notify();
                        }),
                    ),
                );
            }
            list
        };
        let check = |id: &'static str,
                     label: &'static str,
                     on: bool,
                     f: fn(&mut laqta_domain::record::RecordingSettings, bool),
                     cx: &mut Context<Self>| {
            Checkbox::new(id).label(label).checked(on).on_click(cx.listener(move |this, c: &bool, _, cx| {
                f(&mut this.draft.defaults.recording, *c);
                cx.notify();
            }))
        };
        widgets::section("Recording", cx)
            .child(widgets::row("Video codec", codec_buttons(false, self, cx), cx))
            .child(widgets::row("Animated image codec", codec_buttons(true, self, cx), cx))
            .child(widgets::row("Frame rate (video)", Input::new(&self.rec.fps), cx))
            .child(widgets::row("Frame rate (GIF)", Input::new(&self.rec.gif_fps), cx))
            .child(widgets::row("Quality (CRF, empty = codec default)", Input::new(&self.rec.crf), cx))
            .child(check("rec-cursor", "Record the mouse cursor", r.show_cursor, |r, c| r.show_cursor = c, cx))
            .child(check("rec-border", "Show a border around the recorded area", r.show_border, |r, c| r.show_border = c, cx))
            .child(check("rec-system-audio", "Record system audio", r.record_system_audio, |r, c| r.record_system_audio = c, cx))
            .child(widgets::row("Microphone (device name, empty = none)", Input::new(&self.rec.microphone), cx))
            .child(widgets::row("Countdown before start (s)", Input::new(&self.rec.countdown), cx))
            .child(widgets::row("Stop after (s, 0 = off)", Input::new(&self.rec.duration), cx))
            .child(widgets::row("Extra FFmpeg arguments", Input::new(&self.rec.args), cx))
            .child(widgets::row("FFmpeg path (empty = find or download)", Input::new(&self.rec.ffmpeg), cx))
    }

    fn automation(&self, cx: &mut Context<Self>) -> gpui::Div {
        use laqta_domain::capture::ScrollMethod;
        let running = cx.global::<UiState>().services.app.auto_capture_running();
        let mut actions = div().flex().flex_wrap().gap(px(4.));
        for a in [Action::PrintScreen, Action::ActiveMonitor, Action::ActiveWindow, Action::CustomRegion, Action::LastRegion] {
            actions = actions.child(
                Button::new(SharedString::from(format!("auto-{a:?}")))
                    .label(crate::i18n::action_label(a))
                    .small()
                    .selected(self.draft.auto_capture.action == a)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.auto_capture.action = a;
                        cx.notify();
                    })),
            );
        }
        let mut methods = div().flex().flex_wrap().gap(px(4.));
        for (m, label) in [
            (ScrollMethod::MouseWheel, "Mouse wheel"),
            (ScrollMethod::DownArrow, "Down arrow"),
            (ScrollMethod::PageDown, "Page Down"),
            (ScrollMethod::ScrollMessage, "Scroll message (Windows)"),
        ] {
            methods = methods.child(
                Button::new(SharedString::from(format!("scroll-{m:?}")))
                    .label(label)
                    .small()
                    .selected(self.draft.defaults.capture.scrolling.method == m)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.draft.defaults.capture.scrolling.method = m;
                        cx.notify();
                    })),
            );
        }
        let mut watch = div().flex().flex_col().gap(px(8.));
        for (i, (folder, (dir, filter))) in self.draft.watch_folders.iter().zip(&self.watch).enumerate() {
            watch = watch.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .p(px(8.))
                    .rounded(px(6.))
                    .border_1()
                    .border_color(cx.theme().border)
                    .child(widgets::row("Folder", Input::new(dir), cx))
                    .child(widgets::row("Files (e.g. *.png;*.jpg)", Input::new(filter), cx))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap(px(12.))
                            .child(Checkbox::new(SharedString::from(format!("wf-on-{i}"))).label("On").checked(folder.enabled).on_click(
                                cx.listener(move |this, c: &bool, _, cx| {
                                    this.draft.watch_folders[i].enabled = *c;
                                    cx.notify();
                                }),
                            ))
                            .child(
                                Checkbox::new(SharedString::from(format!("wf-sub-{i}")))
                                    .label("Include subfolders")
                                    .checked(folder.include_subfolders)
                                    .on_click(cx.listener(move |this, c: &bool, _, cx| {
                                        this.draft.watch_folders[i].include_subfolders = *c;
                                        cx.notify();
                                    })),
                            )
                            .child(
                                Checkbox::new(SharedString::from(format!("wf-move-{i}")))
                                    .label("Move into the screenshots folder")
                                    .checked(folder.move_to_screenshots)
                                    .on_click(cx.listener(move |this, c: &bool, _, cx| {
                                        this.draft.watch_folders[i].move_to_screenshots = *c;
                                        cx.notify();
                                    })),
                            )
                            .child(div().flex_1())
                            .child(Button::new(SharedString::from(format!("wf-rm-{i}"))).label("Remove").small().ghost().on_click(
                                cx.listener(move |this, _, window, cx| {
                                    let _ = this.collect(cx);
                                    this.draft.watch_folders.remove(i);
                                    this.rebuild_watch_inputs(window, cx);
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        widgets::section("Auto capture", cx)
            .child(widgets::muted("Captures on a timer until you stop it (tray, hotkey, or here).", cx))
            .child(widgets::row("Capture", actions, cx))
            .child(widgets::row("Every (seconds)", Input::new(&self.auto_interval), cx))
            .child(widgets::row("Times (0 = until stopped)", Input::new(&self.auto_count), cx))
            .child(Button::new("auto-toggle").label(if running { "Stop auto capture" } else { "Save and start auto capture" }).on_click(
                cx.listener(move |this, _, _, cx| {
                    if !running {
                        this.save(cx);
                    }
                    let services = cx.global::<UiState>().services.clone();
                    (services.dispatch)(if running { Action::StopAutoCapture } else { Action::StartAutoCapture });
                    cx.notify();
                }),
            ))
            .child(div().h(px(12.)))
            .child(widgets::section("Scrolling capture", cx))
            .child(widgets::muted(
                "Select the scrolling area; Laqta scrolls it and stitches the frames. On Wayland you scroll yourself.",
                cx,
            ))
            .child(widgets::row("Scroll with", methods, cx))
            .child(widgets::row("Wait before the first frame (ms)", Input::new(&self.scroll[0]), cx))
            .child(widgets::row("Wait after each scroll (ms)", Input::new(&self.scroll[1]), cx))
            .child(widgets::row("Stop after (frames)", Input::new(&self.scroll[2]), cx))
            .child(widgets::row("Steps per scroll", Input::new(&self.scroll[3]), cx))
            .child(widgets::row("Fixed rows at the top (px)", Input::new(&self.scroll[4]), cx))
            .child(widgets::row("Fixed rows at the bottom (px)", Input::new(&self.scroll[5]), cx))
            .child(div().h(px(12.)))
            .child(widgets::section("Watch folders", cx))
            .child(widgets::muted("New files in these folders are uploaded with your default upload settings.", cx))
            .child(watch)
            .child(Button::new("wf-add").label("Add folder…").on_click(cx.listener(|_this, _, _, cx| {
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
            })))
    }

    fn integration(&self, cx: &mut Context<Self>) -> gpui::Div {
        let services = &cx.global::<UiState>().services;
        let autostart = services.autostart.is_enabled();
        let shell = services.integration.is_installed();
        let mut section = widgets::section("Integration", cx);
        section =
            match autostart {
                Ok(on) => {
                    section.child(Checkbox::new("autostart").label("Run Laqta when I sign in (starts in the tray)").checked(on).on_click(
                        cx.listener(|this, c: &bool, _, cx| {
                            let services = cx.global::<UiState>().services.clone();
                            let r = std::env::current_exe()
                                .map_err(|e| e.to_string())
                                .and_then(|exe| services.autostart.set_enabled(*c, &exe).map_err(|e| e.to_string()));
                            this.report(
                                r.map(|()| {
                                    if *c { "Laqta will start when you sign in.".into() } else { "Laqta will not start at sign-in.".into() }
                                }),
                                cx,
                            );
                        }),
                    ))
                }
                Err(e) => section.child(widgets::muted(format!("Run at sign-in is not available: {e}"), cx)),
            };
        section = section.child(widgets::muted("Adds “Upload with Laqta” to the file manager’s menu and to Send To.", cx));
        section = match shell {
            Ok(true) => {
                section.child(Button::new("shell-off").label("Remove “Upload with Laqta”").on_click(cx.listener(|this, _, _, cx| {
                    let r = cx.global::<UiState>().services.integration.uninstall().map_err(|e| e.to_string());
                    this.report(r.map(|()| "Removed from the file manager.".into()), cx);
                })))
            }
            Ok(false) => section.child(Button::new("shell-on").label("Add “Upload with Laqta”").on_click(cx.listener(|this, _, _, cx| {
                let services = cx.global::<UiState>().services.clone();
                let r = std::env::current_exe()
                    .map_err(|e| e.to_string())
                    .and_then(|exe| services.integration.install(&exe).map_err(|e| e.to_string()));
                this.report(r.map(|()| "Added to the file manager. Some file managers show it after a restart.".into()), cx);
            }))),
            Err(e) => section.child(widgets::muted(format!("File manager integration is not available: {e}"), cx)),
        };
        section
            .child(widgets::muted("Lets the ShareX browser extension (Chrome, Firefox) send pages, images and links to Laqta.", cx))
            .child(Button::new("nmh").label("Register the browser extension host").on_click(cx.listener(|this, _, _, cx| {
                let services = cx.global::<UiState>().services.clone();
                let r = std::env::current_exe().map_err(|e| e.to_string()).and_then(|exe| {
                    let nmh = exe.with_file_name(if cfg!(windows) { "laqta-nmh.exe" } else { "laqta-nmh" });
                    if !nmh.exists() {
                        return Err(format!("{} is missing", nmh.display()));
                    }
                    services.integration.install_native_messaging(&nmh).map_err(|e| e.to_string())
                });
                this.report(r.map(|()| "Browser extension host registered.".into()), cx);
            })))
    }

    fn data(&self, cx: &mut Context<Self>) -> gpui::Div {
        widgets::section("Data", cx)
            .child(widgets::muted("Back up or move your settings, history and custom uploaders as one archive.", cx))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(Button::new("export").label("Export settings…").on_click(cx.listener(|this, _, _, cx| this.export_archive(cx))))
                    .child(Button::new("import").label("Import settings…").on_click(cx.listener(|this, _, _, cx| this.import_archive(cx)))),
            )
            .child(widgets::muted("Bring in the capture history of an existing ShareX install (History.json).", cx))
            .child(Button::new("sharex-history").label("Import ShareX history…").on_click(cx.listener(|this, _, _, cx| this.import_sharex(cx))))
            .child(widgets::muted(
                "Bring over an existing ShareX setup: workflows and hotkeys, custom uploaders, accounts, effect presets and history. Pick the ShareX folder (Documents\\ShareX) or a .sxb backup.",
                cx,
            ))
            .child(
                div()
                    .flex()
                    .gap(px(8.))
                    .child(Button::new("sharex-folder").label("Import ShareX folder…").on_click(cx.listener(|this, _, _, cx| this.import_sharex_config(true, cx))))
                    .child(Button::new("sharex-backup").label("Import ShareX backup (.sxb)…").on_click(cx.listener(|this, _, _, cx| this.import_sharex_config(false, cx)))),
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
        let mut kinds = div().flex().gap(px(6.));
        for (k, label) in [
            (DestKind::Image, "Images"),
            (DestKind::Text, "Text"),
            (DestKind::File, "Files"),
            (DestKind::Shortener, "URL shortener"),
            (DestKind::Sharer, "URL sharing"),
        ] {
            kinds =
                kinds.child(Button::new(SharedString::from(format!("dk-{label}"))).label(label).selected(self.dest_kind == k).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.dest_kind = k;
                        cx.notify();
                    }),
                ));
        }
        let current = match self.dest_kind {
            DestKind::Image => u.image_destination.clone(),
            DestKind::Text => u.text_destination.clone(),
            DestKind::File => u.file_destination.clone(),
            DestKind::Shortener => u.url_shortener.clone(),
            DestKind::Sharer => u.url_sharer.clone(),
        };
        let choices: Vec<(String, String)> = match self.dest_kind {
            DestKind::Shortener => present::shortener_choices(&services.uploaders.destination_list(), &self.draft.hosts),
            DestKind::Sharer => present::SHARERS.iter().map(|(id, n)| (id.to_string(), n.to_string())).collect(),
            _ => services.uploaders.destination_list(),
        };
        let mut list = div().flex().flex_wrap().gap(px(6.)).max_h(px(220.)).overflow_y_scrollbar();
        for (id, name) in choices {
            let selected = id == current;
            let kind = self.dest_kind;
            list = list.child(Button::new(SharedString::from(format!("dest-{id}"))).label(name).small().selected(selected).on_click(
                cx.listener(move |this, _, _, cx| {
                    let u = &mut this.draft.defaults.upload;
                    match kind {
                        DestKind::Image => u.image_destination = id.clone(),
                        DestKind::Text => u.text_destination = id.clone(),
                        DestKind::File => u.file_destination = id.clone(),
                        DestKind::Shortener => u.url_shortener = id.clone(),
                        DestKind::Sharer => u.url_sharer = id.clone(),
                    }
                    cx.notify();
                }),
            ));
        }
        let mut after = div().flex().flex_col().gap(px(6.));
        for (flag, label) in present::after_upload_options() {
            after = after.child(
                Checkbox::new(SharedString::from(format!("au-{}", flag.bits())))
                    .label(label)
                    .checked(self.draft.defaults.after_upload.contains(flag))
                    .on_click(cx.listener(move |this, v: &bool, _, cx| {
                        this.draft.defaults.after_upload.set(flag, *v);
                        cx.notify();
                    })),
            );
        }
        let mut accounts = div().flex().flex_col().gap(px(8.));
        for (i, (account, inputs)) in self.draft.hosts.iter().zip(&self.hosts).enumerate() {
            let mut card = div().flex().flex_col().gap(px(4.)).p(px(8.)).rounded(px(6.)).bg(cx.theme().secondary).child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .child(div().flex_1().font_weight(gpui::FontWeight::SEMIBOLD).child(account.kind.label()))
                    .child(Button::new(SharedString::from(format!("host-rm-{i}"))).label("Remove").ghost().small().on_click(cx.listener(
                        move |this, _, window, cx| {
                            if i < this.draft.hosts.len() {
                                this.draft.hosts.remove(i);
                                this.rebuild_host_inputs(window, cx);
                            }
                            cx.notify();
                        },
                    ))),
            );
            for ((key, _secret, input), field) in inputs.iter().zip(account.kind.fields()) {
                let _ = key;
                card = card.child(widgets::row(field.label, Input::new(input), cx));
            }
            accounts = accounts.child(card);
        }
        let mut add = div().flex().flex_wrap().gap(px(6.)).child(widgets::muted("Add an account:", cx));
        for kind in laqta_domain::hosts::HostKind::ALL {
            add =
                add.child(Button::new(SharedString::from(format!("host-add-{}", kind.id()))).label(kind.label()).small().ghost().on_click(
                    cx.listener(move |this, _, window, cx| {
                        if this.collect(cx).is_ok() {
                            this.draft.hosts.push(laqta_domain::hosts::HostAccount::new(kind));
                            this.rebuild_host_inputs(window, cx);
                        }
                        cx.notify();
                    }),
                ));
        }
        widgets::section("Uploads", cx)
            .child(widgets::row("Choose a destination for", kinds, cx))
            .child(list)
            .child(
                Checkbox::new("upload-after-capture")
                    .label("Upload after capture")
                    .checked(self.draft.defaults.after_capture.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST))
                    .on_click(cx.listener(|this, v: &bool, _, cx| {
                        this.draft.defaults.after_capture.set(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST, *v);
                        cx.notify();
                    })),
            )
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("After upload"))
            .child(after)
            .child(widgets::row("Copied text ($url, $shorturl, $deletionurl, …)", Input::new(&self.clipboard_format), cx))
            .child(self.clipboard_presets(cx))
            .child(widgets::row("Retries on network errors", Input::new(&self.retries), cx))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Accounts"))
            .child(accounts)
            .child(add)
            .child(
                Button::new("import-sxcu")
                    .label("Import custom uploader (.sxcu)…")
                    .on_click(cx.listener(|this, _, _, cx| this.import_sxcu(cx))),
            )
    }

    fn clipboard_presets(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut row = div().flex().flex_wrap().gap(px(6.)).child(widgets::muted("Presets:", cx));
        for (i, (label, template)) in present::CLIPBOARD_FORMATS.into_iter().enumerate() {
            row = row.child(Button::new(SharedString::from(format!("cf-{i}"))).label(label).small().ghost().on_click(cx.listener(
                move |this, _, window, cx| {
                    this.clipboard_format.update(cx, |input, cx| input.set_value(template, window, cx));
                    cx.notify();
                },
            )));
        }
        row
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
        for (a, label) in
            [(FileExistsAction::AppendNumber, "Add a number"), (FileExistsAction::Overwrite, "Overwrite"), (FileExistsAction::Skip, "Skip")]
        {
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
            .child(check(
                "transparent",
                "Capture windows with transparency (rounded corners, shadows)",
                c.transparent_window,
                |s, v| s.defaults.capture.transparent_window = v,
                cx,
            ))
            .child(widgets::row(
                "Right click in the overlay",
                self.mouse_actions("rc", r.right_click, |s, a| s.defaults.capture.region.right_click = a, cx),
                cx,
            ))
            .child(widgets::row(
                "Middle click in the overlay",
                self.mouse_actions("mc", r.middle_click, |s, a| s.defaults.capture.region.middle_click = a, cx),
                cx,
            ))
            .child(widgets::row("Custom region (x, y, width, height)", Input::new(&self.custom_region), cx))
            .child(widgets::row("Window title for \"Capture window by title\"", Input::new(&self.window_title), cx))
    }

    fn mouse_actions(
        &self,
        id: &str,
        current: RegionMouseAction,
        set: fn(&mut Settings, RegionMouseAction),
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let mut row = div().flex().flex_wrap().gap(px(6.));
        for (a, label) in [
            (RegionMouseAction::None, "Nothing"),
            (RegionMouseAction::CancelCapture, "Cancel"),
            (RegionMouseAction::RemoveShapeOrCancel, "Remove region or cancel"),
            (RegionMouseAction::RemoveShape, "Remove region"),
            (RegionMouseAction::CaptureFullscreen, "Capture fullscreen"),
            (RegionMouseAction::CaptureActiveMonitor, "Capture monitor"),
            (RegionMouseAction::ConfirmSelection, "Confirm"),
        ] {
            row = row.child(Button::new(SharedString::from(format!("{id}-{a:?}"))).label(label).small().selected(current == a).on_click(
                cx.listener(move |this, _, _, cx| {
                    set(&mut this.draft, a);
                    cx.notify();
                }),
            ));
        }
        row
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
                        .child(Checkbox::new(SharedString::from(format!("wf-en-{i}"))).label("On").checked(w.enabled).on_click(
                            cx.listener(move |this, v: &bool, _, cx| {
                                if let Some(w) = this.draft.workflows.get_mut(i) {
                                    w.enabled = *v;
                                }
                                cx.notify();
                            }),
                        ))
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
        for action in [
            Action::RectangleRegion,
            Action::PrintScreen,
            Action::ActiveWindow,
            Action::ActiveMonitor,
            Action::LastRegion,
            Action::CustomRegion,
        ] {
            add = add.child(
                Button::new(SharedString::from(format!("add-{action:?}"))).label(crate::i18n::action_label(action)).ghost().on_click(
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
                        }
                        cx.notify();
                    }),
                ),
            );
        }
        page.child(add)
    }
}

impl Render for SettingsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut nav = div().w(px(200.)).flex().flex_col().gap(px(4.)).p(px(12.)).border_r_1().border_color(cx.theme().border);
        for (page, label) in PAGES {
            nav = nav.child(
                Button::new(SharedString::from(format!("nav-{label}"))).label(label).ghost().selected(self.page == page).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.page = page;
                        cx.notify();
                    }),
                ),
            );
        }
        let content = match self.page {
            Page::General => self.general(cx),
            Page::AfterCapture => {
                let flags = self.draft.defaults.after_capture;
                widgets::section("After capture (defaults)", cx).child(self.after_capture_list(
                    "ac",
                    flags,
                    |this, f, on| this.draft.defaults.after_capture.set(f, on),
                    cx,
                ))
            }
            Page::Image => self.image(cx),
            Page::Naming => self.naming(cx),
            Page::Capture => self.capture(cx),
            Page::Workflows => self.workflows_page(cx),
            Page::Uploads => self.uploads(window, cx),
            Page::Recording => self.recording(cx),
            Page::Automation => self.automation(cx),
            Page::Integration => self.integration(cx),
            Page::Data => self.data(cx),
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
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_row()
                    .overflow_hidden()
                    .child(nav)
                    .child(div().flex_1().p(px(12.)).overflow_y_scrollbar().child(content)),
            )
            .child(footer)
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

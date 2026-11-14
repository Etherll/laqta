//! The settings window. It edits a draft copy of `Settings`; Save validates, then hands the value
//! to `main` (store, `App::set_settings`, hotkey re-registration). Workflow overrides are shown as
//! differences from the defaults (ADR 008).

use gpui::{
    AnyElement, App, AppContext as _, Context, Entity, FocusHandle, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _,
    Render, SharedString, StatefulInteractiveElement as _, Styled as _, Window, WindowOptions, div, prelude::FluentBuilder as _, px, size,
};
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::select::{Select, SelectEvent, SelectState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme as _, Root};
use laqta_domain::capture::RegionMouseAction;
use laqta_domain::hotkey::KeyChord;
use laqta_domain::settings::Theme;
use laqta_domain::workflow::{AfterCaptureTasks, FileExistsAction, ToastPlacement, Workflow};
use laqta_domain::{Action, ImageFormat, Settings, WorkflowId};

use crate::assets::IconName;
use crate::presenters::settings::{self as present, TaskOption};
use crate::presenters::settings_form;
use crate::presenters::settings_reset::{self, InputTexts, Scope};
use crate::runtime::{UiState, activate};
use crate::theme::{space, text};
use crate::widgets::{self, Pill, Slot, Tone};

mod ai;

pub(crate) fn open(cx: &mut App) {
    if activate(cx.global::<UiState>().settings, cx) {
        return;
    }
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(1040., 760., cx)),
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

/// What a path setting's Browse button picks.
#[derive(Clone, Copy)]
enum PathPick {
    Folder,
    /// A file, limited to these extensions when the list is not empty.
    File(&'static [&'static str]),
}

/// Sound files the platform's player takes: Windows plays only WAV.
const SOUND_FILES: &[&str] = if cfg!(windows) { &["wav"] } else { &["wav", "ogg", "oga", "flac"] };

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
    /// The saved settings the draft started from, so a save keeps what other windows wrote since.
    opened: Settings,
    /// What a reset made the window's own since the last save.
    claimed: settings_reset::Claimed,
    /// First-run settings for this OS: what "Reset to default" goes back to.
    defaults: Settings,
    /// The text each field shows at its default.
    default_texts: InputTexts,
    page: Page,
    /// The footer line after Save, a reset or a failed action (`crate::flash`).
    flash: crate::flash::Flash,
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
    ai: ai::AiState,
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
    /// Windows: the hook that catches keys the window can't see, while a row records.
    key_hook: Option<laqta_ports::KeyRecording>,
    /// The search field above the page list, and what it holds.
    search: Entity<InputState>,
    query: String,
    /// Every setting the pages show, collected on the first search (`search_index`).
    index: Option<Vec<crate::presenters::settings_search::Entry>>,
    /// The row a search result opened, drawn highlighted until the page changes.
    highlight: Option<SharedString>,
}

struct RecordingInputs {
    fps: Entity<InputState>,
    gif_fps: Entity<InputState>,
    gif_max_width: Entity<InputState>,
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
    let input = cx.new(|cx| InputState::new(window, cx).default_value(value));
    // Redraws on each edit, so a field's "Reset to default" button comes and goes as you type.
    cx.subscribe(&input, |_, _, ev: &InputEvent, cx| {
        if matches!(ev, InputEvent::Change) {
            cx.notify();
        }
    })
    .detach();
    input
}

impl crate::flash::HasFlash for SettingsWindow {
    fn flash(&mut self) -> &mut crate::flash::Flash {
        &mut self.flash
    }
}

impl SettingsWindow {
    /// The footer line: a passing note when `ok`, else a failure that stays until dismissed.
    fn say(&mut self, (ok, msg): (bool, String), cx: &mut Context<Self>) {
        if ok { crate::flash::Flash::show(self, msg, cx) } else { crate::flash::Flash::fail(self, msg, cx) }
    }

    fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let draft = cx.global::<UiState>().services.app.settings();
        let mut this = Self::build(draft, window, cx);
        // A window closed mid-recording must not leave the global shortcuts off.
        cx.on_release(|_, cx| (cx.global::<UiState>().services.hold_hotkeys)(false)).detach();
        this.refresh_models(cx);
        this.refresh_integration(cx);
        this
    }

    /// The window's state for `draft`, with every input showing the draft's values.
    fn build(draft: Settings, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let defaults = (cx.global::<UiState>().services.default_settings)();
        let language = Self::language_select(draft.language.as_deref(), window, cx);
        let x = InputTexts::of(&draft);
        let mut input = |text: &String| text_input(text.clone(), window, cx);
        let (dir, pattern, window_pattern, subfolder) = (input(&x.dir), input(&x.pattern), input(&x.window_pattern), input(&x.subfolder));
        let (jpeg_quality, auto_jpeg, thumb_w, thumb_h) =
            (input(&x.jpeg_quality), input(&x.auto_jpeg), input(&x.thumb_w), input(&x.thumb_h));
        let (delay, toast_ms, custom_region, window_title) =
            (input(&x.delay), input(&x.toast_ms), input(&x.custom_region), input(&x.window_title));
        let (sound_capture, sound_task, sound_error) = (input(&x.sound_capture), input(&x.sound_task), input(&x.sound_error));
        let (clipboard_format, retries) = (input(&x.clipboard_format), input(&x.retries));
        let rec = RecordingInputs {
            fps: input(&x.rec_fps),
            gif_fps: input(&x.rec_gif_fps),
            gif_max_width: input(&x.rec_gif_max_width),
            crf: input(&x.rec_crf),
            countdown: input(&x.rec_countdown),
            duration: input(&x.rec_duration),
            microphone: input(&x.rec_microphone),
            args: input(&x.rec_args),
            ffmpeg: input(&x.rec_ffmpeg),
            camera_device: input(&x.rec_camera_device),
            camera_size: input(&x.rec_camera_size),
        };
        let (auto_interval, auto_count) = (input(&x.auto_interval), input(&x.auto_count));
        let scroll = [0, 1, 2, 3, 4, 5].map(|i| input(&x.scroll[i]));
        let analyzer = [0, 1, 2].map(|i| input(&x.cloud[i]));
        let custom_name = text_input("My model", window, cx);
        let mut this = SettingsWindow {
            dir,
            pattern,
            window_pattern,
            subfolder,
            jpeg_quality,
            auto_jpeg,
            thumb_w,
            thumb_h,
            delay,
            toast_ms,
            custom_region,
            window_title,
            sound_capture,
            sound_task,
            sound_error,
            workflows: Vec::new(),
            clipboard_format,
            retries,
            hosts: Vec::new(),
            update: None,
            focus: cx.focus_handle(),
            language,
            recording: None,
            rec_focus: cx.focus_handle(),
            rec_error: None,
            key_hook: None,
            search: Self::search_field(window, cx),
            query: String::new(),
            index: None,
            highlight: None,
            rec,
            auto_interval,
            auto_count,
            scroll,
            watch: Vec::new(),
            // Filled by `refresh_models`: building the rows parses the catalog and checks files.
            ai: ai::AiState::default(),
            custom_name,
            cloud: {
                let mut key = || cx.new(|cx| InputState::new(window, cx).placeholder("(stored in the system keyring; type to replace)"));
                let (k1, k2) = (key(), key());
                let [url, model, prompt] = analyzer;
                [url, model, prompt, k1, k2]
            },
            default_texts: InputTexts::of(&defaults),
            opened: draft.clone(),
            claimed: settings_reset::Claimed::default(),
            draft,
            defaults,
            page: Page::General,
            flash: Default::default(),
            integration_status: None,
        };
        this.rebuild_workflow_inputs(window, cx);
        this.rebuild_host_inputs(window, cx);
        this.rebuild_watch_inputs(window, cx);
        this
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
        self.collect_with(cx, true)
    }

    /// The window's text fields, trimmed, in the shape `settings_form::apply` reads.
    fn texts(&self, cx: &App) -> InputTexts {
        let v = |input: &Entity<InputState>| Self::value(input, cx);
        InputTexts {
            dir: v(&self.dir),
            pattern: v(&self.pattern),
            window_pattern: v(&self.window_pattern),
            subfolder: v(&self.subfolder),
            jpeg_quality: v(&self.jpeg_quality),
            auto_jpeg: v(&self.auto_jpeg),
            thumb_w: v(&self.thumb_w),
            thumb_h: v(&self.thumb_h),
            delay: v(&self.delay),
            toast_ms: v(&self.toast_ms),
            custom_region: v(&self.custom_region),
            window_title: v(&self.window_title),
            sound_capture: v(&self.sound_capture),
            sound_task: v(&self.sound_task),
            sound_error: v(&self.sound_error),
            clipboard_format: v(&self.clipboard_format),
            retries: v(&self.retries),
            rec_fps: v(&self.rec.fps),
            rec_gif_fps: v(&self.rec.gif_fps),
            rec_gif_max_width: v(&self.rec.gif_max_width),
            rec_crf: v(&self.rec.crf),
            rec_countdown: v(&self.rec.countdown),
            rec_duration: v(&self.rec.duration),
            rec_microphone: v(&self.rec.microphone),
            rec_args: v(&self.rec.args),
            rec_ffmpeg: v(&self.rec.ffmpeg),
            rec_camera_device: v(&self.rec.camera_device),
            rec_camera_size: v(&self.rec.camera_size),
            auto_interval: v(&self.auto_interval),
            auto_count: v(&self.auto_count),
            scroll: self.scroll.each_ref().map(v),
            cloud: [v(&self.cloud[0]), v(&self.cloud[1]), v(&self.cloud[2])],
        }
    }

    /// Reads every input into the draft. With `store_secrets`, typed keys and passwords also go to
    /// the keyring (they are never part of the draft).
    fn collect_with(&mut self, cx: &App, store_secrets: bool) -> Result<(), String> {
        settings_form::apply(&self.texts(cx), &mut self.draft).map_err(|e| e.to_string())?;
        {
            let secrets = cx.global::<UiState>().services.secrets.clone();
            for (input, key) in [(&self.cloud[3], "cloud:analyzer"), (&self.cloud[4], "cloud:remove.bg")] {
                let v = Self::value(input, cx);
                if store_secrets && !v.is_empty() {
                    secrets.set(key, &v).map_err(|e| format!("Couldn't store the key: {e}"))?;
                }
            }
        }
        for (folder, (dir, filter)) in self.draft.watch_folders.iter_mut().zip(&self.watch) {
            folder.folder = Self::value(dir, cx).into();
            let f = Self::value(filter, cx);
            folder.filter = if f.is_empty() { "*".into() } else { f };
        }
        let secrets = cx.global::<UiState>().services.secrets.clone();
        for (account, inputs) in self.draft.hosts.iter_mut().zip(&self.hosts) {
            for (key, secret, input) in inputs {
                let v = Self::value(input, cx);
                if *secret {
                    if store_secrets && !v.is_empty() {
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
                Some(hk.parse::<KeyChord>().map_err(|e| format!("Shortcut “{hk}” for {}: {e}", w.name))?)
            };
        }
        Ok(())
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let said = match self.collect(cx) {
            Err(e) => (false, e),
            Ok(()) => {
                let conflicts = present::hotkey_conflicts(&self.draft);
                let services = cx.global::<UiState>().services.clone();
                settings_reset::keep_outside_edits(&mut self.draft, &self.opened, &services.app.settings(), self.claimed);
                match (services.save_settings)(self.draft.clone()) {
                    Ok(()) => {
                        self.opened = self.draft.clone();
                        self.claimed = settings_reset::Claimed::default();
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
        };
        self.say(said, cx);
        cx.notify();
    }

    /// Applies and saves the theme at once: a choice that waits for Save looks broken.
    fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.draft.theme = theme;
        crate::theme::apply(theme, cx);
        let services = cx.global::<UiState>().services.clone();
        let mut saved = services.app.settings();
        saved.theme = theme;
        if let Err(e) = (services.save_settings)(saved) {
            self.say((false, crate::presenters::notice::failure_line("Couldn't save the theme", &e.to_string())), cx);
        }
    }

    /// Asks, then puts every setting in `scope` back to its default in the draft (Save keeps it).
    fn confirm_reset(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        // Typed values on other pages live in their inputs until Save; keep them through the reset.
        // Typed keys reach the keyring only once the reset is confirmed.
        if let Err(e) = self.collect_with(cx, false) {
            self.say((false, crate::presenters::notice::failure_line("Fix this before resetting", &e.to_string())), cx);
            cx.notify();
            return;
        }
        let n = settings_reset::changed(scope, &self.draft, &self.defaults);
        if n == 0 {
            return;
        }
        let message = if scope == Scope::All { "Reset all settings?" } else { "Reset this page?" };
        let what = if n == 1 { "1 setting goes".to_owned() } else { format!("{n} settings go") };
        // A page reset touches only that page, so the list of what stays is only worth reading for "Reset all".
        // Model choices save as they're picked (AI features), so their reset does too.
        let detail = match scope {
            Scope::All => format!(
                "{what} back to the default. Anything you added stays: accounts, uploaders, effect presets, watch folders, shortcuts and models, plus your history. Model choices change now; the rest is saved when you press Save."
            ),
            Scope::Models => {
                format!("{what} back to the default. Model choices change now; addresses and prompts are saved when you press Save.")
            }
            Scope::General => format!("{what} back to the default. The theme changes now; the rest is saved when you press Save."),
            _ => format!("{what} back to the default. Nothing is saved until you press Save."),
        };
        let answer = window.prompt(gpui::PromptLevel::Warning, message, Some(&detail), &["Reset", "Cancel"], cx);
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |this, window, cx| {
                // The inputs are rebuilt from the draft, so typed keys are stored now or lost.
                if let Err(e) = this.collect(cx) {
                    this.say((false, crate::presenters::notice::failure_line("Fix this before resetting", &e.to_string())), cx);
                    cx.notify();
                    return;
                }
                this.reset(scope, window, cx);
            });
        })
        .detach();
    }

    fn reset(&mut self, scope: Scope, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.draft.theme;
        settings_reset::reset(scope, &mut self.draft, &self.defaults);
        settings_reset::claim_reset(scope, &mut self.claimed);
        if matches!(scope, Scope::Models | Scope::All) {
            let (models, background) = (self.draft.models.clone(), self.draft.cloud.background);
            self.apply_now(
                move |s| {
                    s.models.clone_from(&models);
                    s.cloud.background = background;
                },
                cx,
            );
        }
        if self.draft.theme != theme {
            self.set_theme(self.draft.theme, cx);
        }
        self.reload_inputs(window, cx);
        self.say((true, "Back to the defaults. Press Save to keep them.".into()), cx);
        cx.notify();
    }

    /// The search field: each edit runs the search; Escape empties it.
    fn search_field(window: &mut Window, cx: &mut Context<Self>) -> Entity<InputState> {
        let input = cx.new(|cx| InputState::new(window, cx).placeholder("Search settings"));
        cx.subscribe(&input, |this, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.query = input.read(cx).value().to_string();
                this.highlight = None;
                cx.notify();
            }
        })
        .detach();
        input
    }

    /// Opens a search result: its page, with its row highlighted, and the search emptied.
    fn open_hit(&mut self, page: usize, label: String, window: &mut Window, cx: &mut Context<Self>) {
        self.page = PAGES[page].0;
        self.recording = None;
        self.sync_key_hook(window, cx);
        self.query.clear();
        self.search.update(cx, |s, cx| s.set_value("", window, cx));
        self.highlight = Some(label.into());
        cx.notify();
    }

    /// One page's body.
    fn page_content(&self, page: Page, window: &mut Window, cx: &mut Context<Self>) -> gpui::Div {
        match page {
            Page::General => self.general(cx),
            Page::AfterCapture => {
                let flags = self.draft.defaults.after_capture;
                self.after_capture_list(
                    "ac",
                    flags,
                    Some(self.defaults.defaults.after_capture),
                    |this, f, on| this.draft.defaults.after_capture.set(f, on),
                    cx,
                )
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
        }
    }

    /// Every setting on every page, from the rows the pages build. Built once per window: labels
    /// don't change while it is open.
    fn search_index(&self, window: &mut Window, cx: &mut Context<Self>) -> Vec<crate::presenters::settings_search::Entry> {
        use crate::presenters::settings_search::Entry;
        let mut out = Vec::new();
        for (i, (page, title, _)) in PAGES.iter().enumerate() {
            let (_, description) = page_header(*page);
            out.push(Entry {
                page: i,
                page_title: (*title).into(),
                section: String::new(),
                label: (*title).into(),
                description: description.into(),
            });
            for (section, label, description) in widgets::collect_rows(|| self.page_content(*page, window, cx)) {
                out.push(Entry { page: i, page_title: (*title).into(), section, label, description });
            }
        }
        out
    }

    /// The search results in place of a page.
    fn search_results(&mut self, window: &mut Window, cx: &mut Context<Self>) -> (String, gpui::Div) {
        use crate::presenters::settings_search::{search, summary};
        if self.index.is_none() {
            self.index = Some(self.search_index(window, cx));
        }
        let index = self.index.as_deref().unwrap_or_default();
        let hits = search(index, &self.query);
        let heading = summary(hits.len(), &self.query);
        if hits.is_empty() {
            let body = widgets::empty_state(
                IconName::Search,
                "Nothing here by that name",
                "Try another word, like “sound”, “format” or “upload”.",
                cx,
            );
            return (heading, div().child(body));
        }
        let t = cx.theme();
        let (fg, muted, wash) = (t.foreground, t.muted_foreground, crate::theme::extra(cx).wash);
        // Pulled out by the row padding, so result text lines up with the heading above.
        let mut list = div().flex().flex_col().gap(px(2.)).mx(-space::S3);
        for (i, hit) in hits.into_iter().enumerate().take(60) {
            let (page, label) = (hit.page, hit.label.clone());
            let place = if hit.section.is_empty() || hit.section == hit.page_title {
                hit.page_title.clone()
            } else {
                format!("{} › {}", hit.page_title, hit.section)
            };
            let row = div()
                .id(("search-hit", i))
                .debug_selector(move || format!("search-hit-{i}"))
                .flex()
                .flex_col()
                .gap(px(2.))
                .px(space::S3)
                .py(space::S2)
                .rounded(crate::theme::radius::CARD)
                .border_1()
                .border_color(gpui::transparent_black())
                .hover(move |s| s.bg(wash))
                .child(
                    div()
                        .flex()
                        .items_baseline()
                        .gap(space::S2)
                        .child(div().text_size(text::BODY).font_weight(FontWeight::MEDIUM).text_color(fg).child(hit.label.clone()))
                        .child(div().text_size(text::LABEL).text_color(muted).child(place)),
                )
                .when(!hit.description.is_empty(), |d| {
                    d.child(div().max_w(px(560.)).text_size(text::LABEL).text_color(muted).child(hit.description.clone()))
                });
            // An arrow on the right says the row opens something.
            let row = row.relative().pr(space::S8).child(
                div()
                    .absolute()
                    .right(space::S3)
                    .top_0()
                    .bottom_0()
                    .flex()
                    .items_center()
                    .child(widgets::icon(IconName::ChevronRight, muted)),
            );
            let open = cx.listener(move |this, _, window, cx| this.open_hit(page, label.clone(), window, cx));
            list = list.child(widgets::clickable(row, open, cx));
        }
        (heading, list)
    }

    /// Rebuilds every input from the draft, keeping the page, messages and loaded state.
    fn reload_inputs(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut fresh = Self::build(self.draft.clone(), window, cx);
        fresh.page = self.page;
        fresh.flash = std::mem::take(&mut self.flash);
        fresh.ai = std::mem::take(&mut self.ai);
        fresh.update = self.update.take();
        fresh.integration_status = self.integration_status.take();
        fresh.focus = self.focus.clone();
        fresh.search = self.search.clone();
        fresh.query = std::mem::take(&mut self.query);
        fresh.highlight = self.highlight.take();
        *self = fresh;
        // A reset mid-recording ends the recording: let the global hotkeys run again.
        self.sync_key_hook(window, cx);
    }

    /// The page's "Reset page" button, while something on it differs from the default.
    /// A line under the page header when shortcuts carry their own copy of this page's settings
    /// (ADR 008): changes here don't reach them, and nothing else on the page says so.
    fn override_note(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        use crate::presenters::settings_overrides::{Section, overriding};
        let sections: &[Section] = match self.page {
            Page::General => &[Section::General, Section::ScreenshotsDir],
            Page::AfterCapture => &[Section::AfterCapture, Section::AfterUpload],
            Page::Image => &[Section::Image, Section::Effects, Section::Beautify],
            Page::Naming => &[Section::Naming],
            Page::Capture | Page::Automation => &[Section::Capture],
            Page::Uploads => &[Section::Upload],
            Page::Recording => &[Section::Recording],
            _ => return None,
        };
        let mut ids: Vec<_> = sections.iter().flat_map(|s| overriding(&self.draft, *s)).collect();
        ids.sort_by_key(|id| id.0);
        ids.dedup();
        let names: Vec<String> = ids.iter().filter_map(|id| self.draft.workflow(*id)).map(|w| format!("“{}”", w.name)).collect();
        let text = match names.as_slice() {
            [] => return None,
            [one] => format!(
                "The shortcut {one} uses its own settings for this page, so changes here don't reach it. Change it under Shortcuts."
            ),
            [first, rest @ ..] => format!(
                "{first} and {} other shortcut{} use their own settings for this page, so changes here don't reach them. Change them under Shortcuts.",
                rest.len(),
                if rest.len() == 1 { "" } else { "s" }
            ),
        };
        Some(
            div()
                .id("override-note")
                .debug_selector(|| "override-note".into())
                .mt(space::S2)
                .text_size(text::SMALL)
                .text_color(cx.theme().muted_foreground)
                .child(text)
                .into_any_element(),
        )
    }

    fn page_reset(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let scope = page_scope(self.page)?;
        let n = settings_reset::changed(scope, &self.draft, &self.defaults);
        if n == 0 {
            return None;
        }
        let label =
            if self.page == Page::Workflows { format!("Restore default shortcuts ({n})") } else { format!("Reset page ({n} changed)") };
        Some(
            widgets::pill(
                "page-reset",
                label,
                Pill::Default,
                true,
                cx.listener(move |this, _, window, cx| this.confirm_reset(scope, window, cx)),
                cx,
            )
            .debug_selector(|| "page-reset".into())
            .into_any_element(),
        )
    }

    /// A path setting: the text stays editable, so a pasted path works, and Browse opens the system
    /// picker through the `Prompt` port.
    #[allow(clippy::too_many_arguments)]
    fn path_row(
        &self,
        id: impl Into<SharedString>,
        label: &'static str,
        description: Option<&'static str>,
        input: &Entity<InputState>,
        pick: PathPick,
        default: Option<fn(&InputTexts) -> &String>,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let id: SharedString = id.into();
        let reset = default.and_then(|d| self.input_reset(&format!("{id}-reset"), input, d, cx));
        let target = input.clone();
        let selector = id.to_string();
        let browse = widgets::pill(
            id,
            "Browse…",
            Pill::Default,
            true,
            cx.listener(move |_this, _, _, cx| {
                let prompt = cx.global::<UiState>().services.prompt.clone();
                let target = target.clone();
                let task = cx.background_spawn(async move {
                    match pick {
                        PathPick::Folder => prompt.pick_folder(label).await,
                        PathPick::File(extensions) => prompt.pick_file(label, extensions).await,
                    }
                });
                cx.spawn(async move |this, cx| {
                    if let Ok(path) = task.await {
                        let _ = this.update_in(cx, |_this, window, cx| {
                            target.update(cx, |input, cx| input.set_value(path.display().to_string(), window, cx));
                            cx.notify();
                        });
                    }
                })
                .detach();
            }),
            cx,
        )
        .debug_selector(move || selector);
        let control =
            div().w_full().flex().items_center().gap(space::S2).child(div().flex_1().min_w_0().child(Input::new(input))).child(browse);
        widgets::row_full(label, description, Slot::Field, reset, control, cx)
    }

    /// A text or number setting with its "Reset to default" button, shown while the text differs
    /// from what the field shows at its default.
    #[allow(clippy::too_many_arguments)]
    fn input_row(
        &self,
        id: &'static str,
        label: impl Into<SharedString>,
        description: Option<impl Into<SharedString>>,
        slot: Slot,
        input: &Entity<InputState>,
        default: fn(&InputTexts) -> &String,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let reset = self.input_reset(id, input, default, cx);
        widgets::row_full(label, description, slot, reset, Input::new(input), cx)
    }

    /// The "Reset to default" button of a choice, shown while the draft's value differs from the
    /// default. `options` names each value for the tooltip.
    fn value_reset<T: PartialEq + Copy + 'static>(
        &self,
        id: &'static str,
        options: &[(T, &'static str)],
        get: fn(&Settings) -> T,
        set: fn(&mut Self, T, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) -> Option<widgets::Reset> {
        let default = get(&self.defaults);
        if get(&self.draft) == default {
            return None;
        }
        let name = options.iter().find(|(v, _)| *v == default).map_or("", |(_, l)| *l);
        Some(widgets::Reset::new(
            id,
            if name.is_empty() { "Reset to default".to_string() } else { format!("Reset to default ({name})") },
            cx.listener(move |this, _, _, cx| {
                set(this, default, cx);
                cx.notify();
            }),
        ))
    }

    fn input_reset(
        &self,
        id: &str,
        input: &Entity<InputState>,
        default: fn(&InputTexts) -> &String,
        cx: &mut Context<Self>,
    ) -> Option<widgets::Reset> {
        let text = default(&self.default_texts).clone();
        if !settings_reset::text_changed(&input.read(cx).value(), &text) {
            return None;
        }
        let tooltip = if text.is_empty() { "Reset to default (empty)".to_string() } else { format!("Reset to default ({text})") };
        let input = input.clone();
        Some(widgets::Reset::new(
            SharedString::from(id.to_string()),
            tooltip,
            cx.listener(move |_, _, window, cx| {
                input.update(cx, |i, cx| i.set_value(text.clone(), window, cx));
                cx.notify();
            }),
        ))
    }

    fn sw(
        &self,
        id: &'static str,
        label: &'static str,
        description: Option<&str>,
        get: fn(&Settings) -> bool,
        set: impl Fn(&mut Self, bool) + Clone + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        let description = description.map(|d| SharedString::from(d.to_string()));
        let (on, default) = (get(&self.draft), get(&self.defaults));
        let reset = (on != default).then(|| {
            let set = set.clone();
            widgets::Reset::new(
                SharedString::from(format!("{id}-reset")),
                if default { "Reset to default (on)" } else { "Reset to default (off)" },
                cx.listener(move |this, _, _, cx| {
                    set(this, default);
                    cx.notify();
                }),
            )
        });
        widgets::switch_row(
            id,
            label,
            description,
            on,
            reset,
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
        // The flags a per-row reset goes back to; `None`: no reset buttons.
        defaults: Option<AfterCaptureTasks>,
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
            let label = if available { label.to_string() } else { format!("{label} (coming soon)") };
            let on = flags.contains(flag);
            let reset = defaults.map(|d| d.contains(flag)).filter(|d| *d != on).map(|d| {
                let f = on_toggle.clone();
                widgets::Reset::new(
                    SharedString::from(format!("{id_prefix}-{}-reset", flag.bits())),
                    if d { "Reset to default (on)" } else { "Reset to default (off)" },
                    cx.listener(move |this, _, _, cx| {
                        f(this, flag, d);
                        cx.notify();
                    }),
                )
            });
            let row = widgets::switch_row(
                SharedString::from(format!("{id_prefix}-{}", flag.bits())),
                label,
                None::<&str>,
                on,
                reset,
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
        let themes = [(Theme::System, "System"), (Theme::Light, "Light"), (Theme::Dark, "Dark")];
        let theme_reset = self.value_reset("theme-reset", &themes, |s| s.theme, |this, t, cx| this.set_theme(t, cx), cx);
        let theme = themes
            .into_iter()
            .map(|(t, label)| {
                // The theme applies and is saved on the click: a choice that waits for Save looks broken.
                widgets::Choice::new(
                    label,
                    self.draft.theme == t,
                    cx.listener(move |this, _, _, cx| {
                        this.set_theme(t, cx);
                        cx.notify();
                    }),
                )
            })
            .collect();
        let placements = [
            (ToastPlacement::TopLeft, "Top left"),
            (ToastPlacement::TopRight, "Top right"),
            (ToastPlacement::BottomLeft, "Bottom left"),
            (ToastPlacement::BottomRight, "Bottom right"),
        ];
        let placement_reset = self.value_reset(
            "toast-pos-reset",
            &placements,
            |s| s.defaults.general.toast.placement,
            |this, p, _| this.draft.defaults.general.toast.placement = p,
            cx,
        );
        let placement = placements
            .into_iter()
            .map(|(p, label)| {
                let on = self.draft.defaults.general.toast.placement == p;
                self.opt(label, on, move |this| this.draft.defaults.general.toast.placement = p, cx)
            })
            .collect();
        let update_note = self.update.as_ref().map(|(m, _)| m.clone()).unwrap_or_else(|| "Laqta checks GitHub for a newer version.".into());
        page()
            .child(
                widgets::section("Appearance", cx)
                    .child(widgets::row_full("Theme", None::<&str>, Slot::Hug, theme_reset, widgets::choice("theme", theme, cx), cx))
                    .child(widgets::row_with(
                        "Language",
                        Some(if cfg!(target_os = "macos") {
                            "Some text may still be in English. The menu bar menu changes after a restart."
                        } else {
                            "Some text may still be in English. The tray menu changes after a restart."
                        }),
                        Slot::Field,
                        self.language_picker(cx),
                        cx,
                    )),
            )
            .child(
                widgets::section("Storage", cx)
                    .child(self.path_row(
                        "dir-browse",
                        "Screenshots folder",
                        Some("Where captures are saved."),
                        &self.dir,
                        PathPick::Folder,
                        Some(|d| &d.dir),
                        cx,
                    ))
                    .child(self.sw(
                        "history",
                        "Keep history",
                        Some(&format!("A list of your captures and uploads, stored in {data_dir}. Takes effect after Laqta restarts.")),
                        |s| s.history.enabled,
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
                        |s| s.defaults.general.show_toast,
                        |this, v| this.draft.defaults.general.show_toast = v,
                        cx,
                    ))
                    .child(self.input_row(
                        "toast-ms-reset",
                        "Show it for",
                        Some("Milliseconds."),
                        Slot::Number,
                        &self.toast_ms,
                        |d| &d.toast_ms,
                        cx,
                    ))
                    .child(widgets::row_full(
                        "Position",
                        None::<&str>,
                        Slot::Hug,
                        placement_reset,
                        widgets::choice("toast-pos", placement, cx),
                        cx,
                    )),
            )
            .child(
                widgets::section("Sounds", cx)
                    .child(self.sw(
                        "snd-capture",
                        "Play a sound after capture",
                        None,
                        |s| s.defaults.general.play_sound_after_capture,
                        |this, v| this.draft.defaults.general.play_sound_after_capture = v,
                        cx,
                    ))
                    .child(self.sw(
                        "snd-task",
                        "Play a sound when a task finishes or fails",
                        None,
                        |s| s.defaults.general.play_sound_after_task,
                        |this, v| this.draft.defaults.general.play_sound_after_task = v,
                        cx,
                    ))
                    .child(self.path_row(
                        "snd-capture-browse",
                        "Capture sound",
                        Some("A sound file. Leave empty for the built-in one."),
                        &self.sound_capture,
                        PathPick::File(SOUND_FILES),
                        Some(|d| &d.sound_capture),
                        cx,
                    ))
                    .child(self.path_row(
                        "snd-task-browse",
                        "Task finished sound",
                        Some("Leave empty for the built-in one."),
                        &self.sound_task,
                        PathPick::File(SOUND_FILES),
                        Some(|d| &d.sound_task),
                        cx,
                    ))
                    .child(self.path_row(
                        "snd-error-browse",
                        "Error sound",
                        Some("Leave empty for the built-in one."),
                        &self.sound_error,
                        PathPick::File(SOUND_FILES),
                        Some(|d| &d.sound_error),
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
                        this.update = Some(r.unwrap_or_else(|e| {
                            (
                                crate::presenters::notice::failure_line(
                                    "Couldn't check for updates. Check your connection and try again",
                                    &e.to_string(),
                                ),
                                false,
                            )
                        }));
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
        let codec_opts: Vec<(VideoCodec, &'static str)> = VideoCodec::available(crate::host_os()).map(|c| (c, c.label())).collect();
        let mut codec_reset = [
            self.value_reset(
                "video-codec-reset",
                &codec_opts,
                |s| s.defaults.recording.codec,
                |this, c, _| this.draft.defaults.recording.codec = c,
                cx,
            ),
            self.value_reset(
                "gif-codec-reset",
                &codec_opts,
                |s| s.defaults.recording.gif_codec,
                |this, c, _| this.draft.defaults.recording.gif_codec = c,
                cx,
            ),
        ];
        let r = &self.draft.defaults.recording;
        let codecs = |gif: bool, this: &Self, cx: &mut Context<Self>| {
            let current = if gif { this.draft.defaults.recording.gif_codec } else { this.draft.defaults.recording.codec };
            let opts = VideoCodec::available(crate::host_os())
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
        let shape_opts = [(S::Circle, "Circle"), (S::RoundedRectangle, "Rounded"), (S::Rectangle, "Rectangle")];
        let corner_opts =
            [(C::TopLeft, "Top left"), (C::TopRight, "Top right"), (C::BottomLeft, "Bottom left"), (C::BottomRight, "Bottom right")];
        let backend_opts = [(B::Ffmpeg, "FFmpeg"), (B::Native, "Native")];
        let shape_reset = self.value_reset(
            "cam-shape-reset",
            &shape_opts,
            |s| s.defaults.recording.camera.shape,
            |this, v, _| this.draft.defaults.recording.camera.shape = v,
            cx,
        );
        let corner_reset = self.value_reset(
            "cam-corner-reset",
            &corner_opts,
            |s| s.defaults.recording.camera.corner,
            |this, v, _| this.draft.defaults.recording.camera.corner = v,
            cx,
        );
        let backend_reset = self.value_reset(
            "backend-reset",
            &backend_opts,
            |s| s.defaults.recording.backend,
            |this, v, _| this.draft.defaults.recording.backend = v,
            cx,
        );
        let shapes = shape_opts
            .into_iter()
            .map(|(v, l)| self.opt(l, cam.shape == v, move |this| this.draft.defaults.recording.camera.shape = v, cx))
            .collect();
        let corners = corner_opts
            .into_iter()
            .map(|(v, l)| self.opt(l, cam.corner == v, move |this| this.draft.defaults.recording.camera.corner = v, cx))
            .collect();
        let backends = backend_opts
            .into_iter()
            .map(|(v, l)| self.opt(l, r.backend == v, move |this| this.draft.defaults.recording.backend = v, cx))
            .collect();
        let rec = |f: fn(&mut laqta_domain::record::RecordingSettings, bool)| {
            move |this: &mut Self, v: bool| f(&mut this.draft.defaults.recording, v)
        };
        page()
            .child(
                widgets::section("Video", cx)
                    .child(widgets::row_full("Format", None::<&str>, Slot::Hug, codec_reset[0].take(), codecs(false, self, cx), cx))
                    .child(self.input_row(
                        "rec-fps-reset",
                        "Frame rate",
                        Some("Frames per second."),
                        Slot::Number,
                        &self.rec.fps,
                        |d| &d.rec_fps,
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-crf-reset",
                        "Quality",
                        Some("CRF: lower means better quality and bigger files. Leave empty for the format's default."),
                        Slot::Number,
                        &self.rec.crf,
                        |d| &d.rec_crf,
                        cx,
                    )),
            )
            .child(
                widgets::section("GIF and animated images", cx)
                    .child(widgets::row_full("Format", None::<&str>, Slot::Hug, codec_reset[1].take(), codecs(true, self, cx), cx))
                    .child(self.input_row(
                        "rec-gif-fps-reset",
                        "Frame rate",
                        Some("Frames per second."),
                        Slot::Number,
                        &self.rec.gif_fps,
                        |d| &d.rec_gif_fps,
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-gif-max-width-reset",
                        "Maximum width",
                        Some("Wider recordings are scaled down to this many pixels, so they save faster and stay small. 0 keeps the full size."),
                        Slot::Number,
                        &self.rec.gif_max_width,
                        |d| &d.rec_gif_max_width,
                        cx,
                    )),
            )
            .child(
                widgets::section("While recording", cx)
                    .child(self.sw(
                        "rec-cursor",
                        "Show the cursor in videos",
                        Some("Screenshots have their own setting under Capture."),
                        |s| s.defaults.recording.show_cursor,
                        rec(|r, c| r.show_cursor = c),
                        cx,
                    ))
                    .child(self.sw(
                        "rec-border",
                        "Show a border around the recorded area",
                        None,
                        |s| s.defaults.recording.show_border,
                        rec(|r, c| r.show_border = c),
                        cx,
                    ))
                    .child(self.sw(
                        "rec-highlight",
                        "Highlight the mouse",
                        Some("Not available on Wayland."),
                        |s| s.defaults.recording.highlight_mouse,
                        rec(|r, c| r.highlight_mouse = c),
                        cx,
                    ))
                    .child(self.sw(
                        "rec-system-audio",
                        "Record system audio",
                        None,
                        |s| s.defaults.recording.record_system_audio,
                        rec(|r, c| r.record_system_audio = c),
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-microphone-reset",
                        "Microphone",
                        Some("Device name. Leave empty to record no microphone."),
                        Slot::Field,
                        &self.rec.microphone,
                        |d| &d.rec_microphone,
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-countdown-reset",
                        "Countdown",
                        Some("Seconds before recording starts."),
                        Slot::Number,
                        &self.rec.countdown,
                        |d| &d.rec_countdown,
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-duration-reset",
                        "Stop after",
                        Some("Seconds. 0 records until you stop it."),
                        Slot::Number,
                        &self.rec.duration,
                        |d| &d.rec_duration,
                        cx,
                    )),
            )
            .child(
                widgets::section("Camera", cx)
                    .child(self.sw(
                        "rec-camera",
                        "Show the camera over the recording",
                        None,
                        |s| s.defaults.recording.camera.enabled,
                        rec(|r, c| r.camera.enabled = c),
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-camera-device-reset",
                        "Camera",
                        Some(if cfg!(windows) {
                            "The camera's name, as Windows lists it. Leave empty for the first camera."
                        } else {
                            "Device name. Leave empty for the first camera."
                        }),
                        Slot::Field,
                        &self.rec.camera_device,
                        |d| &d.rec_camera_device,
                        cx,
                    ))
                    .child(widgets::row_full("Shape", None::<&str>, Slot::Hug, shape_reset, widgets::choice("cam-shape", shapes, cx), cx))
                    .child(widgets::row_full(
                        "Corner",
                        None::<&str>,
                        Slot::Hug,
                        corner_reset,
                        widgets::choice("cam-corner", corners, cx),
                        cx,
                    ))
                    .child(self.input_row(
                        "rec-camera-size-reset",
                        "Size",
                        Some("Percent of the recording's width."),
                        Slot::Number,
                        &self.rec.camera_size,
                        |d| &d.rec_camera_size,
                        cx,
                    )),
            )
            .child(
                widgets::section("Advanced", cx)
                    // Only Windows has a native recorder; elsewhere the choice would do nothing.
                    .when(cfg!(windows), |section| {
                        section.child(widgets::row_full(
                            "Recorder",
                            Some("Native uses Windows' own recorder for H.264 without a microphone, camera or extra arguments. Anything else records with FFmpeg."),
                            Slot::Hug,
                            backend_reset,
                            widgets::choice("backend", backends, cx),
                            cx,
                        ))
                    })
                    .child(self.input_row(
                        "rec-args-reset",
                        "Extra FFmpeg arguments",
                        None::<&str>,
                        Slot::Field,
                        &self.rec.args,
                        |d| &d.rec_args,
                        cx,
                    ))
                    .child(self.path_row(
                        "ffmpeg-browse",
                        "FFmpeg location",
                        Some("Leave empty to find it, or download it on first use."),
                        &self.rec.ffmpeg,
                        PathPick::File(if cfg!(windows) { &["exe"] } else { &[] }),
                        Some(|d| &d.rec_ffmpeg),
                        cx,
                    )),
            )
    }

    fn automation(&self, cx: &mut Context<Self>) -> gpui::Div {
        use laqta_domain::capture::ScrollMethod;
        let running = cx.global::<UiState>().services.app.auto_capture_running();
        let action_opts = [
            (Action::PrintScreen, "Entire screen"),
            (Action::ActiveMonitor, "Active monitor"),
            (Action::ActiveWindow, "Active window"),
            (Action::CustomRegion, "Custom area"),
            (Action::LastRegion, "Last area"),
        ];
        let action_reset = self.value_reset(
            "auto-action-reset",
            &action_opts,
            |s| s.auto_capture.action,
            |this, a, _| this.draft.auto_capture.action = a,
            cx,
        );
        let actions = action_opts
            .into_iter()
            .map(|(a, l)| self.opt(l, self.draft.auto_capture.action == a, move |this| this.draft.auto_capture.action = a, cx))
            .collect();
        let method_opts = [
            (ScrollMethod::MouseWheel, "Mouse wheel"),
            (ScrollMethod::DownArrow, "Down arrow"),
            (ScrollMethod::PageDown, "Page Down"),
            (ScrollMethod::ScrollMessage, "Scroll command"),
        ];
        // The scroll command is a Windows message; elsewhere it only fails at capture time.
        let method_opts: Vec<_> = method_opts.into_iter().filter(|(m, _)| cfg!(windows) || *m != ScrollMethod::ScrollMessage).collect();
        let method_reset = self.value_reset(
            "scroll-method-reset",
            &method_opts,
            |s| s.defaults.capture.scrolling.method,
            |this, m, _| this.draft.defaults.capture.scrolling.method = m,
            cx,
        );
        let methods = method_opts
            .into_iter()
            .map(|(m, l)| {
                let on = self.draft.defaults.capture.scrolling.method == m;
                self.opt(l, on, move |this| this.draft.defaults.capture.scrolling.method = m, cx)
            })
            .collect();
        let toggle = widgets::pill(
            "auto-toggle",
            if running { "Stop" } else { "Start" },
            if running { Pill::Default } else { Pill::Primary },
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
                    .child(self.path_row(format!("wf-browse-{i}"), "Folder", None, dir, PathPick::Folder, None, cx))
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
                let prompt = cx.global::<UiState>().services.prompt.clone();
                let task = cx.background_spawn(async move { prompt.pick_folder("Watch a folder").await });
                cx.spawn(async move |this, cx| {
                    if let Ok(folder) = task.await {
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
                    .child(widgets::row_full(
                        "Capture",
                        None::<&str>,
                        Slot::Hug,
                        action_reset,
                        widgets::choice("auto-action", actions, cx),
                        cx,
                    ))
                    .child(self.input_row(
                        "auto-interval-reset",
                        "Every",
                        Some("Seconds between captures."),
                        Slot::Number,
                        &self.auto_interval,
                        |d| &d.auto_interval,
                        cx,
                    ))
                    .child(self.input_row(
                        "auto-count-reset",
                        "Stop after",
                        Some("Number of captures. 0 keeps going until you stop it."),
                        Slot::Number,
                        &self.auto_count,
                        |d| &d.auto_count,
                        cx,
                    ))
                    .child(widgets::action_row(
                        if running { "Timed capture is running" } else { "Timed capture is off" },
                        Some(if running && cfg!(target_os = "macos") {
                            "Stop it here, from the menu bar or with its shortcut."
                        } else if running {
                            "Stop it here, from the tray or with its shortcut."
                        } else {
                            "Start saves these settings first."
                        }),
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
                    .child(widgets::row_full(
                        "Scroll with",
                        None::<&str>,
                        Slot::Hug,
                        method_reset,
                        widgets::choice("scroll-method", methods, cx),
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-0-reset",
                        "Wait before the first frame",
                        Some("Milliseconds."),
                        Slot::Number,
                        &self.scroll[0],
                        |d| &d.scroll[0],
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-1-reset",
                        "Wait after each scroll",
                        Some("Milliseconds."),
                        Slot::Number,
                        &self.scroll[1],
                        |d| &d.scroll[1],
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-2-reset",
                        "Stop after",
                        Some("Number of scrolls."),
                        Slot::Number,
                        &self.scroll[2],
                        |d| &d.scroll[2],
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-3-reset",
                        "Steps per scroll",
                        None::<&str>,
                        Slot::Number,
                        &self.scroll[3],
                        |d| &d.scroll[3],
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-4-reset",
                        "Ignore a fixed header",
                        Some("Pixels at the top that don't scroll, like a sticky menu bar."),
                        Slot::Number,
                        &self.scroll[4],
                        |d| &d.scroll[4],
                        cx,
                    ))
                    .child(self.input_row(
                        "scroll-5-reset",
                        "Ignore a fixed footer",
                        Some("Pixels at the bottom that don't scroll."),
                        Slot::Number,
                        &self.scroll[5],
                        |d| &d.scroll[5],
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
            None,
            cx.listener(move |this, v: &bool, _, cx| {
                set(this, *v);
                cx.notify();
            }),
            cx,
        )
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
                Some(if cfg!(target_os = "macos") {
                    "Laqta starts in the menu bar. Applies now."
                } else {
                    "Laqta starts in the tray. Applies now."
                }),
                on,
                None,
                cx.listener(|this, c: &bool, _, cx| {
                    let services = cx.global::<UiState>().services.clone();
                    let r = std::env::current_exe()
                        .map_err(|e| e.to_string())
                        // An administrator Laqta starts from a scheduled task, so sign-in doesn't ask UAC each time.
                        .and_then(|exe| {
                            let elevated = services.app.settings().run_as_admin;
                            services.autostart.set_enabled_elevated(*c, elevated, &exe).map_err(|e| e.to_string())
                        });
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
        // Finder takes no menu entries from a plain app, so macOS says how to send a file instead.
        let (files_title, file_manager) = if cfg!(target_os = "macos") {
            let how = "Drag a file onto Laqta in the Dock, or right-click it and choose Open With > Laqta.";
            ("Finder", widgets::row_with("Upload from Finder", Some(how.to_string()), Slot::Hug, div(), cx))
        } else {
            ("File manager", file_manager)
        };
        let startup = widgets::section("Startup", cx).child(startup).when(cfg!(windows), |s| {
            s.child(self.sw(
                "run-as-admin",
                "Run as administrator",
                Some("Windows asks for administrator rights (UAC) when Laqta starts, so its shortcuts and captures also work in apps and games that run as administrator. Laqta then takes no files or links from other programs. Only turn it on if you need it: Laqta offers a one-time restart as administrator when such an app is in front. Applies after Laqta restarts."),
                |s| s.run_as_admin,
                |this, v| this.draft.run_as_admin = v,
                cx,
            ))
            .child(self.sw(
                "hotkeys-via-hook",
                "Shortcuts work in games",
                Some("Catches Laqta's shortcuts with a keyboard hook before other apps see them, for games that take the keyboard for themselves. Those keys then no longer reach the app in front. Games that run as administrator stay out of reach unless Laqta does too."),
                |s| s.hotkeys_via_hook,
                |this, v| this.draft.hotkeys_via_hook = v,
                cx,
            ))
        });
        page().child(startup).child(widgets::section(files_title, cx).child(file_manager)).child(widgets::section("Browser", cx).child(
            widgets::action_row(
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
            ),
        ))
    }
    fn data(&self, cx: &mut Context<Self>) -> gpui::Div {
        let changed = settings_reset::changed(Scope::All, &self.draft, &self.defaults);
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
                            widgets::pill("sharex-folder", "Folder…", Pill::Default, false, cx.listener(|this, _, window, cx| this.import_sharex_config(true, window, cx)), cx)
                                .into_any_element(),
                            widgets::pill("sharex-backup", "Backup…", Pill::Default, false, cx.listener(|this, _, window, cx| this.import_sharex_config(false, window, cx)), cx)
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
            .child(widgets::section("Reset", cx).child(widgets::action_row(
                "Reset all settings",
                Some(if changed == 0 {
                    "Every setting is at its default.".to_owned()
                } else {
                    format!("Puts {changed} changed settings back to their defaults. Accounts, custom uploaders, effect presets, watch folders, history and the shortcuts and models you added stay.")
                }),
                widgets::pill("reset-all", "Reset all…", Pill::Default, false, cx.listener(|this, _, window, cx| this.confirm_reset(Scope::All, window, cx)), cx)
                    .when(changed == 0, |p| p.opacity(0.5)),
                cx,
            )))
    }
    fn import_sharex_config(&mut self, folder: bool, window: &mut Window, cx: &mut Context<Self>) {
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
        cx.spawn_in(window, async move |this, cx| {
            let r = task.await;
            let _ = this.update_in(cx, |this, window, cx| match r {
                Ok(Some(summary)) => {
                    this.draft = cx.global::<UiState>().services.app.settings();
                    this.opened = this.draft.clone();
                    this.claimed = settings_reset::Claimed::default();
                    // The inputs hold the old text until rebuilt, and the next Save would write it back.
                    this.reload_inputs(window, cx);
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
                DestKind::Shortener => present::shortener_choices(&services.uploaders.custom_shortener_list(), &self.draft.hosts),
                DestKind::Sharer => present::sharer_choices(&services.uploaders.custom_sharer_list()),
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
            .child(self.input_row(
                "clipboard-format-reset",
                "Copied text",
                Some("What “Copy the link” puts on the clipboard. $url is the link; $shorturl and $deletionurl also work."),
                Slot::Field,
                &self.clipboard_format,
                |d| &d.clipboard_format,
                cx,
            ))
            .child(widgets::row_with("Format", None::<&str>, Slot::Hug, self.clipboard_presets(cx), cx))
            .child(self.input_row(
                "retries-reset",
                "Retries",
                Some("How many times to retry after a network error."),
                Slot::Number,
                &self.retries,
                |d| &d.retries,
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
                |s| s.defaults.after_capture.contains(AfterCaptureTasks::UPLOAD_IMAGE_TO_HOST),
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
        let said = match result {
            Ok(m) => (true, m),
            Err(e) => (false, e),
        };
        self.say(said, cx);
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
            // Staged, not written: the running app holds the history database open (and on Windows
            // cannot replace it), and its next save would overwrite the imported settings.
            laqta_library::stage_archive(file.path(), &services.data_dir).map(Some).map_err(|e| e.to_string())
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| match r {
                Ok(Some(n)) => this.report(
                    Ok(format!(
                        "Backup ready ({n} file{}). It replaces your settings and history the next time Laqta starts.",
                        if n == 1 { "" } else { "s" }
                    )),
                    cx,
                ),
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
        let exists_opts = [
            (FileExistsAction::AppendNumber, "Add a number"),
            (FileExistsAction::Overwrite, "Overwrite"),
            (FileExistsAction::Skip, "Skip"),
        ];
        let exists_reset = self.value_reset(
            "exists-reset",
            &exists_opts,
            |s| s.defaults.image.file_exists,
            |this, a, _| this.draft.defaults.image.file_exists = a,
            cx,
        );
        let format_opts = ImageFormat::ALL.map(|f| (f, present::format_label(f)));
        let format_reset = self.value_reset(
            "format-reset",
            &format_opts,
            |s| s.defaults.image.format,
            |this, f, _| this.draft.defaults.image.format = f,
            cx,
        );
        let exists = exists_opts
            .into_iter()
            .map(|(a, l)| {
                self.opt(l, self.draft.defaults.image.file_exists == a, move |this| this.draft.defaults.image.file_exists = a, cx)
            })
            .collect();
        page()
            .child(
                widgets::section("Format", cx)
                    .child(widgets::row_full(
                        "Save as",
                        None::<&str>,
                        Slot::Field,
                        format_reset,
                        widgets::dropdown("format", formats, cx),
                        cx,
                    ))
                    .child(self.input_row(
                        "jpeg-quality-reset",
                        "JPEG quality",
                        Some("1 to 100. Higher looks better and makes bigger files."),
                        Slot::Number,
                        &self.jpeg_quality,
                        |d| &d.jpeg_quality,
                        cx,
                    ))
                    .child(self.input_row(
                        "auto-jpeg-reset",
                        "Switch to JPEG above",
                        Some("Kilobytes. A PNG bigger than this is saved as JPEG instead. Leave empty to never switch."),
                        Slot::Number,
                        &self.auto_jpeg,
                        |d| &d.auto_jpeg,
                        cx,
                    ))
                    .child(widgets::row_full(
                        "If the file exists",
                        None::<&str>,
                        Slot::Hug,
                        exists_reset,
                        widgets::choice("exists", exists, cx),
                        cx,
                    )),
            )
            .child(
                widgets::section("Thumbnails", cx)
                    .child(self.input_row("thumb-w-reset", "Width", Some("Pixels."), Slot::Number, &self.thumb_w, |d| &d.thumb_w, cx))
                    .child(self.input_row(
                        "thumb-h-reset",
                        "Height",
                        Some("Pixels. 0 keeps the aspect ratio."),
                        Slot::Number,
                        &self.thumb_h,
                        |d| &d.thumb_h,
                        cx,
                    )),
            )
    }
    fn naming(&self, cx: &mut Context<Self>) -> gpui::Div {
        let services = &cx.global::<UiState>().services;
        let (now, host) = (services.clock.now(), services.host.clone());
        let preview = |input: &Entity<InputState>, cx: &App| present::name_preview(&Self::value(input, cx), now, &host);
        let (p1, p2, p3) = (preview(&self.pattern, cx), preview(&self.window_pattern, cx), preview(&self.subfolder, cx));
        let muted = cx.theme().muted_foreground;
        let token = |group: &'static str, items: &'static str| {
            div()
                .flex()
                .gap(space::S3)
                .py(px(6.))
                .child(div().w(px(140.)).flex_none().text_size(text::LABEL).font_weight(FontWeight::MEDIUM).child(group))
                .child(div().flex_1().min_w_0().text_size(text::LABEL).text_color(muted).child(items))
        };
        page()
            .child(
                widgets::section("Names", cx)
                    .child(self.input_row("pattern-reset", "File name", Some(format!("Preview: {p1}")), Slot::Field, &self.pattern, |d| &d.pattern, cx))
                    .child(self.input_row("window-pattern-reset", "File name for window captures", Some(format!("Preview: {p2}")), Slot::Field, &self.window_pattern, |d| &d.window_pattern, cx))
                    .child(self.input_row("subfolder-reset", "Subfolder", Some(format!("Preview: {p3}")), Slot::Field, &self.subfolder, |d| &d.subfolder, cx)),
            )
            .child(
                widgets::section("Tokens you can use", cx)
                    .child(token("Date and time", "%y year · %mo month · %d day · %h hour · %mi minute · %s second · %ms millisecond · %unix Unix time · also %yy %mon %mon2 %pm %w %w2 %wy"))
                    .child(token("Window", "%t window title · %pn process name"))
                    .child(token("Counter", "%i · also %ia %iA %iAa %iaA %ix %iX %ib{base,digits} %iB{base,digits}"))
                    .child(token("Random", "%rn number · %ra letters and digits · %guid · %radjective · %ranimal · %remoji · also %rna %rx %rX %GUID %rf{file} (a random line of a text file)"))
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
                    .child(self.input_row("delay-reset", 
                        "Delay",
                        Some("Milliseconds to wait before capturing."),
                        Slot::Number,
                        &self.delay, |d| &d.delay,
                        cx,
                    ))
                    .child(self.sw(
                        "cursor",
                        "Show the cursor in screenshots",
                        Some("Also covers scrolling capture. Videos have their own setting under Recording."),
                        |s| s.defaults.capture.show_cursor,
                        |this, v| this.draft.defaults.capture.show_cursor = v,
                        cx,
                    ))
                    .child(self.sw(
                        "transparent",
                        "Keep window transparency",
                        Some("Keeps rounded corners and shadows when capturing a window. Not available on Wayland."),
                        |s| s.defaults.capture.transparent_window,
                        |this, v| this.draft.defaults.capture.transparent_window = v,
                        cx,
                    ))
                    .child(self.sw(
                        "include-own-windows",
                        "Include Laqta's own windows",
                        Some("Lets you capture Laqta's windows: they stay on screen and can be picked as a window. Off: they hide while you capture."),
                        |s| s.defaults.capture.include_own_windows,
                        |this, v| this.draft.defaults.capture.include_own_windows = v,
                        cx,
                    )),
            )
            .child(
                widgets::section("Selection screen", cx)
                    .child(self.sw(
                        "tools-after-select",
                        "Show tools after selecting",
                        Some("Keep the selection open with drawing tools and Copy, Save, Pin and Copy text. Off: capture as soon as you release the mouse. Space hides the tools."),
                        |s| s.defaults.capture.region.show_tools_after_select,
                        |this, v| this.draft.defaults.capture.region.show_tools_after_select = v,
                        cx,
                    ))
                    .child(self.sw(
                        "mode-bar",
                        "Show the mode bar",
                        Some("The bar at the top of the screen: screenshot or video, full screen, monitor, edit after capture, color picker and text capture."),
                        |s| s.defaults.capture.region.show_mode_bar,
                        |this, v| this.draft.defaults.capture.region.show_mode_bar = v,
                        cx,
                    ))
                    .child(self.sw(
                        "multi",
                        "Allow several regions",
                        Some("Draw several areas, then press Enter or Capture to take them together in one image. Tools that need one area, such as recording and text capture, still take one."),
                        |s| s.defaults.capture.region.multi_region,
                        |this, v| this.draft.defaults.capture.region.multi_region = v,
                        cx,
                    ))
                    .child(self.sw(
                        "snap",
                        "Snap to windows",
                        Some("Hover a window to select all of it with one click."),
                        |s| s.defaults.capture.region.snap_to_windows,
                        |this, v| this.draft.defaults.capture.region.snap_to_windows = v,
                        cx,
                    ))
                    .child(self.sw(
                        "regions",
                        "Detect regions inside windows",
                        Some(if r.snap_to_windows {
                            "Hover a sidebar, panel or text box to select just that part. Scroll to grow to the enclosing one."
                        } else {
                            "Needs Snap to windows, which is off."
                        }),
                        |s| s.defaults.capture.region.detect_regions,
                        |this, v| this.draft.defaults.capture.region.detect_regions = v,
                        cx,
                    ))
                    .child(self.sw(
                        "magnifier",
                        "Show the magnifier",
                        None,
                        |s| s.defaults.capture.region.show_magnifier,
                        |this, v| this.draft.defaults.capture.region.show_magnifier = v,
                        cx,
                    ))
                    .child(self.sw(
                        "crosshair",
                        "Show crosshair lines",
                        None,
                        |s| s.defaults.capture.region.show_crosshair,
                        |this, v| this.draft.defaults.capture.region.show_crosshair = v,
                        cx,
                    ))
                    .child(self.sw(
                        "info",
                        "Show position and color",
                        None,
                        |s| s.defaults.capture.region.show_info,
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
                    .child(self.input_row("custom-region-reset", 
                        "Custom area",
                        Some("x, y, width, height in pixels, used by “Capture custom region”."),
                        Slot::Field,
                        &self.custom_region, |d| &d.custom_region,
                        cx,
                    ))
                    .child(self.input_row("window-title-reset", 
                        "Window title",
                        Some("Used by “Capture window by title”."),
                        Slot::Field,
                        &self.window_title, |d| &d.window_title,
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
    /// Starts the special-key hook while a row records, and stops it when recording ends.
    fn sync_key_hook(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        (cx.global::<UiState>().services.hold_hotkeys)(self.recording.is_some());
        if self.recording.is_none() {
            self.key_hook = None;
            return;
        }
        if self.key_hook.is_some() {
            return;
        }
        let Some(record_keys) = cx.global::<UiState>().services.record_keys.clone() else { return };
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<KeyChord>();
        self.key_hook = record_keys(std::sync::Arc::new(move |chord| {
            let _ = tx.unbounded_send(chord);
        }));
        if self.key_hook.is_none() {
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            use futures_lite::StreamExt as _;
            while let Some(chord) = rx.next().await {
                if this.update_in(cx, |this, window, cx| this.recorded_chord(chord, window, cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
    }

    /// A chord the special-key hook caught for the recording row.
    fn recorded_chord(&mut self, chord: KeyChord, window: &mut Window, cx: &mut Context<Self>) {
        let Some(i) = self.recording else { return };
        self.set_hotkey(i, Some(chord), window, cx);
        self.recording = None;
        self.rec_error = None;
        self.sync_key_hook(window, cx);
        cx.notify();
    }

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
            Recorded::Unusable if self.key_hook.is_some() => {
                // The special-key hook reports keys the window can't name (Windows).
            }
            Recorded::Unusable => {
                self.rec_error = Some(
                    "That key can't be a global shortcut. Try a letter, a number, F1–F12 or PrtSc. Apps never see the Fn key: Fn and a key arrive as some other key, and this one isn't supported.",
                )
            }
        }
        self.sync_key_hook(window, cx);
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
        let bad = matches!(status, RowStatus::Conflict(_) | RowStatus::Failed(_) | RowStatus::NoKey);
        let chip_id = format!("wf-key-{i}");
        let mut chip = div()
            .id(SharedString::from(chip_id.clone()))
            .debug_selector(move || chip_id)
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
                    for cap in crate::presenters::hotkeys::keycaps(c, crate::host_os()) {
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
            this.sync_key_hook(window, cx);
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
        let rows: Vec<(String, Option<KeyChord>, bool)> =
            self.draft.workflows.iter().map(|w| (w.name.clone(), w.hotkey, w.enabled)).collect();
        let statuses = row_status(&rows, &report.registered, &report.failed, &report.system_keys);
        // The desktop told which keys it uses (KDE): it keeps the keys saved in its own settings.
        let desktop_keys = !report.system_keys.is_empty();
        let configure = cx.global::<UiState>().services.configure_shortcuts.clone().filter(|_| desktop_keys);

        // Whether shortcuts can work here at all, in one line, with the switch beside it.
        let (tone, headline, detail): (Tone, String, String) = if off {
            (Tone::Neutral, "Shortcuts are off".into(), "Laqta isn't listening for its shortcuts. Turn them back on to use them.".into())
        } else if let Some(why) = &report.unsupported {
            (
                Tone::Danger,
                "Global shortcuts aren't available on this system".into(),
                if cfg!(target_os = "linux") {
                    format!("{why}. You can still bind `laqta --action <name>` in your desktop's keyboard settings.")
                } else {
                    format!("{why}.")
                },
            )
        } else if report.pending {
            (Tone::Busy, "Registering shortcuts…".into(), "The desktop may ask you to confirm them the first time.".into())
        } else {
            let n = statuses.iter().filter(|s| matches!(s, RowStatus::Active | RowStatus::DesktopKey(_))).count();
            (
                Tone::Success,
                format!("{n} shortcut{} active", if n == 1 { "" } else { "s" }),
                if desktop_keys {
                    "They work in every app. Your desktop keeps the keys saved in its own shortcut settings, so change keys there; Laqta shows the key it uses.".into()
                } else if cfg!(target_os = "linux") {
                    "They work in every app. Changes apply when you save; KDE may ask you to confirm new ones.".into()
                } else {
                    "They work in every app. Changes apply when you save.".into()
                },
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
                .when_some(configure.clone(), |d, configure| {
                    d.child(widgets::pill(
                        "open-desktop-shortcuts",
                        "Open shortcut settings",
                        Pill::Default,
                        true,
                        // The portal round trip can take seconds: keep it off the UI thread.
                        move |_, _, cx| {
                            let configure = configure.clone();
                            cx.background_spawn(async move { configure() }).detach();
                        },
                        cx,
                    ))
                })
                .child(Switch::new("hotkeys-on").checked(!off).on_click(cx.listener(|this, v: &bool, _, cx| {
                    this.draft.disable_hotkeys = !*v;
                    cx.notify();
                }))),
        );

        let pending = statuses.iter().filter(|s| matches!(s, RowStatus::Pending)).count();
        let mut list = div().flex().flex_col().rounded(crate::theme::radius::CARD).border_1().border_color(x.ring).overflow_hidden();
        if pending > 0 {
            let line = if pending == 1 {
                "1 shortcut takes effect when you save.".to_string()
            } else {
                format!("{pending} shortcuts take effect when you save. A grey dot marks them.")
            };
            list = list.child(
                div()
                    .px(space::S4)
                    .py(space::S2)
                    .border_b_1()
                    .border_color(x.ring)
                    .bg(t.foreground.opacity(0.03))
                    .child(widgets::muted(line, cx)),
            );
        }
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
                // One line above the list says it, rather than the same note on every row.
                RowStatus::Pending => (Some(t.muted_foreground), None),
                RowStatus::Conflict(why) => (Some(t.danger), Some(why.clone())),
                RowStatus::Failed(why) => (Some(t.danger), Some(format!("The system refused it: {why}"))),
                RowStatus::DesktopKey(key) => (
                    Some(t.warning),
                    Some(format!(
                        "Your desktop assigned {key} instead, because it keeps the key it stored first. Change it in the desktop's shortcut settings, or remove Laqta's entry there and save again."
                    )),
                ),
                RowStatus::NoKey => (
                    Some(t.danger),
                    Some("Your desktop gave this shortcut no key, so it won't work. Set one in the desktop's shortcut settings.".into()),
                ),
                RowStatus::Disabled | RowStatus::Unset => (None, None),
            };
            // Windows gives bare Print Screen to Snipping Tool: offer to switch that off.
            let free = cx.global::<UiState>().services.free_print_screen.clone();
            let snipping_fix = free
                .filter(|_| {
                    matches!(st, RowStatus::Failed(_))
                        && w.hotkey.is_some_and(|c| c.key == laqta_domain::hotkey::Key::PrintScreen && c.is_bare())
                })
                .map(|free| {
                    widgets::pill(
                        SharedString::from(format!("wf-prtsc-{i}")),
                        "Let Laqta use Print Screen",
                        Pill::Default,
                        true,
                        cx.listener(move |this, _, _, cx| {
                            let services = cx.global::<UiState>().services.clone();
                            let said = match free() {
                                // Saving the stored settings registers the shortcuts again.
                                Ok(()) => match (services.save_settings)(services.app.settings()) {
                                    Ok(()) => {
                                        (true, "Print Screen now goes to Laqta. Windows' own setting comes back if you uninstall.".into())
                                    }
                                    Err(e) => (false, format!("Couldn't register the shortcuts again: {e}")),
                                },
                                Err(e) => (false, format!("Couldn't change the Windows setting: {e}")),
                            };
                            this.say(said, cx);
                            cx.notify();
                        }),
                        cx,
                    )
                });
            // The desktop keeps its own key for this row: send people straight to where they change it.
            let desktop_fix = configure.clone().filter(|_| matches!(st, RowStatus::DesktopKey(_) | RowStatus::NoKey)).map(|open| {
                widgets::pill(
                    SharedString::from(format!("wf-desktop-{i}")),
                    "Open shortcut settings",
                    Pill::Default,
                    true,
                    cx.listener(move |_, _, _, _| open()),
                    cx,
                )
            });
            let recording_here = self.recording == Some(i);
            let note = if recording_here {
                Some(self.rec_error.unwrap_or("Press the keys, Esc to keep the old shortcut, Backspace to remove it.").to_owned())
            } else {
                note
            };
            let overriding = w.patch.after_capture.is_some();
            let hidden = present::hidden_override_labels(&w.patch);
            let reset = settings_reset::default_workflow(w, &self.defaults)
                .filter(|_| settings_reset::shortcut_changed(w, &self.defaults))
                .map(|d| {
                    let (hotkey, enabled) = (d.hotkey, d.enabled);
                    let key =
                        hotkey.map_or_else(|| "none".to_owned(), |c| crate::presenters::hotkeys::keycaps(c, crate::host_os()).join("+"));
                    widgets::Reset::new(
                        SharedString::from(format!("wf-reset-{i}")),
                        format!("Reset to default ({key})"),
                        cx.listener(move |this, _, window, cx| {
                            this.set_hotkey(i, hotkey, window, cx);
                            if let Some(w) = this.draft.workflows.get_mut(i) {
                                w.enabled = enabled;
                            }
                            cx.notify();
                        }),
                    )
                });
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
                                        if !hidden.is_empty() {
                                            parts.push(format!("Own settings: {}", hidden.join(", ")));
                                        }
                                        (!parts.is_empty()).then(|| parts.join(" · "))
                                    },
                                    |d, sub| d.child(div().pl(px(8.)).text_size(text::SMALL).text_color(t.muted_foreground).child(sub)),
                                ),
                        )
                        .when_some(dot, |d, c| d.child(div().size(px(8.)).flex_none().rounded(crate::theme::radius::PILL).bg(c)))
                        .child(widgets::with_reset(reset, self.shortcut_chip(i, w.hotkey, &st, cx), cx))
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
                            std::iter::once(widgets::Choice::new(
                                if overriding { "Use the default after-capture steps" } else { "Give it its own after-capture steps" },
                                false,
                                cx.listener(move |this, _, _, cx| {
                                    let defaults = this.draft.defaults.after_capture;
                                    if let Some(w) = this.draft.workflows.get_mut(i) {
                                        w.patch.after_capture = if w.patch.after_capture.is_some() { None } else { Some(defaults) };
                                    }
                                    cx.notify();
                                }),
                            ))
                            .chain((!hidden.is_empty()).then(|| {
                                widgets::Choice::new(
                                    format!("Use the default settings for {}", hidden.join(", ")),
                                    false,
                                    cx.listener(move |this, _, _, cx| {
                                        if let Some(w) = this.draft.workflows.get_mut(i) {
                                            present::clear_hidden_overrides(&mut w.patch);
                                        }
                                        cx.notify();
                                    }),
                                )
                            }))
                            .chain(std::iter::once(widgets::Choice::new(
                                "Remove shortcut",
                                false,
                                cx.listener(move |this, _, window, cx| {
                                    if this.collect(cx).is_ok() && i < this.draft.workflows.len() {
                                        this.draft.workflows.remove(i);
                                        this.recording = None;
                                        this.sync_key_hook(window, cx);
                                        this.rebuild_workflow_inputs(window, cx);
                                    }
                                    cx.notify();
                                }),
                            )))
                            .collect(),
                        )),
                )
                .when_some(note, |d, n| {
                    let danger = matches!(st, RowStatus::Conflict(_) | RowStatus::Failed(_) | RowStatus::NoKey)
                        || (recording_here && self.rec_error.is_some());
                    d.child(div().pl(px(48.)).child(widgets::notice(n, !danger, cx)))
                })
                .when_some(snipping_fix, |d, button| d.child(div().pl(px(48.)).child(button)))
                .when_some(desktop_fix, |d, button| d.child(div().pl(px(48.)).child(button)));
            let row = match w.patch.after_capture {
                Some(flags) => row.child(div().pl(px(48.)).child(self.after_capture_list(
                    &format!("wf-{i}"),
                    flags,
                    None,
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
                        this.sync_key_hook(window, cx);
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
            let listener = cx.listener(move |this, _, window, cx| {
                this.page = page;
                this.highlight = None;
                if !this.query.is_empty() {
                    this.query.clear();
                    this.search.update(cx, |s, cx| s.set_value("", window, cx));
                }
                // Leaving the page ends a recording, so the global shortcuts work again.
                this.recording = None;
                this.sync_key_hook(window, cx);
                cx.notify();
            });
            // While searching, no page is the current one: the results stand in for a page.
            let current = self.page == page && self.query.trim().is_empty();
            nav = nav.child(widgets::nav_row(SharedString::from(format!("nav-{label}")), glyph, label, current, listener, cx));
        }
        let searching = !self.query.trim().is_empty();
        let (title, description, content) = if searching {
            let (heading, results) = self.search_results(window, cx);
            ("Search".to_owned(), heading, results)
        } else {
            let (title, description) = page_header(self.page);
            let content = widgets::with_highlight(self.highlight.clone(), || self.page_content(self.page, window, cx));
            (title.to_owned(), description.to_owned(), content)
        };
        let search_box = div()
            .px(space::S3)
            .pt(space::S1)
            .pb(space::S2)
            .debug_selector(|| "settings-search".into())
            .child(Input::new(&self.search).cleanable(true).prefix(widgets::icon(IconName::Search, cx.theme().muted_foreground)));
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
        let line = self.flash.render(cx.listener(|this, _, _, cx| crate::flash::Flash::dismiss(this, cx)), cx);
        footer = footer.child(div().flex_1().min_w_0().children(line));
        footer = footer.child(widgets::pill("save", "Save", Pill::Primary, false, cx.listener(|this, _, _, cx| this.save(cx)), cx));
        let settings_label =
            div().font_family(text::HEADING_FONT).font_weight(FontWeight::SEMIBOLD).text_size(text::SECTION).child("Settings");
        let body = div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .child(
                // One scroll position per page: a page opens at its top, not where the last one was left.
                div().id("settings-scroll").flex_1().overflow_y_scrollbar().id(("settings-scroll", self.page as usize)).child(
                    div()
                        .max_w(px(780.))
                        .mx_auto()
                        .px(space::S8)
                        .pt(space::S6)
                        .pb(space::S10)
                        .flex()
                        .flex_col()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap(space::S4)
                                .child(widgets::settings_header(title, description, cx).flex_1().min_w_0())
                                .when(!searching, |d| d.children(self.page_reset(cx))),
                        )
                        .when(!searching, |d| d.children(self.override_note(cx)))
                        .child(content),
                ),
            )
            .child(footer);
        // Ctrl+F (Cmd+F on macOS) jumps to the search field; Escape there empties it.
        let find_keys = cx.listener(|this, ev: &gpui::KeyDownEvent, window, cx| {
            let k = &ev.keystroke;
            let chord = if cfg!(target_os = "macos") { k.modifiers.platform } else { k.modifiers.control };
            if chord && k.key == "f" {
                let handle = gpui::Focusable::focus_handle(this.search.read(cx), cx);
                window.focus(&handle, cx);
                cx.stop_propagation();
            } else if k.key == "escape"
                && !this.query.is_empty()
                && gpui::Focusable::focus_handle(this.search.read(cx), cx).is_focused(window)
            {
                this.query.clear();
                this.search.update(cx, |s, cx| s.set_value("", window, cx));
                cx.notify();
                cx.stop_propagation();
            }
        });
        widgets::tab_keys(crate::chrome::rounded(widgets::page(cx), window).bg(cx.theme().sidebar), &self.focus)
            .capture_key_down(find_keys)
            .child(crate::chrome::TitleBar::new().leading(settings_label, px(232.)))
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(div().flex_none().flex().flex_col().child(search_box).child(nav))
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

/// The settings a page's "Reset page" covers; `None` for pages without one.
fn page_scope(page: Page) -> Option<Scope> {
    Some(match page {
        Page::General => Scope::General,
        Page::AfterCapture => Scope::AfterCapture,
        Page::Image => Scope::Image,
        Page::Naming => Scope::Naming,
        Page::Capture => Scope::Capture,
        Page::Workflows => Scope::Shortcuts,
        Page::Uploads => Scope::Uploads,
        Page::Recording => Scope::Recording,
        Page::Automation => Scope::Automation,
        Page::Models => Scope::Models,
        Page::Integration => Scope::Integration,
        Page::Data => return None,
    })
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

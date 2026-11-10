//! The image effects window: presets on the left, the selected preset's effect stack and the
//! selected effect's settings in the middle, and a live preview on a sample image on the right.
//! Saving writes `Settings.effect_presets` through `save_settings`. Each parameter gets a control
//! from `presenters::effect_params`; JSON stays available for the ones the form can't show.

use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, Context, Entity, FontWeight, InteractiveElement as _, IntoElement, ParentElement as _, Render, RenderImage,
    SharedString, StatefulInteractiveElement as _, Styled as _, Subscription, Window, WindowOptions, div, img, prelude::FluentBuilder as _,
    px, relative, size,
};
use gpui_component::input::{Input, InputEvent, InputState, NumberInput, Textarea, TextareaState};
use gpui_component::switch::Switch;
use gpui_component::{ActiveTheme as _, Icon, Root, Sizable as _};
use laqta_domain::Bitmap;
use laqta_domain::effects::{Adjust, Draw, Effect, EffectItem, Filter, Manip, Preset};

use crate::assets::IconName;
use crate::presenters::effect_params::{self as params, Field, FieldKind, FieldValue};
use crate::runtime::UiState;
use crate::theme::{self, radius, space, text};
use crate::widgets::{self, Choice, Pill, Segment};

const SIDEBAR_W: f32 = 236.0;
const PANE_W: f32 = 404.0;
/// The preview's narrowest width: its header holds the two segments and the sample button.
const PREVIEW_MIN_W: f32 = 300.0;
/// The settings column inside a row: wide enough for a number stepper or a dropdown.
const CONTROL_W: f32 = 176.0;
/// The header band every column shares, so their titles line up.
const HEADER_H: f32 = 56.0;

pub(crate) fn open(path: Option<PathBuf>, cx: &mut App) {
    let Some(p) = path else { return open_on(None, None, cx) };
    // Read, decode and convert the file off the GPUI thread; a file that doesn't decode opens on the sample.
    let app = cx.global::<UiState>().services.app.clone();
    let file = p.clone();
    let task = cx.background_spawn(async move {
        let decoded = app.decode_file(file).await;
        decoded.map(|image| {
            let original = crate::toast::render_image(&image);
            (image, original)
        })
    });
    cx.spawn(async move |cx| {
        let decoded = task.await;
        if let Err(e) = &decoded {
            tracing::warn!(path = %p.display(), "effects: cannot open the image: {e}");
        }
        cx.update(|cx| open_on(Some(p), decoded.ok(), cx));
    })
    .detach();
}

/// Opens the window on `opened` (the decoded image at `path` and its GPUI copy), or on the sample image.
fn open_on(path: Option<PathBuf>, opened: Option<(Bitmap, Option<Arc<RenderImage>>)>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(crate::chrome::centered(1180., 780., cx)),
        titlebar: Some(crate::chrome::titlebar("Image effects")),
        window_decorations: crate::chrome::decorations(),
        app_owns_titlebar_drag: true,
        // The sidebar, the pane, and a preview wide enough for its header (segments + button).
        window_min_size: Some(size(px(SIDEBAR_W + PANE_W + PREVIEW_MIN_W), px(560.))),
        app_id: Some(crate::APP_ID.into()),
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| EffectsWindow::new(path, opened, window, cx));
        cx.new(|cx| Root::new(view, window, cx))
    }) {
        tracing::error!("effects window failed: {e:#}");
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Category {
    Adjust,
    Draw,
    Filter,
    Manip,
}

impl Category {
    const ALL: [Category; 4] = [Category::Adjust, Category::Draw, Category::Filter, Category::Manip];

    fn label(self) -> &'static str {
        match self {
            Category::Adjust => "Adjust",
            Category::Draw => "Draw & frame",
            Category::Filter => "Filter",
            Category::Manip => "Transform",
        }
    }
}

/// One settings row of the selected effect: the field and, for typed values, its input.
struct FieldUi {
    field: Field,
    input: Option<Entity<InputState>>,
}

struct EffectsWindow {
    /// `TaskSettings.effect_preset`: which preset the after-capture step applies (`*` = random).
    after_capture: String,
    presets: Vec<Preset>,
    /// What was last saved, to tell the user about unsaved changes.
    saved: (Vec<Preset>, String),
    selected: usize,
    selected_effect: Option<usize>,
    category: Category,
    /// The effect library replaces the settings while the user picks an effect to add.
    adding: bool,
    /// The selected effect's settings as JSON instead of the form.
    json: bool,
    params: Entity<TextareaState>,
    fields: Vec<FieldUi>,
    _field_subs: Vec<Subscription>,
    /// Filters the effect library by name.
    search: Entity<InputState>,
    name: Entity<InputState>,
    sample: Bitmap,
    sample_name: String,
    original: Option<Arc<RenderImage>>,
    preview: Option<Arc<RenderImage>>,
    show_original: bool,
    /// A message and whether it is good news.
    message: Option<(String, bool)>,
}

/// A checkerboard-and-gradient sample, used until the user opens an image.
fn sample_image() -> Bitmap {
    let (w, h) = (480u32, 300u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let check = ((x / 30 + y / 30) % 2) as u8;
            px[i] = (x * 255 / w) as u8;
            px[i + 1] = (y * 255 / h) as u8;
            px[i + 2] = 160 + check * 60;
            px[i + 3] = 255;
        }
    }
    Bitmap::new(w, h, laqta_domain::PixelFormat::Rgba8, px.into()).unwrap_or_else(|_| unreachable!("sample size"))
}

fn fmt_number(v: f64, integer: bool) -> String {
    if integer {
        format!("{v:.0}")
    } else {
        let s = format!("{:.3}", v);
        s.trim_end_matches('0').trim_end_matches('.').to_string()
    }
}

fn category_of(e: &Effect) -> &'static str {
    match e {
        Effect::Adjust(_) => "Adjust",
        Effect::Draw(_) => "Draw",
        Effect::Filter(_) => "Filter",
        Effect::Manip(_) => "Transform",
    }
}

impl EffectsWindow {
    fn new(path: Option<PathBuf>, opened: Option<(Bitmap, Option<Arc<RenderImage>>)>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        let settings = services.app.settings();
        let mut presets = settings.effect_presets.clone();
        if presets.is_empty() {
            presets.push(Preset { name: "Preset 1".into(), effects: Vec::new() });
        }
        let sample_name = match (&opened, &path) {
            (Some(_), Some(p)) => p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
            _ => "Sample image".into(),
        };
        let (sample, original) = match opened {
            Some((image, original)) => (image, original),
            None => {
                let sample = sample_image();
                let original = crate::toast::render_image(&sample);
                (sample, original)
            }
        };
        let params = cx.new(|cx| TextareaState::new(window, cx));
        cx.subscribe(&params, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) && this.json {
                this.apply_json(cx);
            }
        })
        .detach();
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("Search effects"));
        cx.subscribe(&search, |_: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        let first = presets[0].name.clone();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(first).placeholder("Preset name"));
        cx.subscribe(&name, |this: &mut Self, input, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                let n = input.read(cx).value().trim().to_string();
                if !n.is_empty() {
                    let old = this.preset().name.clone();
                    if this.after_capture == old {
                        this.after_capture = n.clone();
                    }
                    this.preset().name = n;
                }
                cx.notify();
            }
        })
        .detach();
        let saved = (presets.clone(), settings.defaults.effect_preset.clone());
        let mut this = EffectsWindow {
            after_capture: settings.defaults.effect_preset.clone(),
            presets,
            saved,
            selected: 0,
            selected_effect: None,
            category: Category::Adjust,
            adding: false,
            json: false,
            params,
            fields: Vec::new(),
            _field_subs: Vec::new(),
            search,
            name,
            sample,
            sample_name,
            original,
            preview: None,
            show_original: false,
            message: None,
        };
        if !this.presets[0].effects.is_empty() {
            this.select_effect(0, window, cx);
        }
        this.render_preview(cx);
        this
    }

    fn preset(&mut self) -> &mut Preset {
        let i = self.selected.min(self.presets.len().saturating_sub(1));
        &mut self.presets[i]
    }

    fn current_effect(&self) -> Option<&Effect> {
        let i = self.selected_effect?;
        self.presets.get(self.selected)?.effects.get(i).map(|it| &it.effect)
    }

    fn dirty(&self) -> bool {
        self.presets != self.saved.0 || self.after_capture != self.saved.1
    }

    fn select_preset(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected = i.min(self.presets.len().saturating_sub(1));
        let n = self.presets[self.selected].name.clone();
        self.name.update(cx, |input, cx| input.set_value(n, window, cx));
        if self.presets[self.selected].effects.is_empty() {
            self.clear_selection();
        } else {
            self.select_effect(0, window, cx);
        }
        self.render_preview(cx);
        cx.notify();
    }

    fn clear_selection(&mut self) {
        self.selected_effect = None;
        self.fields.clear();
        self._field_subs.clear();
    }

    /// Selects an effect and builds a control for each of its parameters.
    fn select_effect(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_effect = Some(i);
        self.adding = false;
        self.fields.clear();
        self._field_subs.clear();
        let Some(effect) = self.current_effect().cloned() else {
            self.selected_effect = None;
            return;
        };
        for (idx, field) in params::form(&effect).fields.into_iter().enumerate() {
            let input = match &field.kind {
                FieldKind::Number { value, integer, step } => {
                    let (value, integer, step) = (*value, *integer, *step);
                    Some(cx.new(|cx| InputState::new(window, cx).default_value(fmt_number(value, integer)).step(step)))
                }
                FieldKind::Color(c) => Some(cx.new(|cx| InputState::new(window, cx).default_value(params::hex(*c)))),
                FieldKind::Text(s) => Some(cx.new(|cx| InputState::new(window, cx).default_value(s.clone()))),
                FieldKind::Toggle(_) | FieldKind::Choice { .. } => None,
            };
            if let Some(input) = &input {
                self._field_subs.push(cx.subscribe(input, move |this: &mut Self, input, ev: &InputEvent, cx| {
                    if matches!(ev, InputEvent::Change) {
                        let text = input.read(cx).value().to_string();
                        this.typed(idx, &text, cx);
                    }
                }));
            }
            self.fields.push(FieldUi { field, input });
        }
        let json = params::to_text(&effect);
        self.params.update(cx, |input, cx| input.set_value(json, window, cx));
        cx.notify();
    }

    /// Text typed into a field. Half-typed values ("-", "#3") are ignored until they parse.
    fn typed(&mut self, idx: usize, text: &str, cx: &mut Context<Self>) {
        let Some(ui) = self.fields.get(idx) else { return };
        let value = match ui.field.kind {
            FieldKind::Number { .. } => text.trim().parse::<f64>().ok().map(FieldValue::Number),
            FieldKind::Color(_) => params::parse_hex(text).map(FieldValue::Color),
            FieldKind::Text(_) => Some(FieldValue::Text(text.to_string())),
            _ => None,
        };
        if let Some(v) = value {
            self.set_field(idx, v, cx);
        }
    }

    fn set_field(&mut self, idx: usize, value: FieldValue, cx: &mut Context<Self>) {
        let (Some(i), Some(ui)) = (self.selected_effect, self.fields.get(idx)) else { return };
        let Some(effect) = self.current_effect().cloned() else { return };
        match params::set(&effect, &ui.field.path, value) {
            Some(changed) => {
                let form = params::form(&changed);
                for (ui, f) in self.fields.iter_mut().zip(form.fields) {
                    ui.field = f;
                }
                if let Some(item) = self.preset().effects.get_mut(i) {
                    item.effect = changed;
                }
                self.message = None;
                self.render_preview(cx);
            }
            None => self.message = Some((format!("{} can't take that value.", ui.field.label), false)),
        }
        cx.notify();
    }

    fn apply_json(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.selected_effect else { return };
        let text = self.params.read(cx).value().to_string();
        match params::from_text(&text) {
            Ok(e) => {
                if let Some(item) = self.preset().effects.get_mut(i) {
                    item.effect = e;
                }
                self.message = None;
                self.render_preview(cx);
            }
            Err(e) => self.message = Some((format!("JSON: {e}"), false)),
        }
        cx.notify();
    }

    fn add_effect(&mut self, e: Effect, window: &mut Window, cx: &mut Context<Self>) {
        self.preset().effects.push(EffectItem { effect: e, enabled: true });
        let n = self.preset().effects.len() - 1;
        self.select_effect(n, window, cx);
        self.render_preview(cx);
    }

    fn move_effect(&mut self, i: usize, up: bool, window: &mut Window, cx: &mut Context<Self>) {
        let len = self.preset().effects.len();
        let j = if up { i.checked_sub(1) } else { (i + 1 < len).then_some(i + 1) };
        if let Some(j) = j {
            self.preset().effects.swap(i, j);
            if self.selected_effect == Some(i) {
                self.select_effect(j, window, cx);
            } else if self.selected_effect == Some(j) {
                self.select_effect(i, window, cx);
            }
            self.render_preview(cx);
        }
        cx.notify();
    }

    fn remove_effect(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        if i >= self.preset().effects.len() {
            return;
        }
        self.preset().effects.remove(i);
        let len = self.preset().effects.len();
        match self.selected_effect {
            Some(s) if s == i && len > 0 => self.select_effect(i.min(len - 1), window, cx),
            Some(s) if s == i => self.clear_selection(),
            Some(s) if s > i => self.select_effect(s - 1, window, cx),
            _ => {}
        }
        self.render_preview(cx);
        cx.notify();
    }

    fn render_preview(&mut self, cx: &mut Context<Self>) {
        let preset = self.preset().clone();
        let sample = self.sample.clone();
        let spawner = cx.global::<UiState>().services.spawner.clone();
        let (tx, rx) = oneshot::channel();
        let fut = spawner.run_compute(Box::new(move || {
            let cx = laqta_imaging::effects::EffectContext::none();
            let _ = tx.send(laqta_imaging::effects::apply_preset(&sample, &preset, &cx));
        }));
        cx.spawn(async move |this, cx| {
            fut.await;
            let result = rx.await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(Ok(img)) => this.preview = crate::toast::render_image(&img),
                    Ok(Err(e)) => this.message = Some((format!("Preview: {e}"), false)),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// Lets the user preview on one of their own images.
    fn open_sample(&mut self, cx: &mut Context<Self>) {
        let codecs = cx.global::<UiState>().services.codecs.clone();
        let task = cx.background_spawn(async move {
            let f = rfd::AsyncFileDialog::new()
                .add_filter("Images", &["png", "jpg", "jpeg", "gif", "bmp", "tif", "tiff", "webp"])
                .pick_file()
                .await?;
            let bytes = std::fs::read(f.path()).ok()?;
            let name = f.file_name();
            codecs.decode(&bytes).ok().map(|b| (b, name))
        });
        cx.spawn(async move |this, cx| {
            if let Some((bmp, name)) = task.await {
                let _ = this.update(cx, |this, cx| {
                    this.original = crate::toast::render_image(&bmp);
                    this.sample = bmp;
                    this.sample_name = name;
                    this.render_preview(cx);
                    cx.notify();
                });
            }
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let mut settings = services.app.settings();
        settings.effect_presets = self.presets.clone();
        settings.defaults.effect_preset = self.after_capture.clone();
        self.message = Some(match (services.save_settings)(settings) {
            Ok(()) => {
                self.saved = (self.presets.clone(), self.after_capture.clone());
                ("Presets saved.".into(), true)
            }
            Err(e) => (format!("Couldn't save: {e}"), false),
        });
        cx.notify();
    }

    fn import_sxie(&mut self, cx: &mut Context<Self>) {
        let read = cx.global::<UiState>().services.sxie_read.clone();
        let task = cx.background_spawn(async move {
            let Some(file) = rfd::AsyncFileDialog::new().add_filter("ShareX effect", &["sxie"]).pick_file().await else { return Ok(None) };
            let bytes = std::fs::read(file.path()).map_err(|e| e.to_string())?;
            read(&bytes).map(Some)
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(preset)) => {
                        this.message = Some((format!("Imported \"{}\". Save to keep it.", preset.name), true));
                        this.presets.retain(|p| p.name != preset.name);
                        this.presets.push(preset);
                        this.selected = this.presets.len() - 1;
                        this.selected_effect = None;
                        this.render_preview(cx);
                    }
                    Ok(None) => {}
                    Err(e) => this.message = Some((format!("Import failed: {e}"), false)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn export_sxie(&mut self, cx: &mut Context<Self>) {
        let Some(preset) = self.presets.get(self.selected).cloned() else { return };
        let write = cx.global::<UiState>().services.sxie_write.clone();
        let task = cx.background_spawn(async move {
            let bytes = write(&preset)?;
            let Some(file) = rfd::AsyncFileDialog::new().set_file_name(format!("{}.sxie", preset.name)).save_file().await else {
                return Ok(None);
            };
            std::fs::write(file.path(), bytes).map_err(|e| e.to_string())?;
            Ok::<_, String>(Some(file.path().display().to_string()))
        });
        cx.spawn(async move |this, cx| {
            let r = task.await;
            let _ = this.update(cx, |this, cx| {
                match r {
                    Ok(Some(path)) => this.message = Some((format!("Exported to {path}."), true)),
                    Ok(None) => {}
                    Err(e) => this.message = Some((format!("Export failed: {e}"), false)),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn defaults(&self) -> Vec<Effect> {
        match self.category {
            Category::Adjust => Adjust::defaults().into_iter().map(Effect::Adjust).collect(),
            Category::Draw => Draw::defaults().into_iter().map(Effect::Draw).collect(),
            Category::Filter => Filter::defaults().into_iter().map(Effect::Filter).collect(),
            Category::Manip => Manip::defaults().into_iter().map(Effect::Manip).collect(),
        }
    }

    fn sidebar(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        let mut list = div().flex().flex_col().gap(px(2.));
        for (i, p) in self.presets.iter().enumerate() {
            let active = i == self.selected;
            let n = p.effects.len();
            let mut row = div()
                .id(("preset", i))
                .h(px(36.))
                .px(space::S3)
                .flex()
                .items_center()
                .gap(space::S3)
                .rounded(radius::MD)
                .border_1()
                .border_color(gpui::transparent_black())
                .text_size(text::ROW)
                .text_color(if active { t.foreground } else { t.foreground.opacity(0.86) })
                .hover(move |s| s.bg(x.wash))
                .child(widgets::icon(IconName::Layers, if active { t.foreground } else { t.muted_foreground }))
                .child(div().flex_1().min_w_0().truncate().child(p.name.clone()))
                .child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(n.to_string()));
            if active {
                row = row.bg(t.accent).font_weight(FontWeight::MEDIUM);
            }
            list = list.child(widgets::clickable(row, cx.listener(move |this, _, w, cx| this.select_preset(i, w, cx)), cx));
        }

        let random = self.after_capture == "*";
        let mut options: Vec<Choice> = self
            .presets
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let name = p.name.clone();
                let chosen = !random && (self.after_capture == name || (self.after_capture.is_empty() && i == 0));
                Choice::new(
                    name.clone(),
                    chosen,
                    cx.listener(move |this, _, _, cx| {
                        this.after_capture = name.clone();
                        cx.notify();
                    }),
                )
            })
            .collect();
        options.push(Choice::new(
            "A random preset",
            random,
            cx.listener(|this, _, _, cx| {
                this.after_capture = "*".into();
                cx.notify();
            }),
        ));

        let ghost_row = |id: &'static str, glyph: IconName, label: &'static str, on: widgets::OnClick| {
            let row = div()
                .id(id)
                .h(px(32.))
                .px(space::S3)
                .flex()
                .items_center()
                .gap(space::S3)
                .rounded(radius::MD)
                .border_1()
                .border_color(gpui::transparent_black())
                .text_size(text::LABEL)
                .text_color(t.foreground.opacity(0.86))
                .hover(move |s| s.bg(x.wash))
                .child(widgets::icon(glyph, t.muted_foreground))
                .child(label);
            widgets::clickable(row, on, cx)
        };

        div()
            .w(px(SIDEBAR_W))
            .flex_none()
            .h_full()
            .flex()
            .flex_col()
            .px(space::S3)
            .pb(space::S4)
            .child(
                div()
                    .h(px(HEADER_H))
                    .flex_none()
                    .pl(space::S3)
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .text_size(text::CAPTION)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(t.muted_foreground)
                            .child("Presets"),
                    )
                    .child(widgets::icon_button(
                        "preset-add",
                        IconName::Plus,
                        "New preset",
                        false,
                        cx.listener(|this, _, w, cx| {
                            let n = this.presets.len() + 1;
                            this.presets.push(Preset { name: format!("Preset {n}"), effects: Vec::new() });
                            let last = this.presets.len() - 1;
                            this.select_preset(last, w, cx);
                        }),
                        cx,
                    )),
            )
            .child(div().id("preset-list").flex_shrink_1().min_h_0().overflow_y_scroll().child(list))
            .child(
                div()
                    .mt(space::S4)
                    .p(space::S3)
                    .flex()
                    .flex_col()
                    .gap(space::S2)
                    .rounded(radius::LG)
                    .bg(t.foreground.opacity(0.04))
                    .child(div().text_size(text::LABEL).font_weight(FontWeight::MEDIUM).child("Use after each capture"))
                    .child(
                        div()
                            .text_size(text::CAPTION)
                            .text_color(t.muted_foreground)
                            .child("Applied when \u{201C}Apply image effects\u{201D} is on in After capture settings."),
                    )
                    .child(widgets::dropdown("after-capture", options, cx)),
            )
            .child(div().flex_1())
            .child(div().h(px(1.)).mx(space::S2).mb(space::S2).bg(t.foreground.opacity(0.08)))
            .child(ghost_row(
                "sxie-import",
                IconName::FolderOpen,
                "Import .sxie…",
                Box::new(cx.listener(|this, _, _, cx| this.import_sxie(cx))),
            ))
            .child(ghost_row(
                "sxie-export",
                IconName::Upload,
                "Export .sxie…",
                Box::new(cx.listener(|this, _, _, cx| this.export_sxie(cx))),
            ))
    }

    fn effect_row(&self, i: usize, item: &EffectItem, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        let selected = self.selected_effect == Some(i);
        let len = self.presets.get(self.selected).map(|p| p.effects.len()).unwrap_or(0);
        let group: SharedString = format!("fx-row-{i}").into();
        let (fg, muted, wash) = (t.foreground, t.muted_foreground, x.wash);
        let small = |id: String, glyph: IconName, tip: &'static str, on: widgets::OnClick| {
            let el = div()
                .id(SharedString::from(id))
                .size(px(28.))
                .flex()
                .items_center()
                .justify_center()
                .rounded(radius::SM)
                .border_1()
                .border_color(gpui::transparent_black())
                .text_color(muted)
                .hover(move |s| s.bg(wash).text_color(fg))
                .tooltip(move |w, cx| gpui_component::tooltip::Tooltip::new(tip).build(w, cx))
                .child(Icon::new(glyph).size(px(14.)));
            // The row under the button selects on click; this click is the button's alone.
            widgets::clickable(
                el,
                move |e, w, cx| {
                    cx.stop_propagation();
                    on(e, w, cx)
                },
                cx,
            )
        };
        let mut actions = div().flex().items_center().opacity(if selected { 1. } else { 0. }).group_hover(group.clone(), |s| s.opacity(1.));
        if i > 0 {
            actions = actions.child(small(
                format!("fx-up-{i}"),
                IconName::ArrowUp,
                "Move up",
                Box::new(cx.listener(move |this, _, w, cx| this.move_effect(i, true, w, cx))),
            ));
        }
        if i + 1 < len {
            actions = actions.child(small(
                format!("fx-down-{i}"),
                IconName::ArrowDown,
                "Move down",
                Box::new(cx.listener(move |this, _, w, cx| this.move_effect(i, false, w, cx))),
            ));
        }
        let actions = actions.child(small(
            format!("fx-rm-{i}"),
            IconName::Trash,
            "Remove",
            Box::new(cx.listener(move |this, _, w, cx| this.remove_effect(i, w, cx))),
        ));
        let enabled = item.enabled;
        let name_color = if enabled { t.foreground } else { t.muted_foreground };
        let mut row = div()
            .id(("fx", i))
            .group(group)
            .h(px(48.))
            .pl(space::S3)
            .pr(space::S1)
            .flex()
            .items_center()
            .gap(space::S3)
            .rounded(radius::MD)
            .border_1()
            .border_color(gpui::transparent_black())
            .hover(move |s| s.bg(x.wash))
            .child(div().w(px(18.)).text_size(text::CAPTION).text_color(t.muted_foreground).child(format!("{}", i + 1)))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .truncate()
                            .text_size(text::ROW)
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(name_color)
                            .child(params::display_name(&item.effect)),
                    )
                    .child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(if enabled {
                        category_of(&item.effect)
                    } else {
                        "Off"
                    })),
            )
            .child(actions)
            .child(div().pl(space::S1).child(Switch::new(("fx-on", i)).checked(enabled).small().on_click(cx.listener(
                move |this, v: &bool, _, cx| {
                    if let Some(it) = this.preset().effects.get_mut(i) {
                        it.enabled = *v;
                    }
                    this.render_preview(cx);
                    cx.notify();
                },
            ))));
        if selected {
            row = row.bg(t.accent);
        }
        widgets::clickable(row, cx.listener(move |this, _, w, cx| this.select_effect(i, w, cx)), cx)
    }

    fn library(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        let segs = Category::ALL
            .into_iter()
            .map(|c| Segment {
                id: format!("cat-{}", c.label()).into(),
                label: c.label().into(),
                selected: self.category == c,
                on_click: Box::new(cx.listener(move |this, _, _, cx| {
                    this.category = c;
                    cx.notify();
                })),
            })
            .collect();
        let query = self.search.read(cx).value().trim().to_lowercase();
        let mut grid = div().flex().flex_wrap().gap(px(6.));
        let mut shown = 0;
        for (i, e) in self.defaults().into_iter().enumerate() {
            let name = params::display_name(&e);
            if !query.is_empty() && !name.to_lowercase().contains(&query) && !e.name().to_lowercase().contains(&query) {
                continue;
            }
            shown += 1;
            let tile = div()
                .id(("add", i))
                .w(px((PANE_W - 2. * 20. - 6.) / 2. - 2.))
                .h(px(36.))
                .px(space::S3)
                .flex()
                .items_center()
                .gap(space::S2)
                .rounded(radius::MD)
                .border_1()
                .border_color(gpui::transparent_black())
                .bg(t.foreground.opacity(0.045))
                .text_size(text::LABEL)
                .hover(move |s| s.bg(x.wash.opacity(2.4)))
                .child(div().flex_1().min_w_0().truncate().child(name))
                .child(Icon::new(IconName::Plus).size(px(13.)).text_color(t.muted_foreground));
            grid = grid.child(widgets::clickable(tile, cx.listener(move |this, _, w, cx| this.add_effect(e.clone(), w, cx)), cx));
        }
        let mut body = div()
            .flex()
            .flex_col()
            .gap(space::S3)
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .font_family(text::HEADING_FONT)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(text::SECTION)
                            .child("Add an effect"),
                    )
                    .child(widgets::icon_button(
                        "library-close",
                        IconName::X,
                        "Close",
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.adding = false;
                            cx.notify();
                        }),
                        cx,
                    )),
            )
            .child(widgets::segmented(segs, cx))
            .child(
                Input::new(&self.search).prefix(Icon::new(IconName::Search).size(px(14.)).text_color(t.muted_foreground)).cleanable(true),
            );
        body = if shown == 0 {
            body.child(widgets::muted("No effects match your search in this category.", cx).py(space::S4))
        } else {
            body.child(grid)
        };
        body
    }

    fn inspector(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let Some(effect) = self.current_effect().cloned() else {
            return div()
                .child(widgets::muted("Pick an effect in the list to change its settings.", cx).text_size(text::LABEL))
                .into_any_element();
        };
        let form = params::form(&effect);
        let header = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .child(
                        div()
                            .font_family(text::HEADING_FONT)
                            .font_weight(FontWeight::MEDIUM)
                            .text_size(text::SECTION)
                            .child(params::display_name(&effect)),
                    )
                    .child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(params::summary(&effect))),
            )
            .child(widgets::pill(
                "json",
                if self.json { "Form" } else { "JSON" },
                Pill::Ghost,
                true,
                cx.listener(|this, _, w, cx| {
                    this.json = !this.json;
                    if !this.json
                        && let Some(i) = this.selected_effect
                    {
                        this.select_effect(i, w, cx);
                    }
                    cx.notify();
                }),
                cx,
            ));
        let mut body = div().flex().flex_col().gap(space::S1).child(header.pb(space::S2));
        if self.json {
            return body.child(Textarea::new(&self.params).h(px(240.)).font_family("monospace").text_size(text::SMALL)).into_any_element();
        }
        if self.fields.is_empty() {
            body = body.child(
                widgets::muted(
                    if form.json_only { "This effect's settings can only be edited as JSON." } else { "This effect has no settings." },
                    cx,
                )
                .text_size(text::LABEL),
            );
            return body.into_any_element();
        }
        for (idx, ui) in self.fields.iter().enumerate() {
            let control = self.control(idx, ui, cx);
            body = body.child(
                div()
                    .min_h(px(44.))
                    .flex()
                    .items_center()
                    .gap(space::S3)
                    .child(
                        div().flex_1().min_w_0().text_size(text::LABEL).text_color(t.foreground.opacity(0.9)).child(ui.field.label.clone()),
                    )
                    .child(div().w(px(CONTROL_W)).flex_none().flex().justify_end().child(control)),
            );
        }
        if form.json_only {
            body = body.child(widgets::muted("More settings are available as JSON.", cx).pt(space::S2));
        }
        body.into_any_element()
    }

    fn control(&self, idx: usize, ui: &FieldUi, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = cx.theme().clone();
        match (&ui.field.kind, &ui.input) {
            (FieldKind::Number { .. }, Some(input)) => {
                let mut n = NumberInput::new(input).small().w_full();
                if let Some(u) = params::unit(&ui.field) {
                    n = n.suffix(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(u));
                }
                n.into_any_element()
            }
            (FieldKind::Color(c), Some(input)) => {
                let mut swatch_color: gpui::Hsla = gpui::rgb(((c.r as u32) << 16) | ((c.g as u32) << 8) | c.b as u32).into();
                swatch_color.a = c.a as f32 / 255.;
                Input::new(input)
                    .small()
                    .w_full()
                    .prefix(div().size(px(16.)).rounded(radius::PILL).bg(swatch_color).border_1().border_color(t.foreground.opacity(0.25)))
                    .into_any_element()
            }
            (FieldKind::Text(_), Some(input)) => Input::new(input).small().w_full().into_any_element(),
            (FieldKind::Toggle(on), _) => Switch::new(("field-switch", idx))
                .checked(*on)
                .on_click(cx.listener(move |this, v: &bool, _, cx| this.set_field(idx, FieldValue::Bool(*v), cx)))
                .into_any_element(),
            (FieldKind::Choice { options, selected }, _) => {
                let choices = options
                    .iter()
                    .enumerate()
                    .map(|(i, o)| {
                        let value = o.to_string();
                        Choice::new(
                            params::choice_label(o),
                            i == *selected,
                            cx.listener(move |this, _, _, cx| this.set_field(idx, FieldValue::Text(value.clone()), cx)),
                        )
                    })
                    .collect();
                div().w_full().child(widgets::dropdown(SharedString::from(format!("field-choice-{idx}")), choices, cx)).into_any_element()
            }
            _ => div().into_any_element(),
        }
    }

    fn pane(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let effects = self.presets.get(self.selected).map(|p| p.effects.clone()).unwrap_or_default();
        let count = effects.len();
        let mut stack = div().flex().flex_col().gap(px(2.));
        for (i, item) in effects.iter().enumerate() {
            stack = stack.child(self.effect_row(i, item, cx));
        }
        if effects.is_empty() {
            stack = stack.child(
                div()
                    .py(space::S6)
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(space::S1)
                    .rounded(radius::LG)
                    .border_1()
                    .border_dashed()
                    .border_color(t.foreground.opacity(0.14))
                    .child(widgets::icon(IconName::Sparkles, t.muted_foreground))
                    .child(div().pt(space::S1).text_size(text::ROW).font_weight(FontWeight::MEDIUM).child("No effects yet"))
                    .child(div().text_size(text::LABEL).text_color(t.muted_foreground).child("Add one to start building this preset.")),
            );
        }
        let can_delete = self.presets.len() > 1;
        let title_row = div()
            .h(px(HEADER_H - 4.))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S2)
            .child(div().flex_1().min_w_0().child(
                Input::new(&self.name).appearance(false).text_size(px(22.)).font_family(text::HEADING_FONT).font_weight(FontWeight::MEDIUM),
            ))
            .when(can_delete, |d| {
                d.child(widgets::icon_button(
                    "preset-delete",
                    IconName::Trash,
                    "Delete this preset",
                    false,
                    cx.listener(|this, _, w, cx| {
                        if this.presets.len() > 1 {
                            this.presets.remove(this.selected);
                            let i = this.selected.saturating_sub(1);
                            this.select_preset(i, w, cx);
                        }
                    }),
                    cx,
                ))
            });
        let summary = match count {
            0 => "Empty preset".to_string(),
            1 => "1 effect".to_string(),
            n => format!("{n} effects, applied top to bottom"),
        };
        let section_head = div()
            .h(px(44.))
            .mt(space::S3)
            .flex()
            .items_center()
            .child(div().flex_1().text_size(text::CAPTION).font_weight(FontWeight::MEDIUM).text_color(t.muted_foreground).child("Effects"))
            .when(!self.adding, |d| {
                d.child(widgets::pill(
                    "add-effect",
                    "Add effect",
                    Pill::Default,
                    true,
                    cx.listener(|this, _, _, cx| {
                        this.adding = true;
                        cx.notify();
                    }),
                    cx,
                ))
            });
        let lower = if self.adding { self.library(cx).into_any_element() } else { self.inspector(cx).into_any_element() };
        div()
            .id("fx-pane")
            .w(px(PANE_W))
            .flex_none()
            .h_full()
            .overflow_y_scroll()
            .border_r_1()
            .border_color(t.border)
            .px(px(20.))
            .pt(px(4.))
            .pb(space::S6)
            .flex()
            .flex_col()
            .child(title_row)
            .child(div().pl(px(10.)).mt(px(-8.)).text_size(text::LABEL).text_color(t.muted_foreground).child(summary))
            .child(section_head)
            .child(stack)
            .child(div().my(space::S5).h(px(1.)).bg(t.border))
            .child(lower)
    }

    fn preview_pane(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let t = cx.theme().clone();
        let x = theme::extra(cx);
        let segs = vec![
            Segment {
                id: "pv-after".into(),
                label: "With effects".into(),
                selected: !self.show_original,
                on_click: Box::new(cx.listener(|this, _, _, cx| {
                    this.show_original = false;
                    cx.notify();
                })),
            },
            Segment {
                id: "pv-before".into(),
                label: "Original".into(),
                selected: self.show_original,
                on_click: Box::new(cx.listener(|this, _, _, cx| {
                    this.show_original = true;
                    cx.notify();
                })),
            },
        ];
        let picture = if self.show_original { self.original.clone() } else { self.preview.clone().or_else(|| self.original.clone()) };
        let caption = format!("{} · {} × {}", self.sample_name, self.sample.width(), self.sample.height());
        let mut stage =
            div().flex_1().min_h_0().flex().flex_col().items_center().justify_center().gap(space::S3).px(space::S8).pb(space::S8);
        if let Some(p) = picture {
            stage = stage.child(img(p).max_w_full().max_h(relative(0.86)).rounded(radius::MD).shadow(vec![gpui::BoxShadow {
                color: gpui::black().opacity(if t.is_dark() { 0.5 } else { 0.18 }),
                offset: gpui::point(px(0.), px(12.)),
                blur_radius: px(36.),
                spread_radius: px(0.),
                inset: false,
            }]));
        }
        let stage = stage.child(div().text_size(text::CAPTION).text_color(t.muted_foreground).child(caption));
        div()
            .flex_1()
            .min_w(px(PREVIEW_MIN_W))
            .h_full()
            .flex()
            .flex_col()
            .bg(x.stage)
            .child(
                // The two controls wrap onto two lines when the pane is narrow.
                div()
                    .min_h(px(HEADER_H))
                    .flex_none()
                    .px(space::S5)
                    .py(space::S2)
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(space::S3)
                    .child(widgets::segmented(segs, cx))
                    .child(div().flex_1())
                    .child(widgets::pill(
                        "open-sample",
                        "Preview on an image…",
                        Pill::Default,
                        true,
                        cx.listener(|this, _, _, cx| this.open_sample(cx)),
                        cx,
                    )),
            )
            .child(stage)
    }
}

impl Render for EffectsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = cx.theme().clone();
        let sidebar = self.sidebar(cx);
        let pane = self.pane(cx);
        let preview = self.preview_pane(cx);

        let status = match (&self.message, self.dirty()) {
            (Some((m, ok)), _) => widgets::notice(m.clone(), *ok, cx).flex_1().min_w_0().truncate(),
            (None, true) => div()
                .flex_1()
                .flex()
                .items_center()
                .gap(space::S2)
                .text_size(text::LABEL)
                .text_color(t.muted_foreground)
                .child(div().size(px(7.)).rounded(radius::PILL).bg(t.warning))
                .child("Unsaved changes"),
            (None, false) => div().flex_1(),
        };
        let footer = div()
            .h(px(60.))
            .flex_none()
            .flex()
            .items_center()
            .gap(space::S2)
            .px(space::S5)
            .border_t_1()
            .border_color(t.border)
            .child(status)
            .child(widgets::pill("close", "Close", Pill::Ghost, false, |_, w, _| w.remove_window(), cx))
            .child(widgets::pill("save", "Save presets", Pill::Primary, false, cx.listener(|this, _, _, cx| this.save(cx)), cx));

        crate::chrome::rounded(widgets::page(cx), window).bg(t.sidebar).child(crate::chrome::TitleBar::new().title("Image effects")).child(
            div().flex_1().min_h_0().flex().child(sidebar).child(
                crate::chrome::sheet(window, cx)
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .child(div().flex_1().min_h_0().flex().child(pane).child(preview))
                    .child(footer),
            ),
        )
    }
}

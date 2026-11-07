//! The image effects window: presets, effects with their parameters (edited as JSON), and a live
//! preview on a sample image. Saving writes `Settings.effect_presets` through `save_settings`.

use std::path::PathBuf;
use std::sync::Arc;

use futures_channel::oneshot;
use gpui::{
    App, AppContext as _, Context, Entity, IntoElement, ParentElement as _, Render, RenderImage, SharedString, Styled as _,
    TitlebarOptions, Window, WindowBounds, WindowOptions, div, img, px, size,
};
use gpui_component::button::{Button, ButtonVariants as _};
use gpui_component::checkbox::Checkbox;
use gpui_component::input::{Input, InputEvent, InputState};
use gpui_component::scroll::ScrollableElement as _;
use gpui_component::{ActiveTheme as _, Root, Selectable as _, Sizable as _};
use laqta_domain::Bitmap;
use laqta_domain::effects::{Adjust, Draw, Effect, EffectItem, Filter, Manip, Preset};

use crate::runtime::UiState;
use crate::widgets;

pub(crate) fn open(path: Option<PathBuf>, cx: &mut App) {
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::centered(size(px(1100.), px(760.)), cx)),
        titlebar: Some(TitlebarOptions { title: Some("Image effects".into()), ..Default::default() }),
        app_id: Some("laqta".into()),
        ..Default::default()
    };
    if let Err(e) = cx.open_window(options, |window, cx| {
        let view = cx.new(|cx| EffectsWindow::new(path, window, cx));
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

struct EffectsWindow {
    presets: Vec<Preset>,
    selected: usize,
    selected_effect: Option<usize>,
    category: Category,
    params: Entity<InputState>,
    name: Entity<InputState>,
    sample: Bitmap,
    preview: Option<Arc<RenderImage>>,
    message: Option<String>,
}

/// A checkerboard-and-gradient sample, used until the user opens an image.
fn sample_image() -> Bitmap {
    let (w, h) = (320u32, 200u32);
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let check = ((x / 20 + y / 20) % 2) as u8;
            px[i] = (x * 255 / w) as u8;
            px[i + 1] = (y * 255 / h) as u8;
            px[i + 2] = 160 + check * 60;
            px[i + 3] = 255;
        }
    }
    Bitmap::new(w, h, laqta_domain::PixelFormat::Rgba8, px.into()).unwrap_or_else(|_| unreachable!("sample size"))
}

fn effect_json(e: &Effect) -> String {
    serde_json::to_string_pretty(e).unwrap_or_default()
}

impl EffectsWindow {
    fn new(path: Option<PathBuf>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let services = cx.global::<UiState>().services.clone();
        let mut presets = services.app.settings().effect_presets;
        if presets.is_empty() {
            presets.push(Preset { name: "Preset 1".into(), effects: Vec::new() });
        }
        let sample = path.and_then(|p| std::fs::read(p).ok()).and_then(|b| services.codecs.decode(&b).ok()).unwrap_or_else(sample_image);
        let params = cx.new(|cx| InputState::new(window, cx).placeholder("Select an effect to edit its parameters"));
        cx.subscribe(&params, |this: &mut Self, _, ev: &InputEvent, cx| {
            if matches!(ev, InputEvent::Change) {
                this.apply_params(cx);
            }
        })
        .detach();
        let first = presets[0].name.clone();
        let name = cx.new(|cx| InputState::new(window, cx).default_value(first));
        let mut this = EffectsWindow {
            presets,
            selected: 0,
            selected_effect: None,
            category: Category::Adjust,
            params,
            name,
            sample,
            preview: None,
            message: None,
        };
        this.render_preview(cx);
        this
    }

    fn preset(&mut self) -> &mut Preset {
        let i = self.selected.min(self.presets.len().saturating_sub(1));
        &mut self.presets[i]
    }

    fn apply_params(&mut self, cx: &mut Context<Self>) {
        let Some(i) = self.selected_effect else { return };
        let text = self.params.read(cx).value().to_string();
        match serde_json::from_str::<Effect>(&text) {
            Ok(e) => {
                if let Some(item) = self.preset().effects.get_mut(i) {
                    item.effect = e;
                }
                self.message = None;
                self.render_preview(cx);
            }
            Err(e) => self.message = Some(format!("Parameters: {e}")),
        }
        cx.notify();
    }

    fn select_effect(&mut self, i: usize, window: &mut Window, cx: &mut Context<Self>) {
        self.selected_effect = Some(i);
        let json = self.preset().effects.get(i).map(|it| effect_json(&it.effect)).unwrap_or_default();
        self.params.update(cx, |input, cx| input.set_value(json, window, cx));
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
                    Ok(Ok(img)) => {
                        this.preview = crate::toast::render_image(&img);
                        this.message = None;
                    }
                    Ok(Err(e)) => this.message = Some(format!("Preview: {e}")),
                    Err(_) => {}
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn save(&mut self, cx: &mut Context<Self>) {
        let name = self.name.read(cx).value().trim().to_string();
        if !name.is_empty() {
            self.preset().name = name;
        }
        let services = cx.global::<UiState>().services.clone();
        let mut settings = services.app.settings();
        settings.effect_presets = self.presets.clone();
        self.message = Some(match (services.save_settings)(settings) {
            Ok(()) => "Presets saved.".into(),
            Err(e) => format!("Couldn't save: {e}"),
        });
        cx.notify();
    }

    fn defaults(&self) -> Vec<Effect> {
        match self.category {
            Category::Adjust => Adjust::defaults().into_iter().map(Effect::Adjust).collect(),
            Category::Draw => Draw::defaults().into_iter().map(Effect::Draw).collect(),
            Category::Filter => Filter::defaults().into_iter().map(Effect::Filter).collect(),
            Category::Manip => Manip::defaults().into_iter().map(Effect::Manip).collect(),
        }
    }
}

impl Render for EffectsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Presets column.
        let mut presets = div().w(px(200.)).flex().flex_col().gap(px(4.)).p(px(10.)).border_r_1().border_color(cx.theme().border);
        for (i, p) in self.presets.iter().enumerate() {
            presets = presets.child(
                Button::new(SharedString::from(format!("preset-{i}"))).label(p.name.clone()).ghost().selected(i == self.selected).on_click(
                    cx.listener(move |this, _, window, cx| {
                        this.selected = i;
                        this.selected_effect = None;
                        let n = this.presets[i].name.clone();
                        this.name.update(cx, |input, cx| input.set_value(n, window, cx));
                        this.render_preview(cx);
                        cx.notify();
                    }),
                ),
            );
        }
        presets = presets.child(Button::new("preset-add").label("New preset").small().on_click(cx.listener(|this, _, _, cx| {
            let n = this.presets.len() + 1;
            this.presets.push(Preset { name: format!("Preset {n}"), effects: Vec::new() });
            this.selected = this.presets.len() - 1;
            this.render_preview(cx);
            cx.notify();
        })));
        if self.presets.len() > 1 {
            presets =
                presets.child(Button::new("preset-del").label("Delete preset").small().ghost().on_click(cx.listener(|this, _, _, cx| {
                    if this.presets.len() > 1 {
                        this.presets.remove(this.selected);
                        this.selected = this.selected.saturating_sub(1);
                        this.render_preview(cx);
                    }
                    cx.notify();
                })));
        }

        // The selected preset's effects.
        let effects = self.presets.get(self.selected).map(|p| p.effects.clone()).unwrap_or_default();
        let mut list = div().flex().flex_col().gap(px(4.));
        for (i, item) in effects.iter().enumerate() {
            let enabled = item.enabled;
            list = list.child(
                div()
                    .flex()
                    .flex_row()
                    .items_center()
                    .gap(px(4.))
                    .child(Checkbox::new(SharedString::from(format!("fx-on-{i}"))).checked(enabled).on_click(cx.listener(
                        move |this, v: &bool, _, cx| {
                            if let Some(it) = this.preset().effects.get_mut(i) {
                                it.enabled = *v;
                            }
                            this.render_preview(cx);
                            cx.notify();
                        },
                    )))
                    .child(
                        Button::new(SharedString::from(format!("fx-{i}")))
                            .label(item.effect.name())
                            .small()
                            .ghost()
                            .selected(self.selected_effect == Some(i))
                            .on_click(cx.listener(move |this, _, window, cx| this.select_effect(i, window, cx))),
                    )
                    .child(div().flex_1())
                    .child(Button::new(SharedString::from(format!("fx-up-{i}"))).label("↑").small().ghost().on_click(cx.listener(
                        move |this, _, _, cx| {
                            if i > 0 {
                                this.preset().effects.swap(i, i - 1);
                                this.render_preview(cx);
                            }
                            cx.notify();
                        },
                    )))
                    .child(Button::new(SharedString::from(format!("fx-rm-{i}"))).label("✕").small().ghost().on_click(cx.listener(
                        move |this, _, _, cx| {
                            if i < this.preset().effects.len() {
                                this.preset().effects.remove(i);
                                this.selected_effect = None;
                                this.render_preview(cx);
                            }
                            cx.notify();
                        },
                    ))),
            );
        }

        // Add an effect.
        let mut cats = div().flex().flex_row().gap(px(4.));
        for (c, label) in [
            (Category::Adjust, "Adjustments"),
            (Category::Draw, "Drawings"),
            (Category::Filter, "Filters"),
            (Category::Manip, "Manipulations"),
        ] {
            cats = cats.child(
                Button::new(SharedString::from(format!("cat-{label}"))).label(label).small().selected(self.category == c).on_click(
                    cx.listener(move |this, _, _, cx| {
                        this.category = c;
                        cx.notify();
                    }),
                ),
            );
        }
        let mut add = div().flex().flex_row().flex_wrap().gap(px(4.));
        for (i, e) in self.defaults().into_iter().enumerate() {
            let name = e.name();
            add = add.child(Button::new(SharedString::from(format!("add-{i}"))).label(name).small().ghost().on_click(cx.listener(
                move |this, _, window, cx| {
                    this.preset().effects.push(EffectItem { effect: e.clone(), enabled: true });
                    let n = this.preset().effects.len() - 1;
                    this.select_effect(n, window, cx);
                    this.render_preview(cx);
                },
            )));
        }

        let middle = div()
            .w(px(380.))
            .flex()
            .flex_col()
            .gap(px(8.))
            .p(px(10.))
            .border_r_1()
            .border_color(cx.theme().border)
            .child(widgets::row("Preset name", Input::new(&self.name), cx))
            .child(div().flex_1().overflow_y_scrollbar().child(list))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Add an effect"))
            .child(cats)
            .child(div().max_h(px(160.)).overflow_y_scrollbar().child(add))
            .child(div().font_weight(gpui::FontWeight::SEMIBOLD).child("Parameters"))
            .child(div().h(px(150.)).child(Input::new(&self.params).h_full()));

        let mut preview = div().flex_1().flex().flex_col().items_center().justify_center().p(px(10.)).bg(cx.theme().secondary);
        if let Some(p) = &self.preview {
            preview = preview.child(img(p.clone()).max_w_full().max_h_full());
        }

        let mut footer = div().flex().flex_row().items_center().gap(px(8.)).p(px(10.)).border_t_1().border_color(cx.theme().border);
        if let Some(m) = &self.message {
            footer = footer.child(widgets::muted(m.clone(), cx));
        }
        footer = footer
            .child(div().flex_1())
            .child(Button::new("close").label("Close").on_click(|_, w, _| w.remove_window()))
            .child(Button::new("save").label("Save presets").primary().on_click(cx.listener(|this, _, _, cx| this.save(cx))));

        widgets::page(cx)
            .child(div().flex_1().flex().flex_row().overflow_hidden().child(presets).child(middle).child(preview))
            .child(footer)
    }
}

//! Settings → AI features: one section per feature, each with its engine, a status and its model
//! choice. Choices apply on the click (like the theme); a model that isn't on disk is downloaded
//! first and only then put in use, so the feature keeps working meanwhile. Typed text (addresses,
//! keys) still waits for Save, as everywhere else in the window.

use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{AnyElement, AppContext as _, Context, IntoElement, ParentElement as _, SharedString, Styled as _, div};
use gpui_component::input::Input;
use laqta_domain::Settings;
use laqta_domain::settings::{BackgroundProvider, OcrEngine};

use super::SettingsWindow;
use crate::presenters::ai_features::{self as present, Download, Need, Status};
use crate::presenters::history::format_size;
use crate::runtime::{ModelProgress, ModelRow, OcrPresetRow, UiState};
use crate::theme::space;
use crate::widgets::{self, Pill, Slot, Tone};

/// A feature whose models download together.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum Card {
    Ocr,
    Background,
    Inpaint,
}

/// A settings change a download puts in use when it finishes.
type Apply = Rc<dyn Fn(&mut Settings)>;

struct Job {
    download: Download,
    cancel: Arc<AtomicBool>,
    /// What it downloads and what it puts in use after, so Try again repeats the same pick.
    ids: Vec<String>,
    apply: Option<Apply>,
}

/// The page's state that isn't a setting: what the model service reported, and running downloads.
#[derive(Default)]
pub(super) struct AiState {
    rows: Vec<ModelRow>,
    presets: Vec<OcrPresetRow>,
    runtime_ready: bool,
    runtime_size: u64,
    disk: u64,
    /// The OS text recognition engine's name, where there is one.
    system_ocr: Option<String>,
    jobs: HashMap<Card, Job>,
    advanced: bool,
    custom_family: usize,
    /// The analyzer service's model list, or why it couldn't be read.
    service_models: Option<Result<Vec<String>, String>>,
    testing: bool,
    /// Bumped by each test and each change of service, so a slow older answer is dropped.
    test_seq: u64,
}

impl Drop for AiState {
    /// A download outlives nothing: closing the window stops it.
    fn drop(&mut self) {
        for job in self.jobs.values() {
            job.cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl SettingsWindow {
    /// Reloads what the model service knows, on a background thread (it reads the disk).
    pub(super) fn refresh_models(&mut self, cx: &mut Context<Self>) {
        let models = cx.global::<UiState>().services.models.clone();
        let task = cx.background_spawn(async move {
            (models.rows(), models.ocr_presets(), models.runtime_ready(), models.runtime_size(), models.disk_usage(), models.system_ocr())
        });
        cx.spawn(async move |this, cx| {
            let (rows, presets, runtime_ready, runtime_size, disk, system_ocr) = task.await;
            let _ = this.update(cx, |this, cx| {
                let ai = &mut this.ai;
                (ai.rows, ai.presets, ai.runtime_ready, ai.runtime_size, ai.disk, ai.system_ocr) =
                    (rows, presets, runtime_ready, runtime_size, disk, system_ocr);
                cx.notify();
            });
        })
        .detach();
    }

    /// Changes the draft and the saved settings at once.
    pub(super) fn apply_now(&mut self, change: impl Fn(&mut Settings), cx: &mut Context<Self>) {
        change(&mut self.draft);
        let services = cx.global::<UiState>().services.clone();
        let mut saved = services.app.settings();
        change(&mut saved);
        if let Err(e) = (services.save_settings)(saved) {
            self.message = Some((false, format!("Couldn't save the choice: {e}")));
        }
        cx.notify();
    }

    fn row(&self, id: &str) -> Option<&ModelRow> {
        self.ai.rows.iter().find(|r| r.id == id)
    }

    fn rows_for(&self, task: &str) -> Vec<&ModelRow> {
        self.ai.rows.iter().filter(|r| r.task == task).collect()
    }

    /// The model in use for `task`, given the setting's choice.
    fn in_use(&self, task: &str, chosen: Option<&str>) -> Option<String> {
        let ids: Vec<(&str, bool)> = self.rows_for(task).iter().map(|r| (r.id.as_str(), r.default)).collect();
        present::in_use(chosen, &ids).map(str::to_owned)
    }

    fn need(&self, id: &str) -> Need {
        self.row(id).map_or(Need { installed: false, available: false, size_bytes: 0, custom: false }, |r| Need {
            installed: r.installed,
            available: r.available,
            size_bytes: r.size_bytes,
            custom: r.custom,
        })
    }

    fn ready(&self, ids: &[String]) -> bool {
        self.ai.runtime_ready && ids.iter().all(|id| self.need(id).installed)
    }

    fn status(&self, card: Card, ids: &[String]) -> Status {
        let needs: Vec<Need> = ids.iter().map(|id| self.need(id)).collect();
        let download = self.ai.jobs.get(&card).map(|j| j.download.clone()).unwrap_or_default();
        present::status(&needs, self.ai.runtime_ready, self.ai.runtime_size, &download)
    }

    /// Downloads `ids` (and ONNX Runtime) for `card`, then puts `apply` in use. A newer download
    /// for the same card cancels this one.
    fn fetch(&mut self, card: Card, ids: Vec<String>, apply: Option<Apply>, cx: &mut Context<Self>) {
        // ONNX Runtime is shared: two downloads at once would both fetch it into the same place.
        let other = self.ai.jobs.iter().any(|(c, j)| *c != card && matches!(j.download, Download::Running { .. }));
        if other && !self.ai.runtime_ready {
            self.message = Some((false, "Wait for the other download to finish first: it also fetches ONNX Runtime.".into()));
            cx.notify();
            return;
        }
        if let Some(old) = self.ai.jobs.get(&card) {
            old.cancel.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.ai.jobs.insert(
            card,
            Job { download: Download::Running { done: 0, total: None }, cancel: cancel.clone(), ids: ids.clone(), apply: apply.clone() },
        );
        cx.notify();
        let models = cx.global::<UiState>().services.models.clone();
        let (tx, mut rx) = futures_channel::mpsc::unbounded::<(u64, Option<u64>)>();
        let progress: ModelProgress = Arc::new(move |a, b| {
            let _ = tx.unbounded_send((a, b));
        });
        let stop = cancel.clone();
        let work = cx.background_spawn(async move {
            for id in ids {
                models.install(id, progress.clone(), stop.clone()).await?;
            }
            Ok::<(), String>(())
        });
        let mine = cancel.clone();
        cx.spawn(async move |this, cx| {
            use futures_lite::StreamExt as _;
            while let Some((done, total)) = rx.next().await {
                let alive = this.update(cx, |this, cx| {
                    if let Some(job) = this.ai.jobs.get_mut(&card).filter(|j| Arc::ptr_eq(&j.cancel, &mine)) {
                        job.download = Download::Running { done, total };
                        cx.notify();
                    }
                });
                if alive.is_err() {
                    break;
                }
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let result = work.await;
            let _ = this.update(cx, |this, cx| {
                let current = this.ai.jobs.get(&card).is_some_and(|j| Arc::ptr_eq(&j.cancel, &cancel));
                if current {
                    match result {
                        // Cancelled too late to stop the download: keep the files, not the choice.
                        Ok(()) if cancel.load(Ordering::Relaxed) => {
                            this.ai.jobs.remove(&card);
                        }
                        Ok(()) => {
                            this.ai.jobs.remove(&card);
                            if let Some(apply) = apply {
                                this.apply_now(move |s| apply(s), cx);
                            }
                        }
                        Err(_) if cancel.load(Ordering::Relaxed) => {
                            this.ai.jobs.remove(&card);
                        }
                        Err(e) => {
                            if let Some(job) = this.ai.jobs.get_mut(&card) {
                                job.download = Download::Failed(e);
                            }
                        }
                    }
                }
                this.refresh_models(cx);
                cx.notify();
            });
        })
        .detach();
    }

    /// Puts `ids` in use with `apply`, downloading first when something is missing.
    fn pick(&mut self, card: Card, ids: Vec<String>, apply: Apply, cx: &mut Context<Self>) {
        if self.ready(&ids) {
            if let Some(job) = self.ai.jobs.remove(&card) {
                job.cancel.store(true, Ordering::Relaxed);
            }
            self.apply_now(move |s| apply(s), cx);
        } else {
            self.fetch(card, ids, Some(apply), cx);
        }
    }

    /// The status line at the top of a feature: its badge, and the action that moves it on.
    fn status_row(&self, card: Card, status: Status, ids: Vec<String>, cx: &mut Context<Self>) -> gpui::Div {
        let (label, tone) = present::badge(&status);
        let tone = match tone {
            present::Tone::Good => Tone::Success,
            present::Tone::Neutral => Tone::Neutral,
            present::Tone::Busy => Tone::Busy,
            present::Tone::Bad => Tone::Danger,
        };
        let id = |what: &str| SharedString::from(format!("{card:?}-{what}"));
        let action: Option<AnyElement> = match &status {
            Status::Missing { bytes } => Some(
                widgets::pill(
                    id("download"),
                    format!("Download {}", format_size(*bytes)),
                    Pill::Primary,
                    true,
                    cx.listener(move |this, _, _, cx| this.fetch(card, ids.clone(), None, cx)),
                    cx,
                )
                .into_any_element(),
            ),
            Status::Downloading(_) => Some(
                widgets::pill(
                    id("cancel"),
                    "Cancel",
                    Pill::Default,
                    true,
                    cx.listener(move |this, _, _, cx| {
                        if let Some(job) = this.ai.jobs.get(&card) {
                            job.cancel.store(true, Ordering::Relaxed);
                        }
                        cx.notify();
                    }),
                    cx,
                )
                .into_any_element(),
            ),
            Status::Failed(_) => Some(
                widgets::pill(
                    id("retry"),
                    "Try again",
                    Pill::Default,
                    true,
                    // The failed job's own pick, not the models in use.
                    cx.listener(move |this, _, _, cx| {
                        let (ids, apply) =
                            this.ai.jobs.get(&card).map_or_else(|| (ids.clone(), None), |j| (j.ids.clone(), j.apply.clone()));
                        this.fetch(card, ids, apply, cx);
                    }),
                    cx,
                )
                .into_any_element(),
            ),
            _ => None,
        };
        let detail = match &status {
            Status::Failed(e) => Some(e.clone()),
            Status::FileMissing => Some("Remove it, or add the file again under Advanced.".into()),
            Status::Missing { .. } if !self.ai.runtime_ready => Some("Includes ONNX Runtime, which every local model uses.".into()),
            _ => None,
        };
        let control = div().flex().items_center().gap(space::S2).child(widgets::badge(label, tone, cx)).children(action);
        widgets::row_with("Status", detail, Slot::Hug, control, cx)
    }

    /// "Fast · 4.6 MB · Apache-2.0", with a warning for weights that forbid commercial use.
    fn model_detail(row: &ModelRow) -> String {
        let mut parts = Vec::new();
        let speed = present::speed_label(&row.speed);
        if !speed.is_empty() {
            parts.push(speed.to_owned());
        }
        if row.size_bytes > 0 {
            parts.push(format_size(row.size_bytes));
        }
        if !row.licence.is_empty() {
            parts.push(row.licence.clone());
        }
        if !row.commercial {
            parts.push("not for commercial use".into());
        }
        if !row.languages.is_empty() {
            parts.push(present::languages(&row.languages));
        }
        if row.custom {
            parts.push("your own model".into());
        }
        if row.custom && !row.installed {
            parts.push("file missing".into());
        }
        parts.join(" · ")
    }

    /// One model to choose: picking it puts it in use (downloading first). Downloaded models get a
    /// Remove button.
    fn model_option(&self, card: Card, row: &ModelRow, selected: bool, apply: Apply, cx: &mut Context<Self>) -> AnyElement {
        let id = row.id.clone();
        // A pending catalog entry or a custom model whose file is gone has nothing to download.
        let usable = row.installed || (row.available && !row.custom);
        let trailing = (row.installed || row.custom).then(|| self.remove_button(row, selected, cx).into_any_element());
        let detail = if row.available || row.installed {
            Self::model_detail(row)
        } else {
            format!("{} · not available yet", Self::model_detail(row))
        };
        widgets::option_row(
            SharedString::from(format!("model-{id}")),
            row.name.clone(),
            detail,
            selected,
            trailing,
            usable.then(|| -> widgets::OnClick {
                Box::new(cx.listener(move |this, _, _, cx| this.pick(card, vec![id.clone()], apply.clone(), cx)))
            }),
            cx,
        )
        .into_any_element()
    }

    fn remove_button(&self, row: &ModelRow, in_use: bool, cx: &mut Context<Self>) -> gpui::Stateful<gpui::Div> {
        let (id, name, custom) = (row.id.clone(), row.name.clone(), row.custom);
        widgets::pill(
            SharedString::from(format!("rm-{id}")),
            "Remove",
            Pill::Ghost,
            true,
            cx.listener(move |_, _, window, cx| {
                let question = present::remove_question(&name, in_use);
                let answer = window.prompt(gpui::PromptLevel::Warning, &question, None, &["Remove", "Cancel"], cx);
                let id = id.clone();
                cx.spawn_in(window, async move |this, cx| {
                    if answer.await != Ok(0) {
                        return;
                    }
                    let _ = this.update(cx, |this, cx| {
                        let services = cx.global::<UiState>().services.clone();
                        if let Err(e) = services.models.remove(&id) {
                            this.message = Some((false, format!("Couldn't remove the model: {e}")));
                            this.refresh_models(cx);
                            return;
                        }
                        if custom {
                            let gone = id.clone();
                            this.apply_now(
                                move |s| {
                                    s.models.custom.retain(|c| c.id != gone);
                                    for slot in [&mut s.models.ocr_det, &mut s.models.ocr_rec, &mut s.models.background] {
                                        if slot.as_deref() == Some(gone.as_str()) {
                                            *slot = None;
                                        }
                                    }
                                },
                                cx,
                            );
                        }
                        this.refresh_models(cx);
                    });
                })
                .detach();
            }),
            cx,
        )
    }

    pub(super) fn models_page(&self, cx: &mut Context<Self>) -> gpui::Div {
        super::page()
            .child(self.ocr_section(cx))
            .child(self.background_section(cx))
            .child(self.inpaint_section(cx))
            .child(self.describe_section(cx))
            .child(self.storage_section(cx))
    }

    fn ocr_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let m = &self.draft.models;
        let mut section = widgets::section("Text recognition (OCR)", cx);
        let os_name = self.ai.system_ocr.clone();
        let system = os_name.is_some() && m.ocr_engine == OcrEngine::System;
        if let Some(os_name) = &os_name {
            let engines = [(OcrEngine::Models, "Laqta models".to_owned()), (OcrEngine::System, os_name.clone())]
                .into_iter()
                .map(|(e, label)| {
                    widgets::Choice::new(
                        label,
                        m.ocr_engine == e,
                        cx.listener(move |this, _, _, cx| this.apply_now(move |s| s.models.ocr_engine = e, cx)),
                    )
                })
                .collect();
            section = section.child(widgets::row_with(
                "Engine",
                Some(format!("{os_name} needs no download. Laqta's models read small and mixed text better.")),
                Slot::Hug,
                widgets::choice("ocr-engine", engines, cx),
                cx,
            ));
        }
        if system {
            return section.child(self.status_row(
                Card::Ocr,
                Status::Elsewhere(format!("Using {}", os_name.unwrap_or_default())),
                Vec::new(),
                cx,
            ));
        }
        let (Some(det), Some(rec)) = (self.in_use("ocr-det", m.ocr_det.as_deref()), self.in_use("ocr-rec", m.ocr_rec.as_deref())) else {
            return section.child(widgets::muted("No text models in this build.", cx));
        };
        let ids = vec![det.clone(), rec.clone()];
        section = section.child(self.status_row(Card::Ocr, self.status(Card::Ocr, &ids), ids, cx));
        let pairs: Vec<(String, String, String)> = self.ai.presets.iter().map(|p| (p.id.clone(), p.det.clone(), p.rec.clone())).collect();
        let active = present::active_preset(&pairs, &det, &rec).map(str::to_owned);
        for p in &self.ai.presets {
            let size: u64 = [&p.det, &p.rec].iter().map(|id| self.need(id).size_bytes).sum();
            let on_disk = self.need(&p.det).installed && self.need(&p.rec).installed;
            let detail = format!(
                "{} {}",
                p.description,
                if on_disk { "Downloaded.".to_owned() } else { format!("{} download.", format_size(size)) }
            );
            let (d, r) = (p.det.clone(), p.rec.clone());
            section = section.child(widgets::option_row(
                SharedString::from(format!("preset-{}", p.id)),
                p.name.clone(),
                detail,
                active.as_deref() == Some(p.id.as_str()),
                None,
                Some(Box::new(cx.listener(move |this, _, _, cx| {
                    let (d, r) = (d.clone(), r.clone());
                    let ids = vec![d.clone(), r.clone()];
                    this.pick(
                        Card::Ocr,
                        ids,
                        Rc::new(move |s: &mut Settings| {
                            s.models.ocr_det = Some(d.clone());
                            s.models.ocr_rec = Some(r.clone());
                        }),
                        cx,
                    );
                }))),
                cx,
            ));
        }
        if active.is_none() {
            let name = |id: &str| self.row(id).map_or(id.to_owned(), |r| r.name.clone());
            section = section.child(widgets::option_row(
                "preset-own",
                "Your own pair",
                format!("{} with {}. Change it under Advanced.", name(&det), name(&rec)),
                true,
                None,
                None,
                cx,
            ));
        }
        section
            .child(div().pt(space::S2).child(widgets::pill(
                "ocr-advanced",
                if self.ai.advanced { "Hide advanced" } else { "Advanced: pick each model, add your own" },
                Pill::Ghost,
                true,
                cx.listener(|this, _, _, cx| {
                    this.ai.advanced = !this.ai.advanced;
                    cx.notify();
                }),
                cx,
            )))
            .children(self.ai.advanced.then(|| self.advanced_models(&det, &rec, cx)))
    }

    fn advanced_models(&self, det: &str, rec: &str, cx: &mut Context<Self>) -> gpui::Div {
        let mut list = div().flex().flex_col().gap(space::S6).pt(space::S4);
        for (task, title, chosen) in [("ocr-det", "Text detection: finds the lines", det), ("ocr-rec", "Text recognition: reads them", rec)]
        {
            let mut group = div().flex().flex_col().gap(gpui::px(2.)).child(widgets::muted(title, cx).pb(space::S1));
            for row in self.rows_for(task) {
                let id = row.id.clone();
                let apply: Apply = Rc::new(move |s: &mut Settings| {
                    let slot = if task == "ocr-det" { &mut s.models.ocr_det } else { &mut s.models.ocr_rec };
                    *slot = Some(id.clone());
                });
                group = group.child(self.model_option(Card::Ocr, row, row.id == chosen, apply, cx));
            }
            list = list.child(group);
        }
        list.child(self.custom_model_form(cx))
    }

    fn custom_model_form(&self, cx: &mut Context<Self>) -> gpui::Div {
        let families = cx.global::<UiState>().services.models.families();
        let fam = families
            .iter()
            .enumerate()
            .map(|(i, (task, family))| {
                widgets::Choice::new(
                    present::family_label(task, family),
                    self.ai.custom_family == i,
                    cx.listener(move |this, _, _, cx| {
                        this.ai.custom_family = i;
                        cx.notify();
                    }),
                )
            })
            .collect();
        div()
            .flex()
            .flex_col()
            .child(widgets::muted("Your own model", cx).pb(space::S1))
            .child(
                widgets::muted("Add an ONNX file of a kind Laqta knows. Laqta checks its inputs and outputs before using it.", cx)
                    .pb(space::S2),
            )
            .child(widgets::row_with("Kind", None::<&str>, Slot::Field, widgets::choice("family", fam, cx), cx))
            .child(widgets::row_with("Name", None::<&str>, Slot::Field, Input::new(&self.custom_name), cx))
            .child(widgets::action_row(
                "Model file",
                Some("Pick the .onnx file. It's added and can be chosen right away."),
                widgets::pill(
                    "custom-add",
                    "Choose file…",
                    Pill::Default,
                    false,
                    cx.listener(|this, _, _, cx| this.add_custom_model(cx)),
                    cx,
                ),
                cx,
            ))
    }

    fn background_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let provider = self.draft.cloud.background;
        let engines = [(BackgroundProvider::Local, "On this computer"), (BackgroundProvider::RemoveBg, "remove.bg")]
            .into_iter()
            .map(|(b, label)| {
                widgets::Choice::new(
                    label,
                    provider == b,
                    cx.listener(move |this, _, _, cx| this.apply_now(move |s| s.cloud.background = b, cx)),
                )
            })
            .collect();
        let mut section = widgets::section("Remove background", cx).child(widgets::row_with(
            "Engine",
            Some(if provider == BackgroundProvider::RemoveBg {
                "remove.bg is an online service: each image is sent to it."
            } else {
                "Runs on this computer. Nothing leaves it."
            }),
            Slot::Hug,
            widgets::choice("bg-engine", engines, cx),
            cx,
        ));
        if provider == BackgroundProvider::RemoveBg {
            return section.child(self.status_row(Card::Background, Status::Elsewhere("Uses remove.bg".into()), Vec::new(), cx)).child(
                widgets::row_with(
                    "remove.bg API key",
                    Some("From remove.bg → Dashboard → API keys. Kept when you press Save."),
                    Slot::Field,
                    Input::new(&self.cloud[4]),
                    cx,
                ),
            );
        }
        let chosen = self.draft.models.background.clone();
        let Some(current) = self.in_use("background", chosen.as_deref()) else {
            return section.child(widgets::muted("No background models in this build.", cx));
        };
        let ids = vec![current.clone()];
        section = section.child(self.status_row(Card::Background, self.status(Card::Background, &ids), ids, cx));
        for row in self.rows_for("background") {
            let id = row.id.clone();
            section = section.child(self.model_option(
                Card::Background,
                row,
                row.id == current,
                Rc::new(move |s: &mut Settings| s.models.background = Some(id.clone())),
                cx,
            ));
        }
        section
    }

    fn inpaint_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let mut section = widgets::section("Erase objects (photo editor)", cx);
        let Some(current) = self.in_use("inpaint", None) else {
            return section.child(widgets::muted("No object removal model in this build.", cx));
        };
        let ids = vec![current.clone()];
        section = section.child(self.status_row(Card::Inpaint, self.status(Card::Inpaint, &ids), ids, cx));
        if let Some(row) = self.row(&current) {
            let control = (row.installed).then(|| self.remove_button(row, true, cx));
            section =
                section.child(widgets::row_with(row.name.clone(), Some(Self::model_detail(row)), Slot::Hug, div().children(control), cx));
        }
        section
    }

    fn describe_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let url = Self::value(&self.cloud[0], cx);
        let preset = present::service_for(&url);
        let mut services: Vec<widgets::Choice> = present::SERVICES
            .iter()
            .enumerate()
            .map(|(i, s)| {
                let s = *s;
                widgets::Choice::new(
                    s.name,
                    preset == Some(i),
                    cx.listener(move |this, _, window, cx| {
                        this.cloud[0].update(cx, |input, cx| input.set_value(s.base_url, window, cx));
                        if !s.model.is_empty() {
                            this.cloud[1].update(cx, |input, cx| input.set_value(s.model, window, cx));
                        }
                        this.ai.service_models = None;
                        this.ai.testing = false;
                        this.ai.test_seq += 1;
                        cx.notify();
                    }),
                )
            })
            .collect();
        services.push(widgets::Choice::new(
            "Other",
            preset.is_none(),
            cx.listener(|this, _, window, cx| {
                this.cloud[0].update(cx, |input, cx| input.set_value("", window, cx));
                this.ai.service_models = None;
                this.ai.testing = false;
                this.ai.test_seq += 1;
                cx.notify();
            }),
        ));
        let needs_key = preset.is_none_or(|i| present::SERVICES[i].needs_key);
        let model = Self::value(&self.cloud[1], cx);
        let test = widgets::pill(
            "analyzer-test",
            if self.ai.testing { "Checking…" } else { "Test connection" },
            Pill::Default,
            true,
            cx.listener(|this, _, _, cx| this.test_service(cx)),
            cx,
        );
        let (result, suggestions): (Option<(bool, String)>, Vec<String>) = match &self.ai.service_models {
            None => (None, Vec::new()),
            Some(Err(e)) => (Some((false, e.clone())), Vec::new()),
            Some(Ok(list)) => {
                let known = model.is_empty() || list.contains(&model);
                let note = match (list.len(), known) {
                    (0, _) => "Connected. The service lists no models.".to_owned(),
                    (n, true) => format!("Connected. {n} models available."),
                    (n, false) => format!("Connected, but it has no model named \"{model}\" ({n} others)."),
                };
                // An unknown name: offer the closest ones the service has.
                let q = model.to_lowercase();
                let picks =
                    if known { Vec::new() } else { list.iter().filter(|m| m.to_lowercase().contains(&q)).take(6).cloned().collect() };
                (Some((known, note)), picks)
            }
        };
        let mut picks = div().flex().flex_wrap().gap(space::S1);
        for (i, name) in suggestions.into_iter().enumerate() {
            let value = name.clone();
            picks = picks.child(widgets::pill(
                SharedString::from(format!("analyzer-model-{i}")),
                name,
                Pill::Ghost,
                true,
                cx.listener(move |this, _, window, cx| {
                    this.cloud[1].update(cx, |input, cx| input.set_value(value.clone(), window, cx));
                    cx.notify();
                }),
                cx,
            ));
        }
        let mut section = widgets::section("Describe image", cx)
            .child(
                widgets::muted(
                    "Sends the capture to an AI service you choose and shows its answer. Addresses and keys are kept when you press Save.",
                    cx,
                )
                .pb(space::S2),
            )
            .child(widgets::row_with("Service", None::<&str>, Slot::Field, widgets::choice("analyzer-service", services, cx), cx))
            .child(self.input_row(
                "cloud-0-reset",
                "Address",
                Some("The base URL, such as http://localhost:11434/v1."),
                Slot::Field,
                &self.cloud[0],
                |d| &d.cloud[0],
                cx,
            ))
            .child(self.input_row("cloud-1-reset", "Model", None::<&str>, Slot::Field, &self.cloud[1], |d| &d.cloud[1], cx));
        if needs_key {
            section = section.child(widgets::row_with(
                "API key",
                Some("Stored in the system keyring."),
                Slot::Field,
                Input::new(&self.cloud[3]),
                cx,
            ));
        }
        section = section
            .child(self.input_row("cloud-2-reset", "Prompt", None::<&str>, Slot::Field, &self.cloud[2], |d| &d.cloud[2], cx))
            .child(widgets::row_with("Connection", result.as_ref().map(|(_, note)| note.clone()), Slot::Hug, test, cx));
        section.child(picks)
    }

    fn test_service(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let url = Self::value(&self.cloud[0], cx);
        let typed = Self::value(&self.cloud[3], cx);
        let key = (!typed.is_empty()).then_some(typed);
        self.ai.testing = true;
        self.ai.service_models = None;
        self.ai.test_seq += 1;
        let seq = self.ai.test_seq;
        cx.notify();
        let task = cx.background_spawn((services.analyzer_models)(url, key));
        cx.spawn(async move |this, cx| {
            let result = task.await;
            let _ = this.update(cx, |this, cx| {
                // A newer test or another service since: this answer is for an old address.
                if this.ai.test_seq != seq {
                    return;
                }
                this.ai.testing = false;
                this.ai.service_models = Some(result);
                cx.notify();
            });
        })
        .detach();
    }

    fn storage_section(&self, cx: &mut Context<Self>) -> gpui::Div {
        let open = widgets::pill(
            "models-open",
            "Open folder",
            Pill::Default,
            true,
            cx.listener(|this, _, _, cx| {
                let services = cx.global::<UiState>().services.clone();
                let dir = services.models.models_dir();
                if let Err(e) = services.shell.open_path(&dir) {
                    this.message = Some((false, format!("Couldn't open the models folder: {e}")));
                    cx.notify();
                }
            }),
            cx,
        );
        widgets::section("On this computer", cx)
            .child(widgets::row_with(
                "Downloaded models",
                Some(format!("{} in the models folder.", format_size(self.ai.disk))),
                Slot::Hug,
                open,
                cx,
            ))
            .child(widgets::row_with(
                "Runs on",
                Some("The processor. Graphics card support isn't in this build yet."),
                Slot::Hug,
                div(),
                cx,
            ))
    }

    pub(super) fn add_custom_model(&mut self, cx: &mut Context<Self>) {
        let services = cx.global::<UiState>().services.clone();
        let families = services.models.families();
        let Some((_, family)) = families.get(self.ai.custom_family).cloned() else { return };
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
                        let name = model.name.clone();
                        this.apply_now(
                            move |s| {
                                s.models.custom.retain(|c| c.id != model.id);
                                s.models.custom.push(model.clone());
                            },
                            cx,
                        );
                        this.report(Ok(format!("Added {name}. Pick it above to use it.")), cx);
                    }
                    Ok(None) => {}
                    Err(e) => this.report(Err(format!("Laqta can't use this model: {e}")), cx),
                }
                this.refresh_models(cx);
            });
        })
        .detach();
    }
}

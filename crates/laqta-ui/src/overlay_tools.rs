//! The capture toolbar and in-place annotation on the region overlay.
//!
//! The editor engine runs here on the selection's pixels, the same way the editor window hosts it
//! for `EditorHost`, so the overlay and the editor share one tool set. The Region session still
//! owns the selection and the outcome: this module only locks the selection while there are
//! annotations, and sends the edited crop (`RegionInput::Annotated`) before `Finish(action)`.

use std::sync::Arc;

use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, RenderImage,
    StatefulInteractiveElement as _, Styled as _, div, img, prelude::FluentBuilder as _, px,
};
use gpui_component::tooltip::Tooltip;
use laqta_domain::Bitmap;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{RegionCommand, RegionInput, SelectionAction};
use laqta_editor::{Chrome, Command, Editor, EditorInput, Tool};

use crate::assets::IconName;
use crate::overlay::OverlayModel;
use crate::presenters::capture_toolbar::{self as bar, Item};
use crate::presenters::editor_interaction::EditorInteraction;
use crate::presenters::render_gate::ComputeFn;
use crate::runtime::UiState;
use crate::theme::radius;

/// The editor engine over the current selection.
pub(crate) struct Annot {
    /// The selection the engine's canvas covers, in desktop pixels.
    pub rect: PhysRect,
    /// The crop it was built from, so a new crop for the same selection is not rebuilt.
    base: Arc<Bitmap>,
    interaction: EditorInteraction,
    compute: ComputeFn,
    pub picture: Option<Arc<RenderImage>>,
    pub chrome: Chrome,
}

impl Annot {
    /// The user has drawn something (or can undo to nothing).
    pub fn has_marks(&self) -> bool {
        self.chrome.annotation_count > 0 || self.chrome.can_undo
    }
}

/// Button size and spacing on the toolbar.
const BUTTON: f32 = 32.0;
const PAD: f32 = 6.0;
const DIVIDER: f32 = 13.0;
pub(crate) const BAR_H: f32 = BUTTON + 2.0 * PAD;

/// The toolbar's width for its groups.
pub(crate) fn bar_width() -> f32 {
    let groups = bar::groups();
    let buttons: usize = groups.iter().map(Vec::len).sum();
    buttons as f32 * (BUTTON + 2.0) + (groups.len() - 1) as f32 * DIVIDER + 2.0 * PAD
}

fn rgba(rgb: u32, alpha: f32) -> Hsla {
    let mut c: Hsla = gpui::rgb(rgb).into();
    c.a = alpha;
    c
}

/// The accent used for the selection, the active tool and Done.
pub(crate) const ACCENT: u32 = 0x3d8bfd;

fn icon(item: Item) -> IconName {
    match item {
        Item::Region => IconName::SquareDashedMousePointer,
        Item::Tool(Tool::Rectangle) => IconName::Square,
        Item::Tool(Tool::Ellipse) => IconName::Circle,
        Item::Tool(Tool::Arrow) => IconName::MoveUpRight,
        Item::Tool(Tool::Freehand) => IconName::PenLine,
        Item::Tool(Tool::Highlight) => IconName::Highlighter,
        Item::Tool(Tool::Text) => IconName::Type,
        Item::Tool(Tool::Step) => IconName::ListOrdered,
        Item::Tool(Tool::Blur) => IconName::Droplet,
        Item::Tool(Tool::Pixelate) => IconName::Grid3x3,
        Item::Tool(_) => IconName::Square,
        Item::Undo => IconName::Undo2,
        Item::Redo => IconName::Redo2,
        Item::CopyText => IconName::ScanText,
        Item::Pin => IconName::Pin,
        Item::Edit => IconName::SquarePen,
        Item::Upload => IconName::CloudUpload,
        Item::Save => IconName::Save,
        Item::Copy => IconName::Copy,
        Item::Cancel => IconName::X,
        Item::Confirm => IconName::Check,
    }
}

impl OverlayModel {
    /// Builds, keeps or drops the engine after a new view: a new selection crop starts a fresh
    /// canvas unless the selection is locked (annotations exist).
    pub(crate) fn sync_annot(&mut self, cx: &mut Context<Self>) {
        let view = &self.view;
        // The app sends a crop once per new selection; later views carry `None` and the engine
        // stays as it is.
        match (view.selected, view.selection_image.clone()) {
            (Some(_), None) => {}
            (Some(rect), Some(base)) => {
                let same = self.annot.as_ref().is_some_and(|a| Arc::ptr_eq(&a.base, &base) || view.locked);
                if !same {
                    let services = cx.global::<UiState>().services.clone();
                    let editor = Editor::new((*base).clone(), services.fonts.clone());
                    let chrome = editor.chrome();
                    let compute = crate::editor_window::compute_fn(services.spawner.clone());
                    let interaction = EditorInteraction::new(editor, compute.clone());
                    self.annot = Some(Annot { rect, base, interaction, compute, picture: None, chrome });
                    if let Some(tool) = self.tool {
                        self.command(Command::SetTool(tool), cx);
                    }
                    // Text mode: the selection is the area to read, so finish with it right away.
                    if self.text_mode {
                        self.finish(SelectionAction::CopyText, cx);
                    }
                }
            }
            _ if !view.locked => {
                self.annot = None;
                self.tool = None;
            }
            _ => {}
        }
    }

    /// The toolbar shows for a settled selection while the user hasn't hidden it (Space).
    pub(crate) fn toolbar_shown(&self) -> bool {
        self.view.selected.is_some() && self.view.tools_visible && self.annot.is_some()
    }

    fn send(&self, input: RegionInput) {
        (self.on_input)(input);
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.annot.as_mut() else { return };
        let render = a.interaction.render();
        let compute = a.compute.clone();
        cx.spawn(async move |this, cx| {
            let picture = match render.await {
                Some(b) => crate::editor_window::compute_picture(&compute, b).await,
                None => None,
            };
            let _ = this.update(cx, |m, cx| {
                let mut again = false;
                if let Some(a) = m.annot.as_mut() {
                    if picture.is_some() {
                        a.picture = picture;
                    }
                    again = a.interaction.render_finished();
                }
                if again {
                    m.start_render(cx);
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn apply(&mut self, step: crate::presenters::editor_interaction::Step, cx: &mut Context<Self>) {
        if let Some(a) = self.annot.as_mut() {
            a.chrome = step.chrome;
        }
        if step.render_now {
            self.start_render(cx);
        }
        cx.notify();
    }

    /// One pointer or key event for the engine, in canvas (selection) pixels.
    pub(crate) fn editor_input(&mut self, input: EditorInput, cx: &mut Context<Self>) {
        let Some(a) = self.annot.as_mut() else { return };
        let step = a.interaction.input(input);
        self.apply(step, cx);
    }

    /// Engine commands here are state math (tool, undo); rendering stays on the pool.
    fn command(&mut self, command: Command, cx: &mut Context<Self>) {
        let Some(a) = self.annot.as_mut() else { return };
        let out = a.interaction.editor().lock().unwrap_or_else(|p| p.into_inner()).execute(command);
        let step = a.interaction.after_output(out);
        self.apply(step, cx);
    }

    /// A toolbar button or its shortcut.
    pub(crate) fn pick(&mut self, item: Item, cx: &mut Context<Self>) {
        match item {
            Item::Region => self.region_tool(),
            Item::Tool(tool) => {
                if self.annot.is_none() {
                    return;
                }
                self.tool = Some(tool);
                self.command(Command::SetTool(tool), cx);
                // While a drawing tool is out, drags draw instead of moving the selection.
                self.send(RegionInput::Command(RegionCommand::Lock));
            }
            Item::Undo => self.command(Command::Undo, cx),
            Item::Redo => self.command(Command::Redo, cx),
            Item::CopyText => self.finish(SelectionAction::CopyText, cx),
            Item::Pin => self.finish(SelectionAction::Pin, cx),
            Item::Edit => self.finish(SelectionAction::Edit, cx),
            Item::Upload => self.finish(SelectionAction::Upload, cx),
            Item::Save => self.finish(SelectionAction::Save, cx),
            Item::Copy => self.finish(SelectionAction::Copy, cx),
            Item::Cancel => self.send(RegionInput::Command(RegionCommand::Cancel)),
            Item::Confirm => self.finish(SelectionAction::Default, cx),
        }
        cx.notify();
    }

    /// Back to adjusting the selection. It stays locked while annotations exist, so they keep
    /// lining up with the pixels under them.
    pub(crate) fn region_tool(&mut self) {
        self.tool = None;
        if !self.annot.as_ref().is_some_and(Annot::has_marks) {
            self.send(RegionInput::Command(RegionCommand::Unlock));
        }
    }

    /// Ends the session with `action`: with annotations, the edited crop goes first.
    pub(crate) fn finish(&mut self, action: SelectionAction, cx: &mut Context<Self>) {
        let finish = RegionInput::Command(RegionCommand::Finish(action));
        let Some(a) = self.annot.as_ref().filter(|a| a.has_marks()) else {
            self.send(finish);
            return;
        };
        let editor = a.interaction.editor();
        let export = crate::presenters::render_gate::compute(&a.compute, move || editor.lock().unwrap_or_else(|p| p.into_inner()).export());
        let on_input = self.on_input.clone();
        cx.spawn(async move |_, _| {
            // Without the edited crop the capture would silently lose the annotations, so the
            // overlay stays open and the user can try again or press Esc.
            match export.await {
                Some(image) => {
                    on_input(RegionInput::Annotated(image));
                    on_input(finish);
                }
                None => tracing::error!("capture toolbar: exporting the annotations failed"),
            }
        })
        .detach();
    }
}

/// The annotation canvas and its chrome (selected annotation, text caret) over the selection.
/// `local` maps a desktop rect into this window; `s` is the window's scale.
pub(crate) fn canvas(a: &Annot, local: impl Fn(PhysRect) -> bar::Rect, s: f32) -> Option<AnyElement> {
    let picture = a.picture.clone()?;
    let r = local(a.rect);
    let mut el = div().absolute().left(px(r.x)).top(px(r.y)).w(px(r.w)).h(px(r.h)).child(img(picture).size_full());
    if let Some(sel) = a.chrome.selection {
        el = el.child(
            div()
                .absolute()
                .left(px(sel.x / s))
                .top(px(sel.y / s))
                .w(px(sel.w / s))
                .h(px(sel.h / s))
                .border_1()
                .border_color(rgba(ACCENT, 1.0)),
        );
    }
    if let Some(c) = a.chrome.caret {
        let h = (a.chrome.style.font.size / s).max(12.0);
        el = el.child(div().absolute().left(px(c.x / s)).top(px(c.y / s)).w(px(2.)).h(px(h)).bg(rgba(ACCENT, 1.0)));
    }
    Some(el.into_any_element())
}

/// The floating toolbar at `at`, built from the presenter's groups.
pub(crate) fn toolbar(model: &Entity<OverlayModel>, at: bar::Rect, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let chrome = m.annot.as_ref().map(|a| a.chrome.clone());
    let mut row = div()
        .id("capture-toolbar")
        .absolute()
        .left(px(at.x))
        .top(px(at.y))
        .h(px(BAR_H))
        .px(px(PAD))
        .flex()
        .items_center()
        .gap(px(2.))
        .rounded(radius::LG)
        .bg(rgba(0x1c1c1f, 0.97))
        .border_1()
        .border_color(rgba(0xffffff, 0.10))
        .shadow_lg()
        .cursor_default()
        // Clicks on the bar never reach the selection or the canvas under it.
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (g, group) in bar::groups().into_iter().enumerate() {
        if g > 0 {
            row = row.child(div().mx(px(5.)).w(px(1.)).h(px(20.)).bg(rgba(0xffffff, 0.14)));
        }
        for item in group {
            let active = match item {
                Item::Region => m.tool.is_none(),
                Item::Tool(t) => m.tool == Some(t),
                _ => false,
            };
            let enabled = match item {
                Item::Undo => chrome.as_ref().is_some_and(|c| c.can_undo),
                Item::Redo => chrome.as_ref().is_some_and(|c| c.can_redo),
                _ => true,
            };
            row = row.child(button(model.clone(), item, active, enabled));
        }
    }
    row.into_any_element()
}

fn button(model: Entity<OverlayModel>, item: Item, active: bool, enabled: bool) -> AnyElement {
    let (bg, fg, hover) = match item {
        Item::Confirm => (rgba(ACCENT, 1.0), rgba(0xffffff, 1.0), rgba(ACCENT, 0.85)),
        _ if active => (rgba(ACCENT, 0.9), rgba(0xffffff, 1.0), rgba(ACCENT, 0.8)),
        Item::Cancel => (gpui::transparent_black(), rgba(0xff6b6b, 1.0), rgba(0xff6b6b, 0.16)),
        _ => (gpui::transparent_black(), rgba(0xffffff, 0.88), rgba(0xffffff, 0.12)),
    };
    let tip = match bar::shortcut(item) {
        Some(keys) => format!("{}  ·  {}", bar::label(item), crate::presenters::hotkeys::shortcut_text(keys, crate::host_os())),
        None => bar::label(item).to_owned(),
    };
    let id = match item {
        Item::Tool(t) => gpui::ElementId::Name(format!("tb-tool-{}", t.name()).into()),
        other => gpui::ElementId::Name(format!("tb-{other:?}").into()),
    };
    div()
        .id(id)
        .size(px(BUTTON))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(8.))
        .bg(bg)
        .when(enabled, |d| d.cursor_pointer().hover(move |s| s.bg(hover)))
        .when(!enabled, |d| d.opacity(0.35))
        .child(gpui_component::Icon::new(icon(item)).size(px(17.)).text_color(fg))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .when(enabled, |d| {
            d.on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.pick(item, cx));
            })
        })
        .into_any_element()
}

/// "1280 × 720" above (or below) the selection.
pub(crate) fn size_badge(rect: PhysRect, at: bar::Rect) -> AnyElement {
    div()
        .absolute()
        .left(px(at.x))
        .top(px(at.y))
        .h(px(at.h))
        .px(px(8.))
        .flex()
        .items_center()
        .rounded(radius::SM)
        .bg(rgba(0x1c1c1f, 0.92))
        .text_color(rgba(0xffffff, 0.92))
        .text_size(px(12.))
        .font_weight(FontWeight::MEDIUM)
        .child(format!("{} × {}", rect.width, rect.height))
        .into_any_element()
}

/// A short line at the top of the screen before anything is selected.
pub(crate) fn hint(win_w: f32, text: &'static str) -> AnyElement {
    div()
        .absolute()
        .top(px(16.))
        .left(px(0.))
        .w(px(win_w))
        .flex()
        .justify_center()
        .child(
            div()
                .px(px(14.))
                .py(px(7.))
                .rounded(radius::PILL)
                .bg(rgba(0x1c1c1f, 0.9))
                .text_color(rgba(0xffffff, 0.9))
                .text_size(px(12.5))
                .child(text),
        )
        .into_any_element()
}

/// A button on the mode bar at the top of the selection screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Screenshot,
    Video,
    Region,
    FullScreen,
    Monitor,
    EditAfter,
    ColorPicker,
    Text,
    Close,
}

impl Mode {
    fn icon(self) -> IconName {
        match self {
            Mode::Screenshot => IconName::Camera,
            Mode::Video => IconName::Video,
            Mode::Region => IconName::SquareDashed,
            Mode::FullScreen => IconName::Maximize,
            Mode::Monitor => IconName::Monitor,
            Mode::EditAfter => IconName::SquarePen,
            Mode::ColorPicker => IconName::Pipette,
            Mode::Text => IconName::ScanText,
            Mode::Close => IconName::X,
        }
    }

    fn tip(self) -> &'static str {
        match self {
            Mode::Screenshot => "Screenshot",
            Mode::Video => "Record video",
            Mode::Region => "Select an area or a window",
            Mode::FullScreen => "Capture every monitor",
            Mode::Monitor => "Capture this monitor",
            Mode::EditAfter => "Open in the editor after capturing",
            Mode::ColorPicker => "Pick a color from the screen",
            Mode::Text => "Copy the text in the area you select",
            Mode::Close => "Close  ·  Esc",
        }
    }
}

/// The mode bar's groups, left to right.
const MODES: [&[Mode]; 5] = [
    &[Mode::Screenshot, Mode::Video],
    &[Mode::Region, Mode::FullScreen, Mode::Monitor],
    &[Mode::EditAfter],
    &[Mode::ColorPicker, Mode::Text],
    &[Mode::Close],
];

/// The mode bar's width, for centring.
pub(crate) fn mode_bar_width() -> f32 {
    let buttons: usize = MODES.iter().map(|g| g.len()).sum();
    buttons as f32 * (BUTTON + 4.0) + (MODES.len() - 1) as f32 * DIVIDER + 2.0 * PAD
}

impl OverlayModel {
    /// A mode bar button.
    pub(crate) fn mode(&mut self, mode: Mode, cx: &mut Context<Self>) {
        match mode {
            Mode::Screenshot | Mode::Region => {
                self.text_mode = false;
            }
            Mode::FullScreen => self.send(RegionInput::Command(RegionCommand::Fullscreen)),
            Mode::Monitor => self.send(RegionInput::Command(RegionCommand::ActiveMonitor)),
            Mode::EditAfter => {
                if self.view.selected.is_some() {
                    self.finish(SelectionAction::Edit, cx);
                } else {
                    self.send(RegionInput::Command(RegionCommand::Annotate));
                }
            }
            Mode::Text => {
                if self.view.selected.is_some() {
                    self.finish(SelectionAction::CopyText, cx);
                } else {
                    self.text_mode = !self.text_mode;
                }
            }
            Mode::Video => self.switch_to(laqta_domain::Action::ScreenRecorder, cx),
            Mode::ColorPicker => self.switch_to(laqta_domain::Action::ScreenColorPicker, cx),
            Mode::Close => self.send(RegionInput::Command(RegionCommand::Cancel)),
        }
        cx.notify();
    }

    /// Ends this selection and starts `action` once the overlay is gone, so the new capture
    /// doesn't see it.
    fn switch_to(&mut self, action: laqta_domain::Action, cx: &mut Context<Self>) {
        self.send(RegionInput::Command(RegionCommand::Cancel));
        let dispatch = cx.global::<UiState>().services.dispatch.clone();
        cx.spawn(async move |_, cx| {
            cx.background_executor().timer(std::time::Duration::from_millis(200)).await;
            dispatch(action);
        })
        .detach();
    }

    fn mode_active(&self, mode: Mode) -> bool {
        match mode {
            Mode::Screenshot => true,
            Mode::Region => !self.text_mode,
            Mode::EditAfter => self.view.annotate,
            Mode::Text => self.text_mode,
            _ => false,
        }
    }
}

/// The mode bar, centred at the top of a `win_w`-wide window.
pub(crate) fn mode_bar(model: &Entity<OverlayModel>, win_w: f32, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let mut row = div()
        .id("mode-bar")
        .absolute()
        .top(px(16.))
        .left(px((win_w - mode_bar_width()) / 2.0))
        .h(px(BAR_H))
        .px(px(PAD))
        .flex()
        .items_center()
        .gap(px(4.))
        .rounded(radius::LG)
        .bg(rgba(0x1c1c1f, 0.97))
        .border_1()
        .border_color(rgba(0xffffff, 0.10))
        .shadow_lg()
        .cursor_default()
        .on_any_mouse_down(|_, _, cx| cx.stop_propagation())
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (g, group) in MODES.iter().enumerate() {
        if g > 0 {
            row = row.child(div().mx(px(4.)).w(px(1.)).h(px(20.)).bg(rgba(0xffffff, 0.14)));
        }
        for &mode in *group {
            let active = m.mode_active(mode);
            let (bg, fg, hover) = match mode {
                _ if active => (rgba(0xffffff, 0.16), rgba(0xffffff, 1.0), rgba(0xffffff, 0.22)),
                _ => (gpui::transparent_black(), rgba(0xffffff, 0.85), rgba(0xffffff, 0.12)),
            };
            let entity = model.clone();
            let tip = mode.tip();
            row = row.child(
                div()
                    .id(gpui::ElementId::Name(format!("mode-{mode:?}").into()))
                    .size(px(BUTTON))
                    .flex_none()
                    .relative()
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(8.))
                    .bg(bg)
                    .cursor_pointer()
                    .hover(move |s| s.bg(hover))
                    .child(gpui_component::Icon::new(mode.icon()).size(px(17.)).text_color(fg))
                    // The active mode carries an underline, so the state never rests on colour alone.
                    .when(active, |d| {
                        d.child(
                            div()
                                .absolute()
                                .bottom(px(3.))
                                .left(px(10.))
                                .w(px(BUTTON - 20.))
                                .h(px(2.))
                                .rounded(radius::PILL)
                                .bg(rgba(ACCENT, 1.0)),
                        )
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_click(move |_, _, cx| entity.update(cx, |m, cx| m.mode(mode, cx))),
            );
        }
    }
    row.into_any_element()
}

/// The line under the mode bar before anything is selected.
pub(crate) fn mode_hint(win_w: f32) -> AnyElement {
    div()
        .absolute()
        .top(px(16.0 + BAR_H + 8.0))
        .left(px(0.))
        .w(px(win_w))
        .flex()
        .justify_center()
        .child(
            div()
                .px(px(12.))
                .py(px(5.))
                .rounded(radius::PILL)
                .bg(rgba(0x1c1c1f, 0.85))
                .text_color(rgba(0xffffff, 0.85))
                .text_size(px(12.))
                .child("Drag to select  ·  Click a window  ·  Space hides the tools  ·  Esc closes"),
        )
        .into_any_element()
}

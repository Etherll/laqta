//! The capture toolbar and in-place annotation on the region overlay.
//!
//! The editor engine runs here on the selection's pixels, the same way the editor window hosts it
//! for `EditorHost`, so the overlay and the editor share one tool set. The Region session still
//! owns the selection and the outcome: this module keeps the marks over the same desktop pixels when
//! the selection moves (`Command::Reframe`, ADR 032), and sends the edited crop
//! (`RegionInput::Annotated`) before `Finish(action)`.

use std::sync::Arc;

use gpui::accesskit::{Action as A11yAction, Role, Toggled};
use gpui::{
    AnyElement, App, Context, Entity, FontWeight, Hsla, InteractiveElement as _, IntoElement, MouseButton, ParentElement as _, RenderImage,
    SharedString, StatefulInteractiveElement as _, Styled as _, div, img, prelude::FluentBuilder as _, px,
};
use gpui_component::tooltip::Tooltip;
use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use laqta_domain::geometry::PhysRect;
use laqta_domain::region::{RegionCommand, RegionInput, SelectionAction};
use laqta_editor::{Chrome, Command, Editor, EditorInput, Style, Tool};

use crate::assets::IconName;
use crate::overlay::OverlayModel;
use crate::presenters::capture_escape::{self, EscapeScene, EscapeStep};
use crate::presenters::capture_pointer::PointerScene;
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
    /// The selection `picture` was rendered for. It trails `rect` for a frame or two after a reframe,
    /// so the old picture stays where it was drawn until the new one lands.
    pub picture_rect: PhysRect,
    pub chrome: Chrome,
    /// The box of the mark the Pointer hovers, outlined so the next click's target shows.
    pub hover: Option<laqta_editor::RectF>,
}

impl Annot {
    /// The user has drawn something (or can undo to nothing).
    pub fn has_marks(&self) -> bool {
        self.chrome.annotation_count > 0 || self.chrome.can_undo
    }

    /// Where each mark sits on the canvas, for tests.
    #[cfg(test)]
    pub(crate) fn mark_rects(&self) -> Vec<laqta_editor::RectF> {
        let editor = self.interaction.editor();
        let editor = editor.lock().unwrap_or_else(|p| p.into_inner());
        editor.annotations().iter().filter_map(|a| a.kind.rect()).collect()
    }
}

/// Button size and spacing on the toolbar.
const BUTTON: f32 = 32.0;
const PAD: f32 = 6.0;
const DIVIDER: f32 = 13.0;
pub(crate) const BAR_H: f32 = BUTTON + 2.0 * PAD;

/// How far `item`'s button centre is from the toolbar's left edge.
fn button_centre(item: Item) -> f32 {
    let mut x = PAD;
    for (g, group) in bar::groups().into_iter().enumerate() {
        if g > 0 {
            x += DIVIDER;
        }
        for i in group {
            if i == item {
                return x + BUTTON / 2.0;
            }
            x += BUTTON + 2.0;
        }
    }
    bar_width() / 2.0
}

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
        Item::Pointer => IconName::MousePointer2,
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
    /// Builds, moves or drops the engine after a new view. A new crop of the same selection session
    /// reframes the engine, so its marks stay over the same desktop pixels (ADR 032); the first crop
    /// builds it. The engine survives a gesture (a click outside, a draw that may come to nothing)
    /// and goes once no selection is held.
    pub(crate) fn sync_annot(&mut self, cx: &mut Context<Self>) {
        let view = &self.view;
        // The app sends a crop once per settled selection; later views carry `None` and the engine
        // stays as it is.
        match (view.selected, view.selection_image.clone()) {
            (Some(_), None) => {}
            (Some(rect), Some(base)) => {
                if let Some(a) = self.annot.as_ref() {
                    if Arc::ptr_eq(&a.base, &base) {
                        return;
                    }
                    // A move carries the marks along, unless Alt left them in place; a resize keeps
                    // them over the pixels they were drawn on.
                    let moved = (a.rect.width, a.rect.height) == (rect.width, rect.height);
                    let old = a.rect;
                    let frame_only = std::mem::take(&mut self.frame_only);
                    let carry = moved && !frame_only;
                    let (dx, dy) = if carry { (0.0, 0.0) } else { ((old.x - rect.x) as f32, (old.y - rect.y) as f32) };
                    let Some(a) = self.annot.as_mut() else { return };
                    a.rect = rect;
                    a.base = base.clone();
                    self.command(Command::Reframe { image: (*base).clone(), dx, dy }, cx);
                    return;
                }
                let services = cx.global::<UiState>().services.clone();
                let mut editor = Editor::new((*base).clone(), services.fonts.clone());
                // The canvas is in desktop pixels, so the reach is the grab size as drawn.
                editor.set_handle_reach(self.grab);
                let chrome = editor.chrome();
                let compute = crate::editor_window::compute_fn(services.spawner.clone());
                let interaction = EditorInteraction::new(editor, compute.clone());
                self.annot = Some(Annot { rect, base, interaction, compute, picture: None, picture_rect: rect, chrome, hover: None });
                // Under Move the engine idles on its select tool; a drawing tool comes with its style.
                match self.tool {
                    Some(tool) => self.use_tool(tool, cx),
                    None => self.command(Command::SetTool(Tool::Select), cx),
                }
                // Text mode: the selection is the area to read, so ask for its text right away.
                if self.text_mode {
                    self.read_text();
                }
            }
            (None, _) if !view.gesture => {
                self.annot = None;
                self.tool = None;
            }
            (None, _) => {}
        }
    }

    /// What the pointer at the desktop point `at` is over, for `capture_pointer`. `wait` takes the
    /// engine's lock even while a render holds it: a press needs the true answer, a hover move can
    /// make do without the mark test.
    pub(crate) fn pointer_scene(&self, at: (i32, i32), wait: bool) -> PointerScene {
        // A handle of the picked mark under the pointer, which reaches past the mark's outline.
        let mark_grip = self.annot.as_ref().and_then(|a| {
            let (x, y) = ((at.0 - a.rect.x) as f32, (at.1 - a.rect.y) as f32);
            crate::presenters::mark_pointer::handle_under(&a.chrome.handles, (x, y), self.grab)
        });
        let over_mark = mark_grip.is_some()
            || self.annot.as_ref().is_some_and(|a| {
                let (x, y) = ((at.0 - a.rect.x) as f32, (at.1 - a.rect.y) as f32);
                let editor = a.interaction.editor();
                let hit = |e: &Editor| e.annotation_at(x, y);
                if wait { hit(&editor.lock().unwrap_or_else(|p| p.into_inner())) } else { editor.try_lock().is_ok_and(|e| hit(&e)) }
            });
        // A multi-region session holds no selection; its active region moves and resizes the same way.
        let active = self.view.active.and_then(|i| self.view.regions.get(i).copied());
        let over_picked = mark_grip.is_some()
            || self.annot.as_ref().is_some_and(|a| {
                let (x, y) = ((at.0 - a.rect.x) as f32, (at.1 - a.rect.y) as f32);
                a.chrome.selection.is_some_and(|r| x >= r.x && x <= r.x + r.w && y >= r.y && y <= r.y + r.h)
            });
        PointerScene {
            frame: self.view.selected.or(active.filter(|_| !self.view.gesture)),
            over_picked,
            grab: self.view.handle_at(at),
            resizing: self.view.resizing,
            gesture: self.view.gesture,
            tool: self.tool,
            over_mark,
            over_text: self.text_point(at.0, at.1).is_some(),
            marks: self.annot.as_ref().is_some_and(Annot::has_marks),
            mark_grip,
            frame_only: false,
        }
    }

    /// Follows the pointer at the desktop point `at` with the hover outline: only under the
    /// Pointer with no press under way. Returns whether the outline changed.
    pub(crate) fn hover(&mut self, at: (i32, i32)) -> bool {
        let pointer = self.tool.is_none() && self.pressed.is_none() && !self.view.gesture;
        let Some(a) = self.annot.as_mut() else { return false };
        let next = if pointer {
            let (x, y) = ((at.0 - a.rect.x) as f32, (at.1 - a.rect.y) as f32);
            // A hover move can't wait on a render; it keeps the old outline until the lock frees.
            match a.interaction.editor().try_lock() {
                Ok(e) => e.hover_at(x, y),
                Err(_) => return false,
            }
        } else {
            None
        };
        std::mem::replace(&mut a.hover, next) != next
    }

    /// The pointer is on a monitor of scale `scale`: the handle reach follows it, for the cursor
    /// and the engine alike.
    pub(crate) fn set_grab_scale(&mut self, scale: f32) {
        let grab = crate::presenters::mark_pointer::GRAB * scale.max(1.0);
        if grab == self.grab {
            return;
        }
        // The engine may be busy with a render; the reach is retried on the next move until it takes.
        if let Some(a) = self.annot.as_ref() {
            match a.interaction.editor().try_lock() {
                Ok(mut e) => e.set_handle_reach(grab),
                Err(_) => return,
            }
        }
        self.grab = grab;
    }

    /// Holds Space: the Pointer stands in for the drawing tool until Space comes up.
    pub(crate) fn borrow_pointer(&mut self) {
        // Not mid-press: switching tools under a drag would end it as the wrong tool and lose the
        // shape being drawn.
        if self.pressed.is_some() {
            return;
        }
        if self.space.space_down(self.tool).is_some() {
            self.region_tool();
        }
    }

    /// Space came up: the drawing tool comes back, once any drag under way has ended.
    pub(crate) fn return_pointer(&mut self, cx: &mut Context<Self>) {
        let back = self.space.space_up(self.pressed.is_some(), self.tool);
        self.give_back(back, cx);
    }

    /// The overlay lost activation with Space perhaps still down: the lent tool comes back.
    pub(crate) fn space_lost(&mut self, cx: &mut Context<Self>) {
        let back = self.space.space_up(false, self.tool);
        self.space.picked();
        self.give_back(back, cx);
    }

    /// A press ended: the drawing tool comes back if Space came up during it.
    pub(crate) fn after_release(&mut self, cx: &mut Context<Self>) {
        let back = self.space.released(self.tool);
        self.give_back(back, cx);
    }

    fn give_back(&mut self, back: Option<Option<Tool>>, cx: &mut Context<Self>) {
        if let Some(Some(tool)) = back
            && self.annot.is_some()
        {
            self.tool = Some(tool);
            self.use_tool(tool, cx);
            cx.notify();
        }
    }

    /// Lets the picked mark go: a press on empty space is about the selection.
    pub(crate) fn let_go_of_mark(&mut self, cx: &mut Context<Self>) {
        if self.mark_selected() {
            self.command(Command::Deselect, cx);
        }
    }

    /// Whether a mark is picked: the arrows and Delete are then the mark's, not the selection's.
    pub(crate) fn mark_selected(&self) -> bool {
        self.annot.as_ref().is_some_and(|a| a.chrome.selection.is_some())
    }

    /// The desktop point (`x`, `y`) in the engine's canvas pixels.
    pub(crate) fn canvas_point(&self, x: i32, y: i32) -> Option<(f32, f32)> {
        let a = self.annot.as_ref()?;
        Some(((x - a.rect.x) as f32, (y - a.rect.y) as f32))
    }

    /// The toolbar shows for a settled selection while the user hasn't hidden it (Space). The
    /// selection's text, read in place, has a bar of its own, and a text capture has no toolbar.
    pub(crate) fn toolbar_shown(&self) -> bool {
        self.view.selected.is_some() && self.view.tools_visible && self.annot.is_some() && self.text.is_none() && !self.view.text_only
    }

    fn send(&self, input: RegionInput) {
        (self.on_input)(input);
    }

    fn start_render(&mut self, cx: &mut Context<Self>) {
        let Some(a) = self.annot.as_mut() else { return };
        let render = a.interaction.render();
        let compute = a.compute.clone();
        let rect = a.rect;
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
                        a.picture_rect = rect;
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
            // A warning about discarding marks is about the marks it counted.
            if a.chrome.annotation_count != step.chrome.annotation_count {
                self.discard_armed = None;
            }
            // The marks changed under a still pointer: the outline may be of a box that moved.
            if a.chrome != step.chrome {
                a.hover = None;
            }
            a.chrome = step.chrome;
        }
        if step.render_now {
            self.start_render(cx);
        }
        cx.notify();
    }

    /// One pointer or key event for the engine, in canvas (selection) pixels.
    pub(crate) fn editor_input(&mut self, input: EditorInput, cx: &mut Context<Self>) {
        let released = matches!(input, EditorInput::PointerUp { .. });
        let Some(a) = self.annot.as_mut() else { return };
        let step = a.interaction.input(input);
        self.apply(step, cx);
        // The engine selects each new mark. Under a drawing tool a selected mark would only show a
        // stray box and take the next colour picked on the strip, so it lets go. The Pointer keeps
        // the mark it picked, and text being typed stays selected.
        let drawing = self.tool.is_some();
        if released && drawing && self.annot.as_ref().is_some_and(|a| a.chrome.selection.is_some() && !a.chrome.editing_text) {
            self.command(Command::Deselect, cx);
        }
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
        // The export under way is of the marks as they are.
        if self.finishing {
            return;
        }
        // A tool picked while Space is held is the one to keep.
        if matches!(item, Item::Pointer | Item::Tool(_)) {
            self.space.picked();
        }
        match item {
            Item::Pointer => self.region_tool(),
            Item::Tool(tool) => {
                if self.annot.is_none() {
                    return;
                }
                self.tool = Some(tool);
                self.use_tool(tool, cx);
            }
            Item::Undo => self.command(Command::Undo, cx),
            Item::Redo => self.command(Command::Redo, cx),
            Item::CopyText => self.read_text(),
            Item::Pin => self.finish(SelectionAction::Pin, cx),
            Item::Edit => self.finish(SelectionAction::Edit, cx),
            Item::Upload => self.finish(SelectionAction::Upload, cx),
            Item::Save => self.finish(SelectionAction::Save, cx),
            Item::Copy => self.finish(SelectionAction::Copy, cx),
            Item::Cancel => self.cancel(cx),
            Item::Confirm => self.finish(SelectionAction::Default, cx),
        }
        cx.notify();
    }

    /// Puts `tool` in the engine's hand with the colour and size last picked for it. The tool
    /// goes first: switching tools drops the engine's selection, so the style can't restyle an
    /// earlier mark.
    fn use_tool(&mut self, tool: Tool, cx: &mut Context<Self>) {
        if let Some(a) = self.annot.as_mut() {
            a.hover = None;
        }
        self.command(Command::SetTool(tool), cx);
        self.command(Command::SetStyle(bar::style_for(tool, &self.marks)), cx);
    }

    /// A colour or size from the style strip or a number key: it styles the next marks, and the
    /// text being typed.
    pub(crate) fn restyle(&mut self, change: impl FnOnce(&Style, Tool) -> Style, cx: &mut Context<Self>) {
        // The export under way is of the marks as they are.
        if self.finishing {
            return;
        }
        let (Some(tool), Some(a)) = (self.tool, self.annot.as_ref()) else { return };
        let style = change(&a.chrome.style, tool);
        bar::remember(&mut self.marks, tool, &style);
        self.marks_changed = true;
        self.command(Command::SetStyle(style), cx);
    }

    /// Back to the Pointer: the engine idles on `Select`, so a press on a mark picks it.
    pub(crate) fn region_tool(&mut self) {
        self.tool = None;
        if let Some(a) = self.annot.as_mut() {
            let out = a.interaction.editor().lock().unwrap_or_else(|p| p.into_inner()).execute(Command::SetTool(Tool::Select));
            a.chrome = a.interaction.after_output(out).chrome;
        }
    }

    fn escape_scene(&self) -> EscapeScene {
        let chrome = self.annot.as_ref().map(|a| &a.chrome);
        EscapeScene {
            typing: chrome.is_some_and(|c| c.editing_text),
            mark_selected: chrome.is_some_and(|c| c.selection.is_some()),
            tool_out: self.tool.is_some(),
            marks: chrome.map_or(0, |c| c.annotation_count),
            armed: self.discard_armed.is_some_and(|at| at.elapsed() < capture_escape::DISARM_AFTER),
        }
    }

    /// Esc: one layer back (typing, the picked mark, the drawing tool), then a warning before the
    /// marks are thrown away, then the capture ends.
    pub(crate) fn escape(&mut self, cx: &mut Context<Self>) {
        let step = capture_escape::escape(&self.escape_scene());
        self.escape_step(step, cx);
    }

    /// The toolbar's and the mode bar's close buttons end the capture at once: a click on ✕ is a
    /// choice, unlike Esc, which also steps back.
    pub(crate) fn cancel(&mut self, cx: &mut Context<Self>) {
        self.escape_step(EscapeStep::Cancel, cx);
    }

    fn escape_step(&mut self, step: EscapeStep, cx: &mut Context<Self>) {
        match step {
            EscapeStep::StopTyping | EscapeStep::Deselect => self.command(Command::Deselect, cx),
            EscapeStep::PutToolAway => self.region_tool(),
            EscapeStep::ArmDiscard => {
                let at = std::time::Instant::now();
                self.discard_armed = Some(at);
                cx.spawn(async move |this, cx| {
                    cx.background_executor().timer(capture_escape::DISARM_AFTER).await;
                    let _ = this.update(cx, |m, cx| {
                        if m.discard_armed == Some(at) {
                            m.discard_armed = None;
                            cx.notify();
                        }
                    });
                })
                .detach();
            }
            EscapeStep::Cancel => self.send(RegionInput::Command(RegionCommand::Cancel)),
        }
        cx.notify();
    }

    /// The warning Esc armed, while it holds.
    pub(crate) fn discard_notice(&self) -> Option<String> {
        let armed = self.discard_armed.is_some_and(|at| at.elapsed() < capture_escape::DISARM_AFTER);
        let marks = self.annot.as_ref().map_or(0, |a| a.chrome.annotation_count);
        (armed && marks > 0).then(|| capture_escape::discard_notice(marks))
    }

    /// Ends the session with `action`: with annotations, the edited crop goes first.
    /// Not while the selection is dragged, or before its new crop reached the engine: the edited
    /// crop would be of the old frame.
    pub(crate) fn finish(&mut self, action: SelectionAction, cx: &mut Context<Self>) {
        let stale = self.annot.as_ref().is_some_and(|a| Some(a.rect) != self.view.selected);
        // Nor mid-press on the canvas: the mark being drawn isn't in the export yet.
        let drawing = self.pressed == Some(crate::presenters::capture_pointer::Route::Editor);
        if self.view.gesture || stale || self.finishing || drawing {
            return;
        }
        let finish = RegionInput::Command(RegionCommand::Finish(action));
        let Some(a) = self.annot.as_ref().filter(|a| a.has_marks()) else {
            self.send(finish);
            return;
        };
        let editor = a.interaction.editor();
        let export = crate::presenters::render_gate::compute(&a.compute, move || editor.lock().unwrap_or_else(|p| p.into_inner()).export());
        let on_input = self.on_input.clone();
        self.finishing = true;
        cx.spawn(async move |this, cx| {
            // Without the edited crop the capture would silently lose the annotations, so the
            // overlay stays open and the user can try again or press Esc.
            match export.await {
                Some(image) => {
                    on_input(RegionInput::Annotated(image));
                    on_input(finish);
                }
                None => {
                    tracing::error!("capture toolbar: exporting the annotations failed");
                    let _ = this.update(cx, |m, cx| {
                        m.finishing = false;
                        cx.notify();
                    });
                }
            }
        })
        .detach();
    }
}

/// The annotation canvas and its chrome (picked mark and its handles, text caret), clipped to
/// `frame`, the selection in this window. The engine's canvas sits at `local(a.rect)`: the last
/// settled selection, which differs from `frame` while the selection is dragged.
pub(crate) fn canvas(a: &Annot, local: impl Fn(PhysRect) -> bar::Rect, frame: bar::Rect, s: f32) -> Option<AnyElement> {
    let picture = a.picture.clone()?;
    let at = |r: bar::Rect| div().absolute().left(px(r.x - frame.x)).top(px(r.y - frame.y)).w(px(r.w)).h(px(r.h));
    let shown = at(local(a.picture_rect)).child(img(picture).size_full());
    let mut el = at(local(a.rect));
    if let Some(sel) = a.chrome.selection {
        el = el.child(
            div()
                .absolute()
                .left(px(sel.x / s - 1.0))
                .top(px(sel.y / s - 1.0))
                .w(px(sel.w / s + 2.0))
                .h(px(sel.h / s + 2.0))
                .border_2()
                .border_color(rgba(ACCENT, 1.0)),
        );
    }
    if let Some(h) = a.hover {
        el = el.child(
            div()
                .absolute()
                .left(px(h.x / s - 2.0))
                .top(px(h.y / s - 2.0))
                .w(px(h.w / s + 4.0))
                .h(px(h.h / s + 4.0))
                .border_1()
                .border_color(rgba(ACCENT, 0.7)),
        );
    }
    for (_, p) in &a.chrome.handles {
        el = el.child(crate::mark_chrome::mark_handle(p.x / s, p.y / s, rgba(ACCENT, 1.0)));
    }
    if let Some(c) = a.chrome.caret {
        let h = (a.chrome.style.font.size / s).max(12.0);
        el = el.child(div().absolute().left(px(c.x / s)).top(px(c.y / s)).w(px(2.)).h(px(h)).bg(rgba(ACCENT, 1.0)));
    }
    let clip = div().absolute().left(px(frame.x)).top(px(frame.y)).w(px(frame.w)).h(px(frame.h)).overflow_hidden().child(shown).child(el);
    Some(clip.into_any_element())
}

/// The floating toolbar at `at`, built from the presenter's groups.
pub(crate) fn toolbar(model: &Entity<OverlayModel>, at: bar::Rect, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let chrome = m.annot.as_ref().map(|a| a.chrome.clone());
    let mut row = div()
        .id("capture-toolbar")
        .role(Role::Toolbar)
        .aria_label("Capture tools")
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
        .on_any_mouse_down(crate::overlay_tools::press_on_bar(model.clone()))
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (g, group) in bar::groups().into_iter().enumerate() {
        if g > 0 {
            row = row.child(div().mx(px(5.)).w(px(1.)).h(px(20.)).bg(rgba(0xffffff, 0.14)));
        }
        for item in group {
            let active = match item {
                Item::Pointer => m.tool.is_none(),
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
    let keys = bar::shortcut(item).map(|k| crate::presenters::hotkeys::shortcut_text(k, crate::host_os()));
    let tip = match &keys {
        Some(keys) => format!("{}  ·  {keys}", bar::label(item)),
        None => bar::label(item).to_owned(),
    };
    let is_tool = matches!(item, Item::Pointer | Item::Tool(_));
    let activate = model.clone();
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
        // Icon-only, so the name a screen reader announces comes from the tooltip's label.
        .role(Role::Button)
        .aria_label(bar::label(item))
        .when_some(keys, |d, keys| d.aria_keyshortcuts(keys))
        .when(is_tool, |d| d.aria_toggled(if active { Toggled::True } else { Toggled::False }))
        .when(!enabled, |d| d.aria_description("Not available yet"))
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .when(enabled, |d| {
            d.on_click(move |_, _, cx| {
                model.update(cx, |m, cx| m.pick(item, cx));
            })
            .on_a11y_action(A11yAction::Click, move |_, _, cx| activate.update(cx, |m, cx| m.pick(item, cx)))
        })
        .into_any_element()
}

/// The size badge's height.
pub(crate) const BADGE_H: f32 = 24.0;

/// The size badge's width for `text` ("1280 × 720", with the position while moving).
pub(crate) fn badge_width(text: &str) -> f32 {
    16.0 + 7.5 * text.chars().count() as f32
}

/// The mouse-down handler for a bar: the press stays on the bar, and its release is the bar's too
/// (the overlay catches releases before any bar sees them).
pub(crate) fn press_on_bar(model: Entity<OverlayModel>) -> impl Fn(&gpui::MouseDownEvent, &mut gpui::Window, &mut App) + 'static {
    move |ev, _, cx| {
        cx.stop_propagation();
        model.update(cx, |m, _| m.chrome_press = Some(ev.button));
    }
}

/// Swatch and size button size on the style strip.
const SWATCH: f32 = 28.0;
const STRIP_H: f32 = SWATCH + 2.0 * PAD;

/// The colours and sizes for the drawing tool in use, beside the toolbar. `None` without a
/// drawing tool, or for one with nothing to style.
pub(crate) fn style_strip(
    model: &Entity<OverlayModel>,
    bar_at: bar::Rect,
    side: bar::Side,
    screen: (f32, f32),
    cx: &App,
) -> Option<AnyElement> {
    let m = model.read(cx);
    let tool = m.tool.filter(|t| bar::has_style(*t))?;
    let style = &m.annot.as_ref()?.chrome.style;
    let colors = bar::palette(tool);
    let current = bar::color_of(style, tool);
    let size = bar::size_of(style, tool);
    let divider = if colors.is_empty() { 0.0 } else { DIVIDER };
    let width = (colors.len() + bar::SIZES.len()) as f32 * (SWATCH + 2.0) + divider + 2.0 * PAD;
    let at = bar::strip(bar_at, side, (width, STRIP_H), screen, bar_at.x + button_centre(Item::Tool(tool)));
    let mut row = div()
        .id("style-strip")
        .role(Role::Toolbar)
        .aria_label("Colour and size")
        .absolute()
        .left(px(at.x))
        .top(px(at.y))
        .h(px(STRIP_H))
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
        .on_any_mouse_down(crate::overlay_tools::press_on_bar(model.clone()))
        .on_mouse_up(MouseButton::Left, |_, _, cx| cx.stop_propagation());
    for (i, &c) in colors.iter().enumerate() {
        row = row.child(swatch(model.clone(), i, c, c == current));
    }
    if !colors.is_empty() {
        row = row.child(div().mx(px(5.)).w(px(1.)).h(px(18.)).bg(rgba(0xffffff, 0.14)));
    }
    // Size dots take the tool's colour, so the strip previews the next mark.
    let dot = if colors.is_empty() { rgba(0xffffff, 0.9) } else { hsla(current) };
    for (i, s) in bar::SIZES.into_iter().enumerate() {
        let entity = model.clone();
        let picked = size == Some(s);
        let d = 6.0 + 4.0 * i as f32;
        row = row.child(
            div()
                .id(gpui::ElementId::Name(format!("strip-size-{i}").into()))
                .size(px(SWATCH))
                .flex_none()
                .flex()
                .items_center()
                .justify_center()
                .rounded(px(7.))
                .cursor_pointer()
                .when(picked, |d| d.bg(rgba(0xffffff, 0.16)))
                .hover(|d| d.bg(rgba(0xffffff, 0.12)))
                .child(div().size(px(d)).rounded(radius::PILL).bg(dot).border_1().border_color(rgba(0x000000, 0.35)))
                .role(Role::RadioButton)
                .aria_label(s.label(tool))
                .aria_toggled(if picked { Toggled::True } else { Toggled::False })
                .tooltip(move |window, cx| Tooltip::new(s.label(tool)).build(window, cx))
                .on_a11y_action(A11yAction::Click, {
                    let entity = entity.clone();
                    move |_, _, cx| entity.update(cx, |m, cx| m.restyle(|st, t| bar::with_size(st, t, s), cx))
                })
                .on_click(move |_, _, cx| entity.update(cx, |m, cx| m.restyle(|st, t| bar::with_size(st, t, s), cx))),
        );
    }
    Some(row.into_any_element())
}

fn hsla(c: Rgba) -> Hsla {
    gpui::rgb((c.r as u32) << 16 | (c.g as u32) << 8 | c.b as u32).into()
}

fn swatch(model: Entity<OverlayModel>, index: usize, color: Rgba, picked: bool) -> AnyElement {
    let name = bar::color_name(color);
    let tip = format!("{name}  ·  {}", index + 1);
    let activate = model.clone();
    div()
        .id(gpui::ElementId::Name(format!("strip-color-{index}").into()))
        .size(px(SWATCH))
        .flex_none()
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .cursor_pointer()
        // The picked colour wears a ring, so the choice never rests on the colour alone.
        .border_2()
        .border_color(if picked { rgba(0xffffff, 0.95) } else { gpui::transparent_black() })
        .hover(move |d| if picked { d } else { d.border_color(rgba(0xffffff, 0.35)) })
        .child(div().size(px(18.)).rounded(radius::PILL).bg(hsla(color)).border_1().border_color(rgba(0xffffff, 0.25)))
        .role(Role::RadioButton)
        .aria_label(name)
        .aria_keyshortcuts((index + 1).to_string())
        .aria_toggled(if picked { Toggled::True } else { Toggled::False })
        .tooltip(move |window, cx| Tooltip::new(tip.clone()).build(window, cx))
        .on_a11y_action(A11yAction::Click, move |_, _, cx| {
            activate.update(cx, |m, cx| m.restyle(|st, t| bar::with_color(st, t, color), cx))
        })
        .on_click(move |_, _, cx| model.update(cx, |m, cx| m.restyle(|st, t| bar::with_color(st, t, color), cx)))
        .into_any_element()
}

/// The selection's size (`bar::badge_text`) above (or below) it.
pub(crate) fn size_badge(text: String, at: bar::Rect) -> AnyElement {
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
        .child(text)
        .into_any_element()
}

/// The notice's height.
pub(crate) const NOTICE_H: f32 = 30.0;

/// The notice's width for `text`.
pub(crate) fn notice_width(text: &str) -> f32 {
    28.0 + 7.0 * text.chars().count() as f32
}

/// A short notice at `at`, beside the toolbar, such as the warning before marks are discarded.
pub(crate) fn notice(text: String, at: bar::Rect) -> AnyElement {
    div()
        .absolute()
        .left(px(at.x))
        .top(px(at.y))
        .w(px(at.w))
        .h(px(at.h))
        .flex()
        .items_center()
        .justify_center()
        .rounded(radius::PILL)
        .bg(rgba(0x1c1c1f, 0.95))
        .border_1()
        .border_color(rgba(0xff6b6b, 0.7))
        .text_color(rgba(0xffffff, 0.95))
        .text_size(px(12.5))
        .child(text)
        .into_any_element()
}

/// A short line at the top of the screen before anything is selected.
pub(crate) fn hint(win_w: f32, text: impl Into<SharedString>) -> AnyElement {
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
                .child(text.into()),
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
            Mode::Text => "Read the text in the area you select",
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
                    self.toggle_text();
                } else {
                    self.text_mode = !self.text_mode;
                }
            }
            Mode::Video => self.switch_to(laqta_domain::Action::ScreenRecorder, cx),
            Mode::ColorPicker => self.switch_to(laqta_domain::Action::ScreenColorPicker, cx),
            Mode::Close => self.cancel(cx),
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
            Mode::Region => !self.text_mode && self.text.is_none(),
            Mode::EditAfter => self.view.annotate,
            Mode::Text => self.text_mode || self.text.is_some(),
            _ => false,
        }
    }
}

/// The mode bar, centred at the top of a `win_w`-wide window.
pub(crate) fn mode_bar(model: &Entity<OverlayModel>, win_w: f32, cx: &App) -> AnyElement {
    let m = model.read(cx);
    let mut row = div()
        .id("mode-bar")
        .role(Role::Toolbar)
        .aria_label("Capture mode")
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
        .on_any_mouse_down(crate::overlay_tools::press_on_bar(model.clone()))
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
                    .role(Role::Button)
                    .aria_label(tip)
                    // Only the modes that stay on are toggles; the rest act once.
                    .when(matches!(mode, Mode::Screenshot | Mode::Region | Mode::EditAfter | Mode::Text), |d| {
                        d.aria_toggled(if active { Toggled::True } else { Toggled::False })
                    })
                    .tooltip(move |window, cx| Tooltip::new(tip).build(window, cx))
                    .on_a11y_action(A11yAction::Click, {
                        let entity = entity.clone();
                        move |_, _, cx| entity.update(cx, |m, cx| m.mode(mode, cx))
                    })
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

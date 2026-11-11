//! Region selection state machine (ADR 012). Pure: no I/O, clock, env or threads.
//!
//! Rects are half-open: a rect covers `x..right()` and `y..bottom()`. Pointer positions are
//! clamped to the last pixel inside `bounds`.

use crate::capture::RegionMouseAction;
use crate::geometry::PhysRect;
use crate::image::Bitmap;

#[derive(Clone, Debug, PartialEq)]
pub enum TargetKind {
    Window,
    Element,
    Monitor,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HoverTarget {
    pub rect: PhysRect,
    /// The platform's window id (HWND, X11 window, sway con id); 0 if none. Only the platform adapter
    /// that produced it reads it (ADR 016's sealed-reference rule).
    pub handle: u64,
    pub title: String,
    pub process: String,
    pub kind: TargetKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RegionConfig {
    pub bounds: PhysRect,
    pub multi_region: bool,
    /// Whether drawing or clicking a region ends the session at once. When false, the region stays as
    /// the active, adjustable selection until the user confirms it (Enter or `Finish`).
    pub confirm_on_select: bool,
    /// Pick one point (the screen colour picker): a click, or Enter, confirms the 1×1 rect under
    /// the cursor; there is no drawing.
    pub pick_point: bool,
    pub snap_to_targets: bool,
    pub min_size: u32,
    pub magnifier_pixels: u32,
    pub info_text: String,
    /// What a right press does while idle.
    pub right_click: RegionMouseAction,
    /// What a middle press does while idle.
    pub middle_click: RegionMouseAction,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Right,
    Middle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionKey {
    Left,
    Right,
    Up,
    Down,
    Enter,
    Escape,
    Delete,
    Tab,
    /// Shows or hides the capture tools.
    Space,
}

/// What the user chose to do with a confirmed selection. `Default` leaves the capture to the
/// configured after-capture steps.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum SelectionAction {
    #[default]
    Default,
    Copy,
    Save,
    Pin,
    Upload,
    /// Open the selection in the image editor.
    Edit,
    /// Recognise the text in the selection and copy it.
    CopyText,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Debug)]
pub enum RegionInput {
    PointerMove {
        x: i32,
        y: i32,
    },
    PointerDown {
        x: i32,
        y: i32,
        button: Button,
    },
    PointerUp {
        x: i32,
        y: i32,
        button: Button,
    },
    Key {
        key: RegionKey,
        mods: KeyMods,
    },
    TargetsLoaded(Vec<HoverTarget>),
    /// Mouse wheel notches over the overlay: positive grows the hover to the enclosing target,
    /// negative shrinks it back.
    Wheel(i32),
    /// A button on the overlay toolbar.
    Command(RegionCommand),
    /// The selection's pixels with the overlay's annotations drawn on. The session does not keep it:
    /// the app that runs the session takes it before the input reaches the state, and the state
    /// ignores it.
    Annotated(Bitmap),
    /// The overlay's text button, or a copy from the text it shows. Like `Annotated`, the app takes
    /// it before it reaches the state: the recogniser and the clipboard are the app's.
    Text(TextCommand),
}

/// What the overlay asks about the text in the selection (ADR 031).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextCommand {
    /// Read the text in the selection and show it in place. The text follows the selection from
    /// then on: a new selection is read again.
    Read,
    /// Leave the text and go back to the selection.
    Close,
    /// Put this text, the words the user picked, on the clipboard.
    Copy(String),
    /// End the session and show what was read in the text window, where it can be edited and saved.
    OpenWindow,
}

impl PartialEq for RegionInput {
    /// `Bitmap` has no equality of its own, so two annotated crops are equal when their size,
    /// format and pixels are.
    fn eq(&self, other: &Self) -> bool {
        use RegionInput::{Annotated, Command, Key, PointerDown, PointerMove, PointerUp, TargetsLoaded, Text, Wheel};
        match (self, other) {
            (PointerMove { x: ax, y: ay }, PointerMove { x: bx, y: by }) => (ax, ay) == (bx, by),
            (PointerDown { x: ax, y: ay, button: a }, PointerDown { x: bx, y: by, button: b }) => (ax, ay, a) == (bx, by, b),
            (PointerUp { x: ax, y: ay, button: a }, PointerUp { x: bx, y: by, button: b }) => (ax, ay, a) == (bx, by, b),
            (Key { key: ak, mods: am }, Key { key: bk, mods: bm }) => (ak, am) == (bk, bm),
            (TargetsLoaded(a), TargetsLoaded(b)) => a == b,
            (Wheel(a), Wheel(b)) => a == b,
            (Command(a), Command(b)) => a == b,
            (Text(a), Text(b)) => a == b,
            (Annotated(a), Annotated(b)) => {
                (a.width(), a.height(), a.format()) == (b.width(), b.height(), b.format()) && a.pixels() == b.pixels()
            }
            _ => false,
        }
    }
}

/// The overlay toolbar's buttons. `Confirm` and `Annotate` act on drawn regions only: while a
/// button is clicked the pointer is on the toolbar, so the target under it was not chosen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionCommand {
    Confirm,
    Cancel,
    Fullscreen,
    ActiveMonitor,
    /// Confirm, then open the selection in the editor before the after-capture steps.
    Annotate,
    /// Confirm the active region (every region in multi-region mode) with an action. Does nothing
    /// while no region is drawn.
    Finish(SelectionAction),
    /// Freeze the selection: pointer and arrow-key edits are ignored until `Unlock`. The overlay
    /// locks once annotations exist on the selection, since moving it would leave them behind.
    Lock,
    Unlock,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Handle {
    N,
    NE,
    E,
    SE,
    S,
    SW,
    W,
    NW,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegionOutcome {
    Confirmed {
        rects: Vec<PhysRect>,
        target: Option<HoverTarget>,
        action: SelectionAction,
    },
    /// Capture the whole desktop.
    Fullscreen,
    /// Capture the monitor under the cursor.
    ActiveMonitor,
    Cancelled,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Phase {
    Idle,
    Drawing { anchor: (i32, i32) },
    Moving { grab: (i32, i32), original: PhysRect },
    Resizing { handle: Handle, original: PhysRect },
    Finished(RegionOutcome),
}

pub const HANDLE_SIZE: u32 = 8;

/// The region state machine. Feed it input with [`RegionState::handle`]; it reports when the
/// overlay must repaint and the outcome once the session ends.
#[derive(Debug)]
pub struct RegionState {
    config: RegionConfig,
    /// Hover targets, front to back. Inside a window, its regions come first, smallest first,
    /// and the window itself last, so the first target under the cursor is the smallest.
    targets: Vec<HoverTarget>,
    /// How many enclosing targets the wheel has grown the hover by.
    level: usize,
    /// The smallest target under the cursor when `level` was set.
    base: Option<usize>,
    regions: Vec<PhysRect>,
    /// The target each region was confirmed from, if any, parallel to `regions`.
    region_targets: Vec<Option<HoverTarget>>,
    /// Set when the session was confirmed with `Annotate`.
    annotate: bool,
    /// Whether the capture tools are shown; Space toggles it.
    tools_visible: bool,
    /// Set by `Lock`: the selection can no longer be moved, resized or removed.
    locked: bool,
    active: Option<usize>,
    /// Index into `targets` of the target under the cursor.
    hover: Option<usize>,
    cursor: (i32, i32),
    phase: Phase,
}

/// What `handle` compares before and after an input to decide whether to repaint.
#[derive(PartialEq)]
struct View {
    cursor: (i32, i32),
    phase: Phase,
    regions: Vec<PhysRect>,
    active: Option<usize>,
    hover: Option<usize>,
    tools_visible: bool,
    locked: bool,
}

impl RegionState {
    /// Whether the user confirmed with `Annotate`: the capture opens in the editor first.
    pub fn annotate_requested(&self) -> bool {
        self.annotate
    }

    /// Whether the capture tools are shown. Starts true; Space toggles it.
    pub fn tools_visible(&self) -> bool {
        self.tools_visible
    }

    /// Whether the selection is frozen by `Lock`.
    pub fn locked(&self) -> bool {
        self.locked
    }

    /// The region held as the adjustable selection: the active region of a single-region session
    /// that does not end on select, once no gesture is under way.
    pub fn selected(&self) -> Option<PhysRect> {
        let waiting = !self.config.confirm_on_select && !self.config.multi_region && matches!(self.phase, Phase::Idle);
        self.active_region().filter(|_| waiting)
    }

    pub fn new(config: RegionConfig) -> Self {
        let cursor = centre(config.bounds);
        Self {
            config,
            targets: Vec::new(),
            level: 0,
            base: None,
            regions: Vec::new(),
            region_targets: Vec::new(),
            annotate: false,
            tools_visible: true,
            locked: false,
            active: None,
            hover: None,
            cursor,
            phase: Phase::Idle,
        }
    }

    /// Applies one input. Returns true if anything visible changed and the overlay should repaint.
    pub fn handle(&mut self, input: RegionInput) -> bool {
        if matches!(self.phase, Phase::Finished(_)) {
            return false;
        }
        match input {
            RegionInput::TargetsLoaded(targets) => self.load_targets(targets),
            input => {
                let before = self.view();
                self.step(input);
                before != self.view()
            }
        }
    }

    pub fn phase(&self) -> &Phase {
        &self.phase
    }

    pub fn outcome(&self) -> Option<&RegionOutcome> {
        match &self.phase {
            Phase::Finished(outcome) => Some(outcome),
            _ => None,
        }
    }

    /// The session picks one pixel (`RegionConfig::pick_point`).
    pub fn picks_point(&self) -> bool {
        self.config.pick_point
    }

    pub fn regions(&self) -> &[PhysRect] {
        &self.regions
    }

    pub fn active_region(&self) -> Option<PhysRect> {
        self.active.and_then(|i| self.regions.get(i).copied())
    }

    /// The rect being drawn right now (normalized), if drawing.
    pub fn drawing_rect(&self) -> Option<PhysRect> {
        match self.phase {
            Phase::Drawing { anchor } => Some(rect_from_points(anchor, self.cursor)),
            _ => None,
        }
    }

    pub fn hovered_target(&self) -> Option<&HoverTarget> {
        self.hover.and_then(|i| self.targets.get(i))
    }

    pub fn cursor(&self) -> (i32, i32) {
        self.cursor
    }

    /// The eight handles of the active region, centred on its corners and edge midpoints.
    pub fn handle_rects(&self) -> Vec<(Handle, PhysRect)> {
        let Some(r) = self.active_region() else {
            return Vec::new();
        };
        let half = to_i32(HANDLE_SIZE / 2);
        let left = r.x;
        let centre_x = r.x.saturating_add_unsigned(r.width / 2);
        let right = r.right().saturating_sub(1);
        let top = r.y;
        let centre_y = r.y.saturating_add_unsigned(r.height / 2);
        let bottom = r.bottom().saturating_sub(1);
        let at = |handle: Handle, x: i32, y: i32| {
            let rect = PhysRect { x: x.saturating_sub(half), y: y.saturating_sub(half), width: HANDLE_SIZE, height: HANDLE_SIZE };
            (handle, rect)
        };
        vec![
            at(Handle::N, centre_x, top),
            at(Handle::NE, right, top),
            at(Handle::E, right, centre_y),
            at(Handle::SE, right, bottom),
            at(Handle::S, centre_x, bottom),
            at(Handle::SW, left, bottom),
            at(Handle::W, left, centre_y),
            at(Handle::NW, left, top),
        ]
    }

    /// The square sample rect of `magnifier_pixels` (odd, at least 1) centred on the cursor and
    /// kept inside the bounds.
    pub fn magnifier_rect(&self) -> PhysRect {
        let bounds = self.config.bounds;
        let side = magnifier_side(self.config.magnifier_pixels, bounds);
        let half = to_i32(side / 2);
        let x = self.cursor.0.saturating_sub(half);
        let y = self.cursor.1.saturating_sub(half);
        clamp_rect(PhysRect { x, y, width: side, height: side }, bounds)
    }

    /// Expands `$x $y` (cursor relative to the bounds origin), `$r $g $b` and `$hex`/`$HEX`
    /// (the sampled pixel), `$w $h` (the drawing or active region, else 0) and `$n` (newline).
    /// Unknown `$` sequences are kept as written.
    pub fn info_text(&self, pixel: [u8; 3]) -> String {
        let bounds = self.config.bounds;
        let [r, g, b] = pixel;
        let size = self.drawing_rect().or_else(|| self.active_region());
        let (w, h) = size.map_or((0, 0), |s| (s.width, s.height));
        // Longer tokens come first so `$hex` is not read as `$h` followed by `ex`.
        let tokens: [(&str, String); 10] = [
            ("$hex", format!("{r:02x}{g:02x}{b:02x}")),
            ("$HEX", format!("{r:02X}{g:02X}{b:02X}")),
            ("$x", self.cursor.0.saturating_sub(bounds.x).to_string()),
            ("$y", self.cursor.1.saturating_sub(bounds.y).to_string()),
            ("$r", r.to_string()),
            ("$g", g.to_string()),
            ("$b", b.to_string()),
            ("$h", h.to_string()),
            ("$w", w.to_string()),
            ("$n", "\n".to_owned()),
        ];
        let text = self.config.info_text.as_str();
        let mut out = String::with_capacity(text.len());
        let mut rest = text;
        while let Some(at) = rest.find('$') {
            out.push_str(&rest[..at]);
            let tail = &rest[at..];
            match tokens.iter().find(|(token, _)| tail.starts_with(*token)) {
                Some((token, value)) => {
                    out.push_str(value);
                    rest = &tail[token.len()..];
                }
                None => {
                    out.push('$');
                    rest = &tail[1..];
                }
            }
        }
        out.push_str(rest);
        out
    }

    fn view(&self) -> View {
        View {
            cursor: self.cursor,
            phase: self.phase.clone(),
            regions: self.regions.clone(),
            active: self.active,
            hover: self.hover,
            tools_visible: self.tools_visible,
            locked: self.locked,
        }
    }

    fn step(&mut self, input: RegionInput) {
        match input {
            RegionInput::PointerMove { x, y } => self.pointer_move((x, y)),
            RegionInput::PointerDown { x, y, button } => self.pointer_down((x, y), button),
            RegionInput::PointerUp { x, y, button } => self.pointer_up((x, y), button),
            RegionInput::Key { key, mods } => self.key(key, mods),
            RegionInput::Wheel(steps) => self.wheel(steps),
            RegionInput::Command(RegionCommand::Confirm) => {
                self.confirm_regions(SelectionAction::Default);
            }
            // With a drawn region, confirm it for annotation; with none yet, toggle annotate mode, so
            // the next confirmed selection opens in the editor.
            RegionInput::Command(RegionCommand::Annotate) => {
                if self.regions.is_empty() {
                    self.annotate = !self.annotate;
                } else {
                    self.annotate = true;
                    self.confirm_regions(SelectionAction::Default);
                }
            }
            RegionInput::Command(RegionCommand::Finish(action)) => {
                self.confirm_regions(action);
            }
            RegionInput::Command(RegionCommand::Lock) => self.lock(),
            RegionInput::Command(RegionCommand::Unlock) => self.locked = false,
            RegionInput::Command(RegionCommand::Cancel) => self.finish(RegionOutcome::Cancelled),
            RegionInput::Command(RegionCommand::Fullscreen) => self.finish(RegionOutcome::Fullscreen),
            RegionInput::Command(RegionCommand::ActiveMonitor) => self.finish(RegionOutcome::ActiveMonitor),
            // `handle` routes this variant to `load_targets`.
            RegionInput::TargetsLoaded(_) => {}
            // The app takes the annotated crop; the selection itself does not change.
            RegionInput::Annotated(_) => {}
            // The app reads and copies text; the selection itself does not change.
            RegionInput::Text(_) => {}
        }
    }

    /// Replaces the target list. Returns true if the list changed.
    fn load_targets(&mut self, targets: Vec<HoverTarget>) -> bool {
        let changed = self.targets != targets;
        if changed {
            self.targets = targets;
            self.base = None;
            self.update_hover();
        }
        changed
    }

    fn set_cursor(&mut self, at: (i32, i32)) {
        self.cursor = clamp_point(at, self.config.bounds);
        if matches!(self.phase, Phase::Idle) {
            self.update_hover();
        }
    }

    /// The targets under the cursor, smallest first, ending at the front window (or the last
    /// one if no window is under it): what the wheel steps through.
    fn chain(&self) -> Vec<usize> {
        if !self.config.snap_to_targets {
            return Vec::new();
        }
        let mut chain = Vec::new();
        for (i, t) in self.targets.iter().enumerate() {
            if contains(t.rect, self.cursor) {
                chain.push(i);
                if t.kind != TargetKind::Element {
                    break;
                }
            }
        }
        chain
    }

    /// Picks the hover at the current wheel level. The level resets when the smallest target
    /// under the cursor changes, so moving onto another element starts from the smallest again.
    fn update_hover(&mut self) {
        let chain = self.chain();
        if chain.first().copied() != self.base {
            self.base = chain.first().copied();
            self.level = 0;
        }
        self.hover = chain.get(self.level.min(chain.len().saturating_sub(1))).copied();
    }

    fn wheel(&mut self, steps: i32) {
        if !matches!(self.phase, Phase::Idle) {
            return;
        }
        let chain = self.chain();
        if chain.is_empty() {
            return;
        }
        let top = chain.len() - 1;
        let level = self.level.min(top);
        self.level = if steps >= 0 {
            level.saturating_add(steps.unsigned_abs() as usize).min(top)
        } else {
            level.saturating_sub(steps.unsigned_abs() as usize)
        };
        self.hover = chain.get(self.level).copied();
    }

    fn set_active(&mut self, rect: PhysRect) {
        if let Some(i) = self.active {
            self.regions[i] = rect;
        }
    }

    /// The pixel under the cursor, as a 1×1 selection.
    fn pick_point(&mut self) {
        let (x, y) = self.cursor;
        let rect = PhysRect { x, y, width: 1, height: 1 };
        self.finish(RegionOutcome::Confirmed { rects: vec![rect], target: None, action: SelectionAction::Default });
    }

    fn finish(&mut self, outcome: RegionOutcome) {
        self.phase = Phase::Finished(outcome);
    }

    fn enter_idle(&mut self) {
        self.phase = Phase::Idle;
        self.update_hover();
    }

    /// Freezes the selection. A gesture under way is dropped first, as a right press would.
    fn lock(&mut self) {
        self.cancel_gesture();
        self.locked = true;
    }

    fn pointer_move(&mut self, at: (i32, i32)) {
        self.set_cursor(at);
        if self.locked {
            return;
        }
        match self.phase {
            Phase::Moving { grab, original } => {
                let dx = self.cursor.0.saturating_sub(grab.0);
                let dy = self.cursor.1.saturating_sub(grab.1);
                let moved = PhysRect { x: original.x.saturating_add(dx), y: original.y.saturating_add(dy), ..original };
                self.set_active(clamp_rect(moved, self.config.bounds));
            }
            Phase::Resizing { handle, original } => {
                self.set_active(resize(original, handle, self.cursor, self.config.bounds));
            }
            _ => {}
        }
    }

    fn pointer_down(&mut self, at: (i32, i32), button: Button) {
        self.set_cursor(at);
        if self.config.pick_point && button == Button::Left {
            self.pick_point();
            return;
        }
        // The pointer belongs to the annotation tools now; every button is theirs, not the session's.
        if self.locked {
            return;
        }
        match button {
            Button::Left if matches!(self.phase, Phase::Idle) => self.start_gesture(),
            Button::Left => {}
            Button::Right | Button::Middle if matches!(self.phase, Phase::Idle) => {
                let action = if button == Button::Right { self.config.right_click } else { self.config.middle_click };
                self.apply_mouse_action(action);
            }
            Button::Right | Button::Middle => self.cancel_gesture(),
        }
    }

    /// Left press in Idle: resize a handle of the active region, move a region, or draw.
    fn start_gesture(&mut self) {
        let c = self.cursor;
        if let Some(i) = self.active {
            let original = self.regions[i];
            if let Some(handle) = self.handle_at(c) {
                self.phase = Phase::Resizing { handle, original };
                return;
            }
            if contains(original, c) {
                self.phase = Phase::Moving { grab: c, original };
                return;
            }
        }
        if let Some(i) = self.regions.iter().rposition(|r| contains(*r, c)) {
            self.active = Some(i);
            self.phase = Phase::Moving { grab: c, original: self.regions[i] };
            return;
        }
        self.phase = Phase::Drawing { anchor: c };
    }

    fn handle_at(&self, at: (i32, i32)) -> Option<Handle> {
        self.handle_rects().into_iter().find(|(_, rect)| contains(*rect, at)).map(|(handle, _)| handle)
    }

    fn pointer_up(&mut self, at: (i32, i32), button: Button) {
        self.set_cursor(at);
        if self.locked || button != Button::Left {
            return;
        }
        match self.phase {
            Phase::Drawing { anchor } => self.finish_drawing(anchor),
            Phase::Moving { .. } | Phase::Resizing { .. } => self.enter_idle(),
            Phase::Idle | Phase::Finished(_) => {}
        }
    }

    fn finish_drawing(&mut self, anchor: (i32, i32)) {
        let bounds = self.config.bounds;
        let drawn = rect_from_points(anchor, self.cursor);
        if drawn.width < self.config.min_size && drawn.height < self.config.min_size {
            // A click: take the target under the press, if there is one.
            match self.hovered_target().cloned() {
                Some(target) => self.add_region(clamp_rect(target.rect, bounds), Some(target)),
                None => self.enter_idle(),
            }
        } else {
            self.add_region(clamp_rect(drawn, bounds), None);
        }
    }

    fn add_region(&mut self, rect: PhysRect, target: Option<HoverTarget>) {
        if !self.config.multi_region {
            // One selection at a time: a new draw replaces the held one.
            self.regions.clear();
            self.region_targets.clear();
        }
        self.regions.push(rect);
        self.region_targets.push(target.clone());
        self.active = Some(self.regions.len() - 1);
        if !self.config.multi_region && self.config.confirm_on_select {
            self.finish(RegionOutcome::Confirmed { rects: vec![rect], target, action: SelectionAction::Default });
        } else {
            self.enter_idle();
        }
    }

    /// A right or middle press during a draw, move or resize drops the gesture. A move or resize
    /// goes back to where it started.
    fn cancel_gesture(&mut self) {
        match self.phase {
            Phase::Drawing { .. } => self.enter_idle(),
            Phase::Moving { original, .. } | Phase::Resizing { original, .. } => {
                self.set_active(original);
                self.enter_idle();
            }
            Phase::Idle | Phase::Finished(_) => {}
        }
    }

    /// The configured action for a right or middle press while idle.
    fn apply_mouse_action(&mut self, action: RegionMouseAction) {
        let under_cursor = self.regions.iter().rposition(|r| contains(*r, self.cursor));
        match action {
            RegionMouseAction::None => {}
            RegionMouseAction::CancelCapture => self.finish(RegionOutcome::Cancelled),
            RegionMouseAction::RemoveShapeOrCancel => match under_cursor {
                Some(i) if self.config.multi_region => self.remove_region(i),
                _ => self.finish(RegionOutcome::Cancelled),
            },
            RegionMouseAction::RemoveShape => {
                if let Some(i) = under_cursor {
                    self.remove_region(i);
                }
            }
            RegionMouseAction::CaptureFullscreen => self.finish(RegionOutcome::Fullscreen),
            RegionMouseAction::CaptureActiveMonitor => self.finish(RegionOutcome::ActiveMonitor),
            RegionMouseAction::ConfirmSelection => self.confirm(),
        }
    }

    fn remove_region(&mut self, i: usize) {
        self.regions.remove(i);
        self.region_targets.remove(i);
        self.active = match self.active {
            Some(a) if a == i => None,
            Some(a) if a > i => Some(a - 1),
            other => other,
        };
    }

    fn key(&mut self, key: RegionKey, mods: KeyMods) {
        match key {
            RegionKey::Escape => self.finish(RegionOutcome::Cancelled),
            RegionKey::Enter if self.config.pick_point => self.pick_point(),
            RegionKey::Enter => self.confirm(),
            RegionKey::Space => self.tools_visible = !self.tools_visible,
            _ if self.locked => {}
            _ if matches!(self.phase, Phase::Idle) => self.idle_key(key, mods),
            _ => {}
        }
    }

    fn idle_key(&mut self, key: RegionKey, mods: KeyMods) {
        match key {
            RegionKey::Delete => {
                if let Some(i) = self.active {
                    self.remove_region(i);
                }
            }
            RegionKey::Tab => self.cycle_active(),
            RegionKey::Left | RegionKey::Right | RegionKey::Up | RegionKey::Down => {
                self.nudge(key, mods);
            }
            RegionKey::Enter | RegionKey::Escape | RegionKey::Space => {}
        }
    }

    fn cycle_active(&mut self) {
        let n = self.regions.len();
        if n > 0 {
            self.active = Some(self.active.map_or(0, |a| (a + 1) % n));
        }
    }

    /// Arrow keys: move the active region (Shift 10 px), resize it with Ctrl (Right and Down
    /// grow, Left and Up shrink), or move the cursor when there is no region.
    fn nudge(&mut self, key: RegionKey, mods: KeyMods) {
        let step = if mods.shift { 10 } else { 1 };
        let (dx, dy) = arrow_delta(key, step);
        let Some(r) = self.active.map(|i| self.regions[i]) else {
            let moved = (self.cursor.0.saturating_add(dx), self.cursor.1.saturating_add(dy));
            self.set_cursor(moved);
            return;
        };
        let bounds = self.config.bounds;
        let next = if mods.ctrl {
            PhysRect {
                width: add_size(r.width, dx).min(room(r.x, bounds.right()).max(1)),
                height: add_size(r.height, dy).min(room(r.y, bounds.bottom()).max(1)),
                ..r
            }
        } else {
            PhysRect { x: r.x.saturating_add(dx), y: r.y.saturating_add(dy), ..r }
        };
        self.set_active(clamp_rect(next, bounds));
    }

    /// Enter and the confirm mouse action: the drawn regions, else the target under the cursor.
    fn confirm(&mut self) {
        if self.confirm_regions(SelectionAction::Default) {
            return;
        }
        if let Some(target) = self.hovered_target().cloned() {
            let rect = clamp_rect(target.rect, self.config.bounds);
            self.finish(RegionOutcome::Confirmed { rects: vec![rect], target: Some(target), action: SelectionAction::Default });
        }
    }

    /// Ends the session with the drawn regions: all of them in multi-region mode, else the active
    /// one. Returns false, and does nothing, when no region is drawn.
    fn confirm_regions(&mut self, action: SelectionAction) -> bool {
        if self.regions.is_empty() {
            return false;
        }
        let outcome = if self.config.multi_region {
            RegionOutcome::Confirmed { rects: self.regions.clone(), target: None, action }
        } else {
            let i = self.active.unwrap_or(0);
            RegionOutcome::Confirmed { rects: vec![self.regions[i]], target: self.region_targets[i].clone(), action }
        };
        self.finish(outcome);
        true
    }
}

/// Which way each arrow key moves, in pixels.
fn arrow_delta(key: RegionKey, step: i32) -> (i32, i32) {
    match key {
        RegionKey::Left => (-step, 0),
        RegionKey::Right => (step, 0),
        RegionKey::Up => (0, -step),
        RegionKey::Down => (0, step),
        _ => (0, 0),
    }
}

/// The new edges of a resize. Edges that the handle drags sit on the pointer (the right and
/// bottom edges one pixel past it, since the pointer's own pixel is inside the rect). The
/// others stay where they were. Normalizing the two lines makes a flip just a min and a width.
fn resize(original: PhysRect, handle: Handle, cursor: (i32, i32), bounds: PhysRect) -> PhysRect {
    let moves_left = matches!(handle, Handle::NW | Handle::W | Handle::SW);
    let moves_right = matches!(handle, Handle::NE | Handle::E | Handle::SE);
    let moves_top = matches!(handle, Handle::NW | Handle::N | Handle::NE);
    let moves_bottom = matches!(handle, Handle::SW | Handle::S | Handle::SE);

    let a_x = if moves_left { cursor.0 } else { original.x };
    let b_x = if moves_right { cursor.0.saturating_add(1) } else { original.right() };
    let a_y = if moves_top { cursor.1 } else { original.y };
    let b_y = if moves_bottom { cursor.1.saturating_add(1) } else { original.bottom() };

    clamp_rect(PhysRect { x: a_x.min(b_x), y: a_y.min(b_y), width: a_x.abs_diff(b_x).max(1), height: a_y.abs_diff(b_y).max(1) }, bounds)
}

/// The magnifier side: odd, at least 1, and no bigger than the bounds.
fn magnifier_side(pixels: u32, bounds: PhysRect) -> u32 {
    let fit = bounds.width.min(bounds.height).max(1);
    let fit_odd = if fit.is_multiple_of(2) { fit - 1 } else { fit };
    (pixels.max(1) | 1).min(fit_odd)
}

fn centre(bounds: PhysRect) -> (i32, i32) {
    let at = (bounds.x.saturating_add_unsigned(bounds.width / 2), bounds.y.saturating_add_unsigned(bounds.height / 2));
    clamp_point(at, bounds)
}

fn clamp_point(at: (i32, i32), bounds: PhysRect) -> (i32, i32) {
    let max_x = bounds.right().saturating_sub(1).max(bounds.x);
    let max_y = bounds.bottom().saturating_sub(1).max(bounds.y);
    (at.0.clamp(bounds.x, max_x), at.1.clamp(bounds.y, max_y))
}

/// Whether the pixel at `at` lies inside `rect`.
fn contains(rect: PhysRect, (x, y): (i32, i32)) -> bool {
    (rect.x..rect.right()).contains(&x) && (rect.y..rect.bottom()).contains(&y)
}

/// How many pixels fit from `start` up to `end`, exclusive.
fn room(start: i32, end: i32) -> u32 {
    u32::try_from(end.saturating_sub(start)).unwrap_or(0)
}

/// Adds a signed delta to a size, keeping it at least 1.
fn add_size(size: u32, delta: i32) -> u32 {
    u32::try_from((i64::from(size) + i64::from(delta)).max(1)).unwrap_or(u32::MAX)
}

fn to_i32(v: u32) -> i32 {
    i32::try_from(v).unwrap_or(i32::MAX)
}

/// The rect spanned by two corner points, inclusive of both. Never smaller than 1x1.
pub fn rect_from_points(a: (i32, i32), b: (i32, i32)) -> PhysRect {
    PhysRect { x: a.0.min(b.0), y: a.1.min(b.1), width: a.0.abs_diff(b.0).saturating_add(1), height: a.1.abs_diff(b.1).saturating_add(1) }
}

/// Moves and shrinks `r` so that it lies fully inside `bounds`, keeping it at least 1x1.
pub fn clamp_rect(r: PhysRect, bounds: PhysRect) -> PhysRect {
    let width = r.width.clamp(1, bounds.width.max(1));
    let height = r.height.clamp(1, bounds.height.max(1));
    let max_x = bounds.right().saturating_sub_unsigned(width).max(bounds.x);
    let max_y = bounds.bottom().saturating_sub_unsigned(height).max(bounds.y);
    PhysRect { x: r.x.clamp(bounds.x, max_x), y: r.y.clamp(bounds.y, max_y), width, height }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOUNDS: PhysRect = PhysRect { x: 0, y: 0, width: 1000, height: 800 };

    fn config() -> RegionConfig {
        RegionConfig {
            bounds: BOUNDS,
            multi_region: false,
            confirm_on_select: true,
            pick_point: false,
            snap_to_targets: true,
            min_size: 5,
            magnifier_pixels: 5,
            info_text: String::new(),
            right_click: RegionMouseAction::RemoveShapeOrCancel,
            middle_click: RegionMouseAction::CancelCapture,
        }
    }

    fn multi() -> RegionConfig {
        RegionConfig { multi_region: true, ..config() }
    }

    fn with_clicks(right_click: RegionMouseAction, middle_click: RegionMouseAction, multi_region: bool) -> RegionConfig {
        RegionConfig { multi_region, right_click, middle_click, ..config() }
    }

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    fn window(title: &str, area: PhysRect) -> HoverTarget {
        HoverTarget { handle: 0, rect: area, title: title.to_owned(), process: "app.exe".to_owned(), kind: TargetKind::Window }
    }

    fn move_to(x: i32, y: i32) -> RegionInput {
        RegionInput::PointerMove { x, y }
    }

    fn press(x: i32, y: i32) -> RegionInput {
        RegionInput::PointerDown { x, y, button: Button::Left }
    }

    fn release(x: i32, y: i32) -> RegionInput {
        RegionInput::PointerUp { x, y, button: Button::Left }
    }

    fn right_press(x: i32, y: i32) -> RegionInput {
        RegionInput::PointerDown { x, y, button: Button::Right }
    }

    fn middle_press(x: i32, y: i32) -> RegionInput {
        RegionInput::PointerDown { x, y, button: Button::Middle }
    }

    fn tap(key: RegionKey) -> RegionInput {
        RegionInput::Key { key, mods: KeyMods::default() }
    }

    fn tap_with(key: RegionKey, shift: bool, ctrl: bool) -> RegionInput {
        RegionInput::Key { key, mods: KeyMods { shift, ctrl, alt: false } }
    }

    /// Left-drags from `from` to `to` and releases there.
    fn drag(state: &mut RegionState, from: (i32, i32), to: (i32, i32)) {
        state.handle(press(from.0, from.1));
        state.handle(move_to(to.0, to.1));
        state.handle(release(to.0, to.1));
    }

    #[test]
    fn click_on_hovered_window_confirms_its_rect_and_target() {
        let mut state = RegionState::new(config());
        let front = window("Editor", rect(300, 200, 400, 300));
        let back = window("Desktop", rect(0, 0, 1000, 800));
        state.handle(RegionInput::TargetsLoaded(vec![front.clone(), back]));
        state.handle(move_to(400, 300));
        assert_eq!(state.hovered_target(), Some(&front));
        state.handle(press(400, 300));
        state.handle(release(400, 300));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![front.rect], target: Some(front.clone()), action: SelectionAction::Default })
        );
    }

    #[test]
    fn press_and_release_without_a_move_clicks_the_target() {
        let mut state = RegionState::new(config());
        let win = window("Editor", rect(300, 200, 400, 300));
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(press(350, 250));
        state.handle(release(350, 250));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![win.rect], target: Some(win), action: SelectionAction::Default })
        );
    }

    #[test]
    fn drag_draws_a_normalised_rect() {
        let mut state = RegionState::new(config());
        state.handle(press(100, 100));
        assert!(state.handle(move_to(200, 150)));
        assert_eq!(state.drawing_rect(), Some(rect(100, 100, 101, 51)));
        state.handle(release(200, 150));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![rect(100, 100, 101, 51)], target: None, action: SelectionAction::Default })
        );

        let mut backwards = RegionState::new(config());
        drag(&mut backwards, (200, 150), (100, 100));
        assert_eq!(backwards.outcome(), state.outcome());
    }

    #[test]
    fn tiny_drag_without_a_target_is_ignored() {
        let mut state = RegionState::new(config());
        state.handle(press(10, 10));
        state.handle(move_to(12, 11));
        state.handle(release(12, 11));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.outcome(), None);
        assert!(state.regions().is_empty());
    }

    #[test]
    fn multi_region_adds_and_enter_confirms_all() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        assert_eq!(state.regions(), &[rect(10, 10, 51, 51), rect(100, 100, 51, 51)]);
        assert_eq!(state.active_region(), Some(rect(100, 100, 51, 51)));
        assert_eq!(state.phase(), &Phase::Idle);
        state.handle(tap(RegionKey::Enter));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed {
                rects: vec![rect(10, 10, 51, 51), rect(100, 100, 51, 51)],
                target: None,
                action: SelectionAction::Default
            })
        );
    }

    #[test]
    fn right_click_removes_the_region_under_the_cursor() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        assert!(state.handle(right_press(30, 30)));
        assert_eq!(state.regions(), &[rect(100, 100, 51, 51)]);
        assert_eq!(state.active_region(), Some(rect(100, 100, 51, 51)));

        state.handle(right_press(500, 500));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn delete_removes_the_active_region_and_tab_cycles() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        state.handle(tap(RegionKey::Tab));
        assert_eq!(state.active_region(), Some(rect(10, 10, 51, 51)));
        state.handle(tap(RegionKey::Delete));
        assert_eq!(state.regions(), &[rect(100, 100, 51, 51)]);
        assert_eq!(state.active_region(), None);
    }

    #[test]
    fn escape_cancels_and_later_input_is_ignored() {
        let mut state = RegionState::new(config());
        state.handle(press(10, 10));
        assert!(state.handle(tap(RegionKey::Escape)));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
        assert!(!state.handle(move_to(50, 50)));
        assert!(!state.handle(tap(RegionKey::Enter)));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn right_click_outside_a_multi_session_cancels() {
        let mut state = RegionState::new(config());
        assert!(state.handle(right_press(5, 5)));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn moving_a_region_clamps_at_the_bounds() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (199, 199));
        state.handle(press(150, 150));
        assert_eq!(state.phase(), &Phase::Moving { grab: (150, 150), original: rect(100, 100, 100, 100) });
        state.handle(move_to(-500, 150));
        assert_eq!(state.active_region(), Some(rect(0, 100, 100, 100)));
        state.handle(move_to(5000, 5000));
        assert_eq!(state.active_region(), Some(rect(900, 700, 100, 100)));
        state.handle(release(5000, 5000));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.active_region(), Some(rect(900, 700, 100, 100)));
    }

    #[test]
    fn right_click_cancels_a_move_and_restores_the_region() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (199, 199));
        state.handle(press(150, 150));
        state.handle(move_to(300, 300));
        assert_eq!(state.active_region(), Some(rect(250, 250, 100, 100)));
        state.handle(right_press(300, 300));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.active_region(), Some(rect(100, 100, 100, 100)));
    }

    #[test]
    fn resize_from_se_and_nw_handles_with_flip() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (199, 199));

        state.handle(press(199, 199));
        assert_eq!(state.phase(), &Phase::Resizing { handle: Handle::SE, original: rect(100, 100, 100, 100) });
        state.handle(move_to(249, 259));
        assert_eq!(state.active_region(), Some(rect(100, 100, 150, 160)));
        state.handle(release(249, 259));

        state.handle(press(100, 100));
        assert_eq!(state.phase(), &Phase::Resizing { handle: Handle::NW, original: rect(100, 100, 150, 160) });
        state.handle(move_to(60, 70));
        assert_eq!(state.active_region(), Some(rect(60, 70, 190, 190)));
        state.handle(move_to(300, 320));
        assert_eq!(state.active_region(), Some(rect(250, 260, 50, 60)));
        state.handle(release(300, 320));
        assert_eq!(state.phase(), &Phase::Idle);
    }

    #[test]
    fn se_handle_flips_past_the_top_left_corner_and_keeps_one_pixel() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (199, 199));
        state.handle(press(199, 199));
        state.handle(move_to(49, 49));
        assert_eq!(state.active_region(), Some(rect(50, 50, 50, 50)));
        state.handle(move_to(99, 99));
        assert_eq!(state.active_region(), Some(rect(100, 100, 1, 1)));
    }

    #[test]
    fn handle_rects_surround_the_active_region() {
        let mut state = RegionState::new(multi());
        assert!(state.handle_rects().is_empty());
        drag(&mut state, (100, 100), (199, 199));
        let handles = state.handle_rects();
        assert_eq!(handles.len(), 8);
        assert!(handles.contains(&(Handle::NW, rect(96, 96, 8, 8))));
        assert!(handles.contains(&(Handle::SE, rect(195, 195, 8, 8))));
    }

    #[test]
    fn arrows_nudge_and_ctrl_resize_the_region_with_shift_steps() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (149, 149));
        state.handle(tap(RegionKey::Right));
        assert_eq!(state.active_region(), Some(rect(101, 100, 50, 50)));
        state.handle(tap_with(RegionKey::Down, true, false));
        assert_eq!(state.active_region(), Some(rect(101, 110, 50, 50)));
        state.handle(tap_with(RegionKey::Right, false, true));
        assert_eq!(state.active_region(), Some(rect(101, 110, 51, 50)));
        state.handle(tap_with(RegionKey::Down, true, true));
        assert_eq!(state.active_region(), Some(rect(101, 110, 51, 60)));
        state.handle(tap_with(RegionKey::Left, true, true));
        assert_eq!(state.active_region(), Some(rect(101, 110, 41, 60)));
        state.handle(tap_with(RegionKey::Up, false, true));
        assert_eq!(state.active_region(), Some(rect(101, 110, 41, 59)));
    }

    #[test]
    fn nudges_and_resizes_stop_at_the_bounds() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (950, 100), (999, 149));
        state.handle(tap_with(RegionKey::Right, true, false));
        assert_eq!(state.active_region(), Some(rect(950, 100, 50, 50)));
        state.handle(tap_with(RegionKey::Right, false, true));
        assert_eq!(state.active_region(), Some(rect(950, 100, 50, 50)));
    }

    #[test]
    fn arrows_move_the_cursor_without_a_region() {
        let mut state = RegionState::new(config());
        assert_eq!(state.cursor(), (500, 400));
        assert!(state.handle(tap(RegionKey::Left)));
        assert_eq!(state.cursor(), (499, 400));
        state.handle(tap_with(RegionKey::Down, true, false));
        assert_eq!(state.cursor(), (499, 410));
    }

    #[test]
    fn magnifier_square_is_clamped_into_the_corners() {
        let mut state = RegionState::new(config());
        state.handle(move_to(0, 0));
        assert_eq!(state.magnifier_rect(), rect(0, 0, 5, 5));
        state.handle(move_to(999, 799));
        assert_eq!(state.magnifier_rect(), rect(995, 795, 5, 5));
        state.handle(move_to(500, 400));
        assert_eq!(state.magnifier_rect(), rect(498, 398, 5, 5));
    }

    #[test]
    fn magnifier_size_is_made_odd_and_at_least_one() {
        let mut even = config();
        even.magnifier_pixels = 4;
        assert_eq!(RegionState::new(even).magnifier_rect(), rect(498, 398, 5, 5));

        let mut zero = config();
        zero.magnifier_pixels = 0;
        assert_eq!(RegionState::new(zero).magnifier_rect(), rect(500, 400, 1, 1));
    }

    #[test]
    fn info_text_replaces_tokens_without_prefix_clashes() {
        let mut cfg = config();
        cfg.info_text = "$x,$y $r $g $b $hex $HEX$n$w x $h $z $".to_owned();
        let mut state = RegionState::new(cfg);
        state.handle(move_to(110, 220));
        assert_eq!(state.info_text([255, 16, 1]), "110,220 255 16 1 ff1001 FF1001\n0 x 0 $z $");
    }

    #[test]
    fn info_text_reports_the_drawing_size_and_relative_cursor() {
        let mut cfg = config();
        cfg.info_text = "$w x $h $hex$h".to_owned();
        let mut state = RegionState::new(cfg);
        state.handle(press(10, 10));
        state.handle(move_to(19, 29));
        assert_eq!(state.info_text([255, 16, 1]), "10 x 20 ff100120");

        let mut cfg = config();
        cfg.bounds = rect(100, 50, 1000, 800);
        cfg.info_text = "$x,$y".to_owned();
        let mut state = RegionState::new(cfg);
        state.handle(move_to(110, 60));
        assert_eq!(state.info_text([0, 0, 0]), "10,10");
    }

    fn element(area: PhysRect) -> HoverTarget {
        HoverTarget { kind: TargetKind::Element, ..window("", area) }
    }

    #[test]
    fn the_wheel_grows_the_hover_to_the_enclosing_targets_and_back() {
        let mut state = RegionState::new(config());
        let button = element(rect(420, 320, 40, 20));
        let sidebar = element(rect(400, 300, 100, 300));
        let front = window("Front", rect(300, 200, 400, 500));
        let back = window("Back", rect(0, 0, 1000, 800));
        state.handle(RegionInput::TargetsLoaded(vec![button.clone(), sidebar.clone(), front.clone(), back]));
        state.handle(move_to(430, 330));
        assert_eq!(state.hovered_target(), Some(&button));

        assert!(state.handle(RegionInput::Wheel(1)));
        assert_eq!(state.hovered_target(), Some(&sidebar));
        assert!(state.handle(RegionInput::Wheel(5)));
        assert_eq!(state.hovered_target(), Some(&front), "stops at the front window");
        assert!(!state.handle(RegionInput::Wheel(1)));
        assert!(state.handle(RegionInput::Wheel(-1)));
        assert_eq!(state.hovered_target(), Some(&sidebar));

        // Moving inside the same smallest target keeps the level.
        state.handle(move_to(435, 332));
        assert_eq!(state.hovered_target(), Some(&sidebar));
        // Leaving it resets to the smallest under the cursor.
        state.handle(move_to(450, 500));
        assert_eq!(state.hovered_target(), Some(&sidebar));
        state.handle(RegionInput::Wheel(1));
        assert_eq!(state.hovered_target(), Some(&front));
        state.handle(move_to(430, 330));
        assert_eq!(state.hovered_target(), Some(&button));
    }

    #[test]
    fn a_click_after_the_wheel_takes_the_grown_target() {
        let mut state = RegionState::new(config());
        let button = element(rect(420, 320, 40, 20));
        let front = window("Front", rect(300, 200, 400, 500));
        state.handle(RegionInput::TargetsLoaded(vec![button, front.clone()]));
        state.handle(move_to(430, 330));
        state.handle(RegionInput::Wheel(1));
        state.handle(RegionInput::PointerDown { x: 430, y: 330, button: Button::Left });
        state.handle(RegionInput::PointerUp { x: 430, y: 330, button: Button::Left });
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![front.rect], target: Some(front), action: SelectionAction::Default })
        );
    }

    #[test]
    fn targets_loaded_later_update_the_hover() {
        let mut state = RegionState::new(config());
        state.handle(move_to(400, 300));
        assert_eq!(state.hovered_target(), None);

        let win = window("Editor", rect(300, 200, 400, 300));
        assert!(state.handle(RegionInput::TargetsLoaded(vec![win.clone()])));
        assert_eq!(state.hovered_target(), Some(&win));
        assert!(!state.handle(RegionInput::TargetsLoaded(vec![win.clone()])));

        assert!(state.handle(RegionInput::TargetsLoaded(Vec::new())));
        assert_eq!(state.hovered_target(), None);
    }

    #[test]
    fn rect_helpers_are_inclusive_and_stay_inside_bounds() {
        assert_eq!(rect_from_points((5, 5), (1, 9)), rect(1, 5, 5, 5));
        assert_eq!(rect_from_points((7, 7), (7, 7)), rect(7, 7, 1, 1));
        assert_eq!(clamp_rect(rect(-10, -10, 20, 20), BOUNDS), rect(0, 0, 20, 20));
        assert_eq!(clamp_rect(rect(990, 790, 50, 50), BOUNDS), rect(950, 750, 50, 50));
        assert_eq!(clamp_rect(rect(10, 10, 5000, 5), BOUNDS), rect(0, 10, 1000, 5));
    }

    #[test]
    fn shipped_defaults_cancel_on_middle_and_remove_on_right() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (10, 10), (60, 60));
        state.handle(right_press(30, 30));
        assert!(state.regions().is_empty());
        assert_eq!(state.outcome(), None);

        drag(&mut state, (10, 10), (60, 60));
        state.handle(middle_press(30, 30));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn none_does_nothing_on_either_button() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::None, true));
        drag(&mut state, (10, 10), (60, 60));
        state.handle(right_press(30, 30));
        state.handle(middle_press(30, 30));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.outcome(), None);
        assert_eq!(state.regions(), &[rect(10, 10, 51, 51)]);
    }

    #[test]
    fn cancel_capture_cancels_on_either_button() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::CancelCapture, RegionMouseAction::None, true));
        drag(&mut state, (10, 10), (60, 60));
        state.handle(right_press(30, 30));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));

        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::CancelCapture, false));
        state.handle(middle_press(5, 5));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn remove_shape_removes_the_region_under_the_cursor_and_keeps_going() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::RemoveShape, RegionMouseAction::RemoveShape, true));
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        state.handle(right_press(30, 30));
        assert_eq!(state.regions(), &[rect(100, 100, 51, 51)]);
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.outcome(), None);

        state.handle(middle_press(500, 500));
        assert_eq!(state.regions(), &[rect(100, 100, 51, 51)]);
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn remove_shape_or_cancel_removes_first_then_cancels_on_empty_ground() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::RemoveShapeOrCancel, true));
        drag(&mut state, (10, 10), (60, 60));
        state.handle(middle_press(30, 30));
        assert!(state.regions().is_empty());
        assert_eq!(state.outcome(), None);

        state.handle(middle_press(30, 30));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn capture_fullscreen_and_active_monitor_finish_with_their_outcomes() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::CaptureFullscreen, RegionMouseAction::None, false));
        state.handle(right_press(5, 5));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Fullscreen));

        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::CaptureActiveMonitor, false));
        state.handle(middle_press(5, 5));
        assert_eq!(state.outcome(), Some(&RegionOutcome::ActiveMonitor));
    }

    #[test]
    fn confirm_selection_confirms_like_enter() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::ConfirmSelection, RegionMouseAction::None, true));
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        state.handle(right_press(30, 30));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed {
                rects: vec![rect(10, 10, 51, 51), rect(100, 100, 51, 51)],
                target: None,
                action: SelectionAction::Default
            })
        );

        let win = window("Editor", rect(300, 200, 400, 300));
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::ConfirmSelection, false));
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(move_to(400, 300));
        state.handle(middle_press(400, 300));
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![win.rect], target: Some(win), action: SelectionAction::Default })
        );
    }

    #[test]
    fn a_point_picker_confirms_the_pixel_under_a_click_or_enter() {
        let mut state = RegionState::new(RegionConfig { pick_point: true, ..config() });
        state.handle(RegionInput::PointerMove { x: 40, y: 30 });
        state.handle(press(40, 30));
        let one = |x, y| Some(RegionOutcome::Confirmed { rects: vec![rect(x, y, 1, 1)], target: None, action: SelectionAction::Default });
        assert_eq!(state.outcome().cloned(), one(40, 30));

        let mut state = RegionState::new(RegionConfig { pick_point: true, ..config() });
        state.handle(RegionInput::PointerMove { x: 7, y: 9 });
        state.handle(RegionInput::Key { key: RegionKey::Enter, mods: KeyMods::default() });
        assert_eq!(state.outcome().cloned(), one(7, 9));
    }

    #[test]
    fn toolbar_buttons_ignore_the_target_under_the_toolbar() {
        let win = window("Browser", rect(0, 0, 1920, 1080));
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::None, false));
        state.handle(RegionInput::TargetsLoaded(vec![win]));
        state.handle(RegionInput::PointerMove { x: 960, y: 20 });
        assert!(state.hovered_target().is_some());

        state.handle(RegionInput::Command(RegionCommand::Annotate));
        assert!(state.annotate_requested() && state.outcome().is_none(), "Annotate only arms annotate mode");
        state.handle(RegionInput::Command(RegionCommand::Confirm));
        assert!(state.outcome().is_none(), "Capture with nothing drawn waits for a selection");

        drag(&mut state, (100, 100), (300, 200));
        assert!(state.annotate_requested());
        assert_eq!(
            state.outcome(),
            Some(&RegionOutcome::Confirmed { rects: vec![rect(100, 100, 201, 101)], target: None, action: SelectionAction::Default })
        );
    }

    #[test]
    fn annotate_confirms_and_flags() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::None, false));
        state.handle(RegionInput::Command(RegionCommand::Annotate));
        assert!(state.annotate_requested() && state.outcome().is_none());
        state.handle(RegionInput::Command(RegionCommand::Annotate));
        assert!(!state.annotate_requested());
        state.handle(RegionInput::Command(RegionCommand::Annotate));
        drag(&mut state, (10, 10), (60, 60));
        assert!(state.annotate_requested());
        assert!(matches!(state.outcome(), Some(RegionOutcome::Confirmed { .. })));
    }

    #[test]
    fn confirm_selection_with_nothing_to_confirm_stays_idle() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::ConfirmSelection, false));
        state.handle(middle_press(5, 5));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn a_right_press_during_a_drag_cancels_the_drag_whatever_the_action() {
        let mut state = RegionState::new(with_clicks(RegionMouseAction::CaptureFullscreen, RegionMouseAction::None, false));
        state.handle(press(100, 100));
        state.handle(move_to(150, 150));
        state.handle(right_press(150, 150));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn a_middle_press_during_a_drag_cancels_the_drag() {
        let mut state = RegionState::new(multi());
        state.handle(press(100, 100));
        state.handle(move_to(150, 150));
        state.handle(middle_press(150, 150));
        assert_eq!(state.phase(), &Phase::Idle);
        assert!(state.regions().is_empty());
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn a_middle_press_during_a_move_restores_the_region() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (100, 100), (199, 199));
        state.handle(press(150, 150));
        state.handle(move_to(300, 300));
        assert_eq!(state.active_region(), Some(rect(250, 250, 100, 100)));
        state.handle(middle_press(300, 300));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.active_region(), Some(rect(100, 100, 100, 100)));
        assert_eq!(state.outcome(), None);
    }

    /// A single-region session that keeps the drawn region as an adjustable selection.
    fn selecting() -> RegionConfig {
        RegionConfig { confirm_on_select: false, pick_point: false, ..config() }
    }

    /// A selection drawn from (100, 100) to (300, 200): the rect 100,100 201x101.
    fn selected_state() -> RegionState {
        let mut state = RegionState::new(selecting());
        drag(&mut state, (100, 100), (300, 200));
        state
    }

    const SELECTED: PhysRect = PhysRect { x: 100, y: 100, width: 201, height: 101 };

    fn confirmed(rects: Vec<PhysRect>, target: Option<HoverTarget>, action: SelectionAction) -> RegionOutcome {
        RegionOutcome::Confirmed { rects, target, action }
    }

    #[test]
    fn a_drawn_region_stays_selected_when_the_session_does_not_confirm_on_select() {
        let mut state = RegionState::new(selecting());
        state.handle(press(100, 100));
        state.handle(move_to(300, 200));
        assert_eq!(state.selected(), None, "still drawing");
        state.handle(release(300, 200));

        assert_eq!(state.outcome(), None);
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.regions(), &[SELECTED]);
        assert_eq!(state.active_region(), Some(SELECTED));
        assert_eq!(state.selected(), Some(SELECTED));
        assert_eq!(state.handle_rects().len(), 8, "the handles are there to adjust it");
    }

    #[test]
    fn confirm_on_select_still_ends_the_session_and_holds_no_selection() {
        let mut state = RegionState::new(config());
        drag(&mut state, (100, 100), (300, 200));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Default)));
        assert_eq!(state.selected(), None);
    }

    #[test]
    fn a_clicked_target_stays_selected_with_its_target() {
        let win = window("Editor", rect(300, 200, 400, 300));
        let mut state = RegionState::new(selecting());
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(press(350, 250));
        state.handle(release(350, 250));
        assert_eq!(state.outcome(), None);
        assert_eq!(state.selected(), Some(win.rect));

        state.handle(tap(RegionKey::Enter));
        assert_eq!(state.outcome(), Some(&confirmed(vec![win.rect], Some(win), SelectionAction::Default)));
    }

    #[test]
    fn a_selected_region_can_be_moved_resized_and_nudged() {
        let mut state = selected_state();

        // Drag from inside, away from the handles.
        state.handle(press(150, 150));
        assert!(matches!(state.phase(), Phase::Moving { .. }));
        state.handle(move_to(170, 160));
        state.handle(release(170, 160));
        assert_eq!(state.selected(), Some(rect(120, 110, 201, 101)));

        // Drag the south-east handle, which sits on the last pixel of the rect.
        state.handle(press(320, 210));
        assert!(matches!(state.phase(), Phase::Resizing { handle: Handle::SE, .. }));
        state.handle(move_to(420, 310));
        state.handle(release(420, 310));
        assert_eq!(state.selected(), Some(rect(120, 110, 301, 201)));

        state.handle(tap(RegionKey::Right));
        state.handle(tap_with(RegionKey::Down, true, false));
        assert_eq!(state.selected(), Some(rect(121, 120, 301, 201)));
        assert_eq!(state.outcome(), None, "adjusting never ends the session");
    }

    #[test]
    fn drawing_outside_the_selection_replaces_it() {
        let mut state = selected_state();
        drag(&mut state, (500, 400), (600, 500));
        assert_eq!(state.regions(), &[rect(500, 400, 101, 101)], "a single-region session holds one selection");
        assert_eq!(state.selected(), Some(rect(500, 400, 101, 101)));
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn delete_clears_the_selection_and_enter_then_confirms_nothing() {
        let mut state = selected_state();
        state.handle(tap(RegionKey::Delete));
        assert!(state.regions().is_empty());
        assert_eq!(state.selected(), None);
        state.handle(tap(RegionKey::Enter));
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn enter_and_escape_end_a_selected_session() {
        let mut state = selected_state();
        state.handle(tap(RegionKey::Enter));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Default)));

        let mut state = selected_state();
        state.handle(tap(RegionKey::Escape));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn space_toggles_the_tools_and_asks_for_a_repaint() {
        let mut state = RegionState::new(config());
        assert!(state.tools_visible(), "the tools start shown");
        assert!(state.handle(tap(RegionKey::Space)), "the overlay must repaint to hide them");
        assert!(!state.tools_visible());
        assert!(state.handle(tap(RegionKey::Space)));
        assert!(state.tools_visible());
        assert_eq!(state.outcome(), None, "Space never ends the session");
    }

    #[test]
    fn space_works_with_a_selection_while_drawing_and_while_locked() {
        let mut state = selected_state();
        state.handle(tap(RegionKey::Space));
        assert!(!state.tools_visible());
        assert_eq!(state.selected(), Some(SELECTED), "toggling the tools leaves the selection alone");

        state.handle(RegionInput::Command(RegionCommand::Lock));
        state.handle(tap(RegionKey::Space));
        assert!(state.tools_visible(), "a locked selection can still hide or show the tools");

        let mut drawing = RegionState::new(selecting());
        drawing.handle(press(10, 10));
        drawing.handle(move_to(50, 50));
        drawing.handle(tap(RegionKey::Space));
        assert!(!drawing.tools_visible());
        assert!(matches!(drawing.phase(), Phase::Drawing { .. }), "the draw carries on");
    }

    #[test]
    fn finish_confirms_the_selected_region_with_each_action() {
        for action in [
            SelectionAction::Default,
            SelectionAction::Copy,
            SelectionAction::Save,
            SelectionAction::Pin,
            SelectionAction::Upload,
            SelectionAction::Edit,
            SelectionAction::CopyText,
        ] {
            let mut state = selected_state();
            assert!(state.handle(RegionInput::Command(RegionCommand::Finish(action))));
            assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, action)), "{action:?}");
        }
    }

    #[test]
    fn finish_keeps_the_target_the_region_came_from() {
        let win = window("Editor", rect(300, 200, 400, 300));
        let mut state = RegionState::new(selecting());
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(press(350, 250));
        state.handle(release(350, 250));
        state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Pin)));
        assert_eq!(state.outcome(), Some(&confirmed(vec![win.rect], Some(win), SelectionAction::Pin)));
    }

    #[test]
    fn finish_without_a_region_does_nothing_even_over_a_target() {
        let mut state = RegionState::new(selecting());
        state.handle(RegionInput::TargetsLoaded(vec![window("Browser", rect(0, 0, 1000, 800))]));
        state.handle(move_to(500, 400));
        assert!(state.hovered_target().is_some());

        assert!(!state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Copy))));
        assert_eq!(state.outcome(), None, "the target under the toolbar was never chosen");
        assert_eq!(state.phase(), &Phase::Idle);

        // Once a region is drawn the same command works.
        drag(&mut state, (100, 100), (300, 200));
        state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Copy)));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Copy)));
    }

    #[test]
    fn finish_in_multi_region_confirms_every_region() {
        let mut state = RegionState::new(multi());
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Save)));
        assert_eq!(state.outcome(), Some(&confirmed(vec![rect(10, 10, 51, 51), rect(100, 100, 51, 51)], None, SelectionAction::Save)));
    }

    #[test]
    fn confirm_and_annotate_keep_the_default_action() {
        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Confirm));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Default)));

        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Annotate));
        assert!(state.annotate_requested());
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Default)));
    }

    #[test]
    fn lock_freezes_the_selection_against_pointer_input() {
        let mut state = selected_state();
        assert!(!state.locked());
        assert!(state.handle(RegionInput::Command(RegionCommand::Lock)), "the overlay repaints to show the lock");
        assert!(state.locked());

        // Pressing inside, on a handle and outside start nothing, and dragging changes nothing.
        for start in [(150, 150), (300, 200), (500, 400)] {
            state.handle(press(start.0, start.1));
            assert_eq!(state.phase(), &Phase::Idle, "press at {start:?}");
            state.handle(move_to(start.0 + 40, start.1 + 30));
            state.handle(release(start.0 + 40, start.1 + 30));
            assert_eq!(state.regions(), &[SELECTED], "press at {start:?}");
            assert_eq!(state.phase(), &Phase::Idle);
        }
        assert_eq!(state.outcome(), None);
        assert_eq!(state.cursor(), (540, 430), "the cursor still follows the pointer");

        // The right and middle buttons belong to the annotation tools too.
        state.handle(right_press(150, 150));
        state.handle(middle_press(150, 150));
        assert_eq!(state.outcome(), None);
        assert_eq!(state.regions(), &[SELECTED]);
    }

    #[test]
    fn lock_ignores_delete_arrows_and_tab() {
        let mut state = RegionState::new(RegionConfig { multi_region: true, ..selecting() });
        drag(&mut state, (10, 10), (60, 60));
        drag(&mut state, (100, 100), (150, 150));
        let active = state.active_region();
        state.handle(RegionInput::Command(RegionCommand::Lock));

        for key in [RegionKey::Delete, RegionKey::Left, RegionKey::Right, RegionKey::Up, RegionKey::Down, RegionKey::Tab] {
            state.handle(tap(key));
            state.handle(tap_with(key, true, true));
        }
        assert_eq!(state.regions(), &[rect(10, 10, 51, 51), rect(100, 100, 51, 51)]);
        assert_eq!(state.active_region(), active);
        assert_eq!(state.outcome(), None);
    }

    #[test]
    fn enter_while_locked_finishes_with_the_default_action_and_escape_cancels() {
        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Lock));
        state.handle(tap(RegionKey::Enter));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Default)));

        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Lock));
        state.handle(tap(RegionKey::Escape));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn finish_and_the_other_commands_work_while_locked() {
        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Lock));
        state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Edit)));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Edit)));

        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Lock));
        state.handle(RegionInput::Command(RegionCommand::Cancel));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Cancelled));
    }

    #[test]
    fn unlock_gives_the_selection_back() {
        let mut state = selected_state();
        state.handle(RegionInput::Command(RegionCommand::Lock));
        assert!(state.handle(RegionInput::Command(RegionCommand::Unlock)));
        assert!(!state.locked());

        state.handle(press(150, 150));
        assert!(matches!(state.phase(), Phase::Moving { .. }));
        state.handle(move_to(160, 150));
        state.handle(release(160, 150));
        assert_eq!(state.selected(), Some(rect(110, 100, 201, 101)));
        state.handle(tap(RegionKey::Delete));
        assert!(state.regions().is_empty());
    }

    #[test]
    fn locking_mid_gesture_puts_the_region_back_and_ends_the_gesture() {
        let mut state = selected_state();
        state.handle(press(150, 150));
        state.handle(move_to(200, 180));
        assert_eq!(state.active_region(), Some(rect(150, 130, 201, 101)));

        state.handle(RegionInput::Command(RegionCommand::Lock));
        assert_eq!(state.phase(), &Phase::Idle);
        assert_eq!(state.selected(), Some(SELECTED));
        state.handle(move_to(250, 220));
        state.handle(release(250, 220));
        assert_eq!(state.selected(), Some(SELECTED));
    }

    #[test]
    fn a_locked_state_reports_a_change_only_once() {
        let mut state = selected_state();
        assert!(state.handle(RegionInput::Command(RegionCommand::Lock)));
        assert!(!state.handle(RegionInput::Command(RegionCommand::Lock)), "locking twice changes nothing");
        assert!(state.handle(RegionInput::Command(RegionCommand::Unlock)));
        assert!(!state.handle(RegionInput::Command(RegionCommand::Unlock)));
    }

    fn crop(shade: u8) -> Bitmap {
        Bitmap::new(2, 1, crate::image::PixelFormat::Rgba8, vec![shade; 8].into()).expect("2x1 RGBA is 8 bytes")
    }

    #[test]
    fn an_annotated_crop_changes_nothing_in_the_state() {
        let mut state = selected_state();
        assert!(!state.handle(RegionInput::Annotated(crop(9))), "nothing to repaint");
        assert_eq!(state.selected(), Some(SELECTED));
        assert_eq!(state.outcome(), None);

        state.handle(RegionInput::Command(RegionCommand::Finish(SelectionAction::Copy)));
        assert_eq!(state.outcome(), Some(&confirmed(vec![SELECTED], None, SelectionAction::Copy)));
    }

    #[test]
    fn annotated_inputs_compare_by_pixels() {
        assert_eq!(RegionInput::Annotated(crop(1)), RegionInput::Annotated(crop(1)));
        assert_ne!(RegionInput::Annotated(crop(1)), RegionInput::Annotated(crop(2)));
        assert_ne!(RegionInput::Annotated(crop(1)), RegionInput::Command(RegionCommand::Confirm));
        assert_eq!(press(1, 2), press(1, 2));
        assert_ne!(press(1, 2), release(1, 2));
        assert_ne!(move_to(1, 2), move_to(1, 3));
        assert_eq!(tap(RegionKey::Space), tap(RegionKey::Space));
        assert_ne!(tap(RegionKey::Space), tap_with(RegionKey::Space, true, false));
    }
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn state() -> RegionState {
        RegionState::new(RegionConfig {
            bounds: PhysRect { x: 0, y: 0, width: 100, height: 100 },
            multi_region: false,
            confirm_on_select: true,
            pick_point: false,
            snap_to_targets: true,
            min_size: 2,
            magnifier_pixels: 5,
            info_text: String::new(),
            right_click: Default::default(),
            middle_click: Default::default(),
        })
    }

    #[test]
    fn toolbar_commands_finish() {
        for (cmd, out) in [
            (RegionCommand::Cancel, RegionOutcome::Cancelled),
            (RegionCommand::Fullscreen, RegionOutcome::Fullscreen),
            (RegionCommand::ActiveMonitor, RegionOutcome::ActiveMonitor),
        ] {
            let mut s = state();
            s.handle(RegionInput::Command(cmd));
            assert_eq!(s.outcome(), Some(&out));
        }
        let mut s = state();
        s.handle(RegionInput::Command(RegionCommand::Confirm));
        assert_eq!(s.outcome(), None, "nothing selected yet");
    }
}

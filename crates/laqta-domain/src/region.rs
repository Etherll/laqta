//! Region selection state machine (ADR 012, M1 §4.1). Pure: no I/O, clock, env or threads.
//!
//! Rects are half-open: a rect covers `x..right()` and `y..bottom()`. Pointer positions are
//! clamped to the last pixel inside `bounds`.

use crate::capture::RegionMouseAction;
use crate::geometry::PhysRect;

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
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub struct KeyMods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum RegionInput {
    PointerMove { x: i32, y: i32 },
    PointerDown { x: i32, y: i32, button: Button },
    PointerUp { x: i32, y: i32, button: Button },
    Key { key: RegionKey, mods: KeyMods },
    TargetsLoaded(Vec<HoverTarget>),
    /// A button on the overlay toolbar.
    Command(RegionCommand),
}

/// The overlay toolbar's buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegionCommand {
    Confirm,
    Cancel,
    Fullscreen,
    ActiveMonitor,
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
    /// Hover targets, front to back.
    targets: Vec<HoverTarget>,
    regions: Vec<PhysRect>,
    /// The target each region was confirmed from, if any, parallel to `regions`.
    region_targets: Vec<Option<HoverTarget>>,
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
}

impl RegionState {
    pub fn new(config: RegionConfig) -> Self {
        let cursor = centre(config.bounds);
        Self {
            config,
            targets: Vec::new(),
            regions: Vec::new(),
            region_targets: Vec::new(),
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
        View { cursor: self.cursor, phase: self.phase.clone(), regions: self.regions.clone(), active: self.active, hover: self.hover }
    }

    fn step(&mut self, input: RegionInput) {
        match input {
            RegionInput::PointerMove { x, y } => self.pointer_move((x, y)),
            RegionInput::PointerDown { x, y, button } => self.pointer_down((x, y), button),
            RegionInput::PointerUp { x, y, button } => self.pointer_up((x, y), button),
            RegionInput::Key { key, mods } => self.key(key, mods),
            RegionInput::Command(RegionCommand::Confirm) => self.confirm(),
            RegionInput::Command(RegionCommand::Cancel) => self.finish(RegionOutcome::Cancelled),
            RegionInput::Command(RegionCommand::Fullscreen) => self.finish(RegionOutcome::Fullscreen),
            RegionInput::Command(RegionCommand::ActiveMonitor) => self.finish(RegionOutcome::ActiveMonitor),
            // `handle` routes this variant to `load_targets`.
            RegionInput::TargetsLoaded(_) => {}
        }
    }

    /// Replaces the target list. Returns true if the list changed.
    fn load_targets(&mut self, targets: Vec<HoverTarget>) -> bool {
        let changed = self.targets != targets;
        if changed {
            self.targets = targets;
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

    fn update_hover(&mut self) {
        self.hover = if self.config.snap_to_targets { self.targets.iter().position(|t| contains(t.rect, self.cursor)) } else { None };
    }

    fn set_active(&mut self, rect: PhysRect) {
        if let Some(i) = self.active {
            self.regions[i] = rect;
        }
    }

    fn finish(&mut self, outcome: RegionOutcome) {
        self.phase = Phase::Finished(outcome);
    }

    fn enter_idle(&mut self) {
        self.phase = Phase::Idle;
        self.update_hover();
    }

    fn pointer_move(&mut self, at: (i32, i32)) {
        self.set_cursor(at);
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
        if button != Button::Left {
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
        self.regions.push(rect);
        self.region_targets.push(target.clone());
        self.active = Some(self.regions.len() - 1);
        if self.config.multi_region {
            self.enter_idle();
        } else {
            self.finish(RegionOutcome::Confirmed { rects: vec![rect], target });
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
            RegionKey::Enter => self.confirm(),
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
            RegionKey::Enter | RegionKey::Escape => {}
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

    fn confirm(&mut self) {
        let bounds = self.config.bounds;
        if !self.regions.is_empty() {
            let outcome = if self.config.multi_region {
                RegionOutcome::Confirmed { rects: self.regions.clone(), target: None }
            } else {
                let i = self.active.unwrap_or(0);
                RegionOutcome::Confirmed { rects: vec![self.regions[i]], target: self.region_targets[i].clone() }
            };
            self.finish(outcome);
        } else if let Some(target) = self.hovered_target().cloned() {
            let rect = clamp_rect(target.rect, bounds);
            self.finish(RegionOutcome::Confirmed { rects: vec![rect], target: Some(target) });
        }
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
        assert_eq!(state.outcome(), Some(&RegionOutcome::Confirmed { rects: vec![front.rect], target: Some(front.clone()) }));
    }

    #[test]
    fn press_and_release_without_a_move_clicks_the_target() {
        let mut state = RegionState::new(config());
        let win = window("Editor", rect(300, 200, 400, 300));
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(press(350, 250));
        state.handle(release(350, 250));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Confirmed { rects: vec![win.rect], target: Some(win) }));
    }

    #[test]
    fn drag_draws_a_normalised_rect() {
        let mut state = RegionState::new(config());
        state.handle(press(100, 100));
        assert!(state.handle(move_to(200, 150)));
        assert_eq!(state.drawing_rect(), Some(rect(100, 100, 101, 51)));
        state.handle(release(200, 150));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Confirmed { rects: vec![rect(100, 100, 101, 51)], target: None }));

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
            Some(&RegionOutcome::Confirmed { rects: vec![rect(10, 10, 51, 51), rect(100, 100, 51, 51)], target: None })
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
            Some(&RegionOutcome::Confirmed { rects: vec![rect(10, 10, 51, 51), rect(100, 100, 51, 51)], target: None })
        );

        let win = window("Editor", rect(300, 200, 400, 300));
        let mut state = RegionState::new(with_clicks(RegionMouseAction::None, RegionMouseAction::ConfirmSelection, false));
        state.handle(RegionInput::TargetsLoaded(vec![win.clone()]));
        state.handle(move_to(400, 300));
        state.handle(middle_press(400, 300));
        assert_eq!(state.outcome(), Some(&RegionOutcome::Confirmed { rects: vec![win.rect], target: Some(win) }));
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
}

#[cfg(test)]
mod command_tests {
    use super::*;

    fn state() -> RegionState {
        RegionState::new(RegionConfig {
            bounds: PhysRect { x: 0, y: 0, width: 100, height: 100 },
            multi_region: false,
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

//! Which monitor a window goes on, and where on it: the capture overlay's windows and chrome, and the
//! recording border. The GPUI code converts its display list to [Screen]s or [Rect]s and asks here.
//! Toasts, popups and the recording bar still place themselves (`placement::main_display`,
//! `recording_bar::popup_on`).
//!
//! Two coordinate spaces meet here. Frames, selections, cursor positions and recording areas are
//! physical pixels in the virtual desktop ([PhysRect]). Screens are laid out by GPUI in logical
//! pixels: Windows and X11 put a display at its physical origin divided by its own scale, so a
//! screen covers [Screen::physical] in the physical space.

use laqta_domain::geometry::PhysRect;

pub use super::capture_toolbar::Rect;

/// A display as GPUI reports it, in logical pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Screen {
    pub id: u64,
    pub bounds: Rect,
    /// The part of the display not covered by a taskbar or panel; equals [Screen::bounds] where
    /// the platform reports none.
    pub work: Rect,
    pub scale: f32,
}

impl Screen {
    fn scale(&self) -> f32 {
        if self.scale.is_finite() { self.scale.max(0.1) } else { 1.0 }
    }

    /// The display's bounds in physical pixels: its logical bounds times its own scale.
    fn physical(&self) -> Rect {
        let s = self.scale();
        Rect { x: self.bounds.x * s, y: self.bounds.y * s, w: self.bounds.w * s, h: self.bounds.h * s }
    }

    /// [Screen::physical], rounded to whole pixels as (left, top, right, bottom).
    fn extent(&self) -> Extent {
        let p = self.physical();
        let (x, y) = (p.x.round() as i64, p.y.round() as i64);
        Extent { x0: x, y0: y, x1: x + p.w.round() as i64, y1: y + p.h.round() as i64 }
    }
}

/// A rectangle as edges, in whole pixels; wide enough that no sum overflows.
#[derive(Clone, Copy, Debug)]
struct Extent {
    x0: i64,
    y0: i64,
    x1: i64,
    y1: i64,
}

impl Extent {
    fn of(r: PhysRect) -> Extent {
        let (x, y) = (i64::from(r.x), i64::from(r.y));
        Extent { x0: x, y0: y, x1: x + i64::from(r.width), y1: y + i64::from(r.height) }
    }

    fn contains(self, x: i64, y: i64) -> bool {
        (self.x0..self.x1).contains(&x) && (self.y0..self.y1).contains(&y)
    }

    fn overlap(self, other: Extent) -> i64 {
        let w = self.x1.min(other.x1) - self.x0.max(other.x0);
        let h = self.y1.min(other.y1) - self.y0.max(other.y0);
        if w > 0 && h > 0 { w * h } else { 0 }
    }
}

/// One frame (one monitor's capture) and the display its window goes on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Slot {
    /// The frame's index in the list given to [pair].
    pub frame: usize,
    /// The display the frame's window opens on; [None] when no display matches the frame.
    pub screen: Option<u64>,
    /// Where the frame's window goes, in the logical pixels of the display. The display's own
    /// bounds, or the frame's rect over the display's scale when several frames share one
    /// display. Without a display, the frame's physical rect as is.
    pub logical: Rect,
    /// The display's scale; 1.0 without a display.
    pub scale: f32,
    /// The frame, in physical pixels of the virtual desktop. The selection rules of
    /// [chrome_owner] read it.
    pub phys: PhysRect,
}

/// Pairs each frame with a display, one slot per frame in the frames' order. A frame with no
/// display keeps its slot with [Slot::screen] [None], so a frame is never dropped and slot
/// indices stay frame indices.
///
/// Windows and X11 put a display at its physical origin divided by its scale, so a frame's origin
/// names its display exactly, whatever the mix of scales. Where that doesn't hold for every frame
/// (Wayland lays displays out itself), frames and displays are both sorted by position (left to
/// right, then top to bottom) and paired in turn, when there are as many of each. Failing that
/// (an X11 screen that spans several monitors), each frame takes the display it overlaps most,
/// and several frames may share one.
pub fn pair(frames: &[PhysRect], screens: &[Screen]) -> Vec<Slot> {
    let assigned = assign(frames, screens);
    let mut uses = vec![0usize; screens.len()];
    for &i in assigned.iter().flatten() {
        uses[i] += 1;
    }
    frames
        .iter()
        .zip(&assigned)
        .enumerate()
        .map(|(frame, (&phys, &screen))| match screen {
            Some(i) => {
                let s = &screens[i];
                let logical = if uses[i] == 1 { s.bounds } else { logical_of(phys, s.scale()) };
                Slot { frame, screen: Some(s.id), logical, scale: s.scale(), phys }
            }
            None => Slot { frame, screen: None, logical: logical_of(phys, 1.0), scale: 1.0, phys },
        })
        .collect()
}

/// The index into `screens` each frame goes on.
fn assign(frames: &[PhysRect], screens: &[Screen]) -> Vec<Option<usize>> {
    let exact: Vec<Option<usize>> = frames
        .iter()
        .map(|f| {
            screens.iter().position(|s| {
                let scale = s.scale();
                (f.x as f32 / scale - s.bounds.x).abs() < 1.0 && (f.y as f32 / scale - s.bounds.y).abs() < 1.0
            })
        })
        .collect();
    let distinct = exact.iter().enumerate().all(|(i, a)| a.is_none() || exact[..i].iter().all(|b| b != a));
    if exact.iter().all(Option::is_some) && distinct {
        return exact;
    }
    if !frames.is_empty() && frames.len() == screens.len() {
        let mut by_frame: Vec<usize> = (0..frames.len()).collect();
        by_frame.sort_by_key(|&i| (frames[i].x, frames[i].y));
        let mut by_screen: Vec<usize> = (0..screens.len()).collect();
        by_screen.sort_by(|&a, &b| {
            screens[a].bounds.x.total_cmp(&screens[b].bounds.x).then(screens[a].bounds.y.total_cmp(&screens[b].bounds.y))
        });
        let mut out = vec![None; frames.len()];
        for (frame, screen) in by_frame.into_iter().zip(by_screen) {
            out[frame] = Some(screen);
        }
        return out;
    }
    frames.iter().map(|&f| most_overlap(screens, f)).collect()
}

/// The index of the screen `rect` overlaps most, the first of equals; none when it overlaps none.
fn most_overlap(screens: &[Screen], rect: PhysRect) -> Option<usize> {
    let r = Extent::of(rect);
    let mut best: Option<(usize, i64)> = None;
    for (i, s) in screens.iter().enumerate() {
        let area = r.overlap(s.extent());
        if area > 0 && best.is_none_or(|(_, a)| area > a) {
            best = Some((i, area));
        }
    }
    best.map(|(i, _)| i)
}

fn logical_of(phys: PhysRect, scale: f32) -> Rect {
    Rect { x: phys.x as f32 / scale, y: phys.y as f32 / scale, w: phys.width as f32 / scale, h: phys.height as f32 / scale }
}

/// The slot (an index into `slots`) that carries the capture chrome: the hint, the toolbar and
/// the mode bar. In order: the monitor holding the selection's centre, the monitor with most of
/// the selection, the monitor under the cursor, then the first. A slot without a display has no
/// window to carry chrome, so it is passed over while any slot has one.
pub fn chrome_owner(slots: &[Slot], cursor: (i32, i32), selection: Option<PhysRect>) -> usize {
    let with_screen: Vec<usize> = (0..slots.len()).filter(|&i| slots[i].screen.is_some()).collect();
    let usable = if with_screen.is_empty() { (0..slots.len()).collect() } else { with_screen };
    let holding = |x: i64, y: i64| usable.iter().copied().find(|&i| Extent::of(slots[i].phys).contains(x, y));
    if let Some(sel) = selection {
        let centre = (i64::from(sel.x) + i64::from(sel.width) / 2, i64::from(sel.y) + i64::from(sel.height) / 2);
        if let Some(i) = holding(centre.0, centre.1) {
            return i;
        }
        let area = Extent::of(sel);
        let mut best: Option<(usize, i64)> = None;
        for &i in &usable {
            let a = area.overlap(Extent::of(slots[i].phys));
            if a > 0 && best.is_none_or(|(_, b)| a > b) {
                best = Some((i, a));
            }
        }
        if let Some((i, _)) = best {
            return i;
        }
    }
    holding(i64::from(cursor.0), i64::from(cursor.1)).or(usable.first().copied()).unwrap_or(0)
}

/// Which side of a recording area a border strip runs along.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Side {
    Top,
    Bottom,
    Left,
    Right,
}

/// One window of the recording border.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Strip {
    pub side: Side,
    pub rect: Rect,
}

/// The windows of a `thickness`-wide border just outside a recording `area`, so the border is never
/// part of the recording. Everything is in one logical space: `area` and `screens` alike.
///
/// The top and bottom strips span the area's width and both side strips. `tall` makes them that
/// much taller, the extra away from the area (Windows makes no window shorter than a title bar);
/// near the top of its screen the top strip starts at the screen's edge and hangs over the area
/// instead. `slack` moves the left strip that far further out (Windows rounds a narrow window up).
///
/// A strip whose line starts on no screen is left out, as shifting it would put it over the area.
/// One on a neighbouring screen stays: an area that starts where two monitors meet keeps its left
/// side on the other monitor.
pub fn recording_border(area: Rect, thickness: f32, tall: f32, slack: f32, screens: &[Rect]) -> Vec<Strip> {
    let t = thickness;
    let Rect { x, y, w, h } = area;
    let holding = |px: f32, py: f32| screens.iter().find(|s| px >= s.x && px < s.right() && py >= s.y && py < s.bottom());
    let mut strips = Vec::with_capacity(4);
    if let Some(screen) = holding(x - t, y - t) {
        let top = (y - t - tall).max(screen.y).min(y - t);
        strips.push(Strip { side: Side::Top, rect: Rect { x: x - t, y: top, w: w + 2.0 * t, h: t + tall } });
    }
    if holding(x - t, y + h).is_some() {
        strips.push(Strip { side: Side::Bottom, rect: Rect { x: x - t, y: y + h, w: w + 2.0 * t, h: t + tall } });
    }
    if holding(x - t - slack, y).is_some() {
        strips.push(Strip { side: Side::Left, rect: Rect { x: x - t - slack, y, w: t, h } });
    }
    if holding(x + w, y).is_some() {
        strips.push(Strip { side: Side::Right, rect: Rect { x: x + w, y, w: t, h } });
    }
    strips
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect { x, y, w, h }
    }

    fn phys(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    fn screen(id: u64, bounds: Rect, scale: f32) -> Screen {
        Screen { id, bounds, work: bounds, scale }
    }

    /// 150% primary (2880x1620 physical) with a 100% monitor to its right. GPUI puts a display at
    /// its physical origin over its own scale, so the second one starts at 2880 logical.
    fn mixed() -> (Vec<PhysRect>, Vec<Screen>) {
        (
            vec![phys(0, 0, 2880, 1620), phys(2880, 0, 1920, 1080)],
            vec![screen(10, rect(0.0, 0.0, 1920.0, 1080.0), 1.5), screen(20, rect(2880.0, 0.0, 1920.0, 1080.0), 1.0)],
        )
    }

    /// A 100% monitor left of the primary: the desktop starts at x = -1920.
    fn left_of_primary() -> (Vec<PhysRect>, Vec<Screen>) {
        (
            vec![phys(0, 0, 1920, 1080), phys(-1920, 0, 1920, 1080)],
            vec![screen(1, rect(0.0, 0.0, 1920.0, 1080.0), 1.0), screen(2, rect(-1920.0, 0.0, 1920.0, 1080.0), 1.0)],
        )
    }

    #[test]
    fn pair_names_each_display_exactly_across_mixed_scales() {
        let (frames, screens) = mixed();
        let slots = pair(&frames, &screens);
        assert_eq!(slots.len(), 2);
        assert_eq!((slots[0].frame, slots[0].screen, slots[0].scale), (0, Some(10), 1.5));
        assert_eq!(slots[0].logical, rect(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!((slots[1].frame, slots[1].screen, slots[1].scale), (1, Some(20), 1.0));
        assert_eq!(slots[1].logical, rect(2880.0, 0.0, 1920.0, 1080.0));
        assert_eq!(slots[1].phys, frames[1]);
    }

    #[test]
    fn pair_is_independent_of_the_order_of_either_list() {
        let (frames, mut screens) = mixed();
        screens.reverse();
        let reversed = [frames[1], frames[0]];
        let slots = pair(&reversed, &screens);
        assert_eq!(slots[0].screen, Some(20));
        assert_eq!(slots[1].screen, Some(10));
    }

    #[test]
    fn pair_handles_a_monitor_left_of_the_primary() {
        let (frames, screens) = left_of_primary();
        let slots = pair(&frames, &screens);
        assert_eq!(slots[0].screen, Some(1));
        assert_eq!(slots[1].screen, Some(2));
        assert_eq!(slots[1].logical.x, -1920.0);
        // The same at 150%: -2880 physical is -1920 logical.
        let frames = [phys(0, 0, 1920, 1080), phys(-2880, 0, 2880, 1620)];
        let screens = [screen(1, rect(0.0, 0.0, 1920.0, 1080.0), 1.0), screen(2, rect(-1920.0, 0.0, 1920.0, 1080.0), 1.5)];
        let slots = pair(&frames, &screens);
        assert_eq!((slots[1].screen, slots[1].scale), (Some(2), 1.5));
    }

    #[test]
    fn pair_falls_back_to_position_order_where_origins_do_not_line_up() {
        // Wayland: fractional scale, so no origin matches, but there is a display per frame.
        let frames = [phys(2400, 0, 2400, 1350), phys(0, 0, 2400, 1350)];
        let screens = [screen(7, rect(2000.0, 0.0, 2000.0, 1125.0), 1.0), screen(8, rect(0.0, 0.0, 2000.0, 1125.0), 1.0)];
        let slots = pair(&frames, &screens);
        assert_eq!(slots[0].screen, Some(7));
        assert_eq!(slots[1].screen, Some(8));
        assert_eq!(slots[0].logical, screens[0].bounds);
    }

    #[test]
    fn pair_shares_a_single_x11_screen_between_two_monitors() {
        let frames = [phys(0, 0, 1920, 1080), phys(1920, 0, 1920, 1080)];
        let screens = [screen(1, rect(0.0, 0.0, 3840.0, 1080.0), 1.0)];
        let slots = pair(&frames, &screens);
        assert_eq!(slots.len(), 2);
        assert_eq!((slots[0].screen, slots[1].screen), (Some(1), Some(1)));
        assert_eq!(slots[0].logical, rect(0.0, 0.0, 1920.0, 1080.0));
        assert_eq!(slots[1].logical, rect(1920.0, 0.0, 1920.0, 1080.0));
    }

    #[test]
    fn pair_keeps_an_empty_slot_for_a_frame_with_no_screen() {
        let (frames, screens) = mixed();
        // Only the first display is known: the second frame keeps its slot.
        let slots = pair(&frames, &screens[..1]);
        assert_eq!(slots.len(), 2);
        assert_eq!(slots[0].screen, Some(10));
        assert_eq!(slots[1].screen, None);
        assert_eq!(slots[1].frame, 1);
        assert_eq!(slots[1].scale, 1.0);
        assert_eq!(slots[1].logical, rect(2880.0, 0.0, 1920.0, 1080.0));
        // No displays at all: a slot per frame still.
        let none = pair(&frames, &[]);
        assert_eq!(none.len(), 2);
        assert!(none.iter().all(|s| s.screen.is_none()));
        assert!(pair(&[], &screens).is_empty());
    }

    fn two_slots() -> Vec<Slot> {
        // Two monitors with a gap between them: A is 0..1000, B is 1100..2100.
        let frames = [phys(0, 0, 1000, 800), phys(1100, 0, 1000, 800)];
        let screens = [screen(1, rect(0.0, 0.0, 1000.0, 800.0), 1.0), screen(2, rect(1100.0, 0.0, 1000.0, 800.0), 1.0)];
        pair(&frames, &screens)
    }

    #[test]
    fn chrome_owner_prefers_the_monitor_holding_the_selection_centre() {
        let slots = two_slots();
        // Most of it is on A, but the centre (1150) is on B.
        let sel = phys(400, 100, 1500, 200);
        assert_eq!(chrome_owner(&slots, (10, 10), Some(sel)), 1);
        // The cursor is elsewhere and doesn't matter.
        assert_eq!(chrome_owner(&slots, (2000, 10), Some(phys(100, 100, 200, 200))), 0);
    }

    #[test]
    fn chrome_owner_takes_most_overlap_when_the_centre_is_between_monitors() {
        let slots = two_slots();
        // 700..1300: the centre (1000) is in the gap; 300 px on A, 200 on B.
        let sel = phys(700, 100, 600, 200);
        assert_eq!(chrome_owner(&slots, (1500, 10), Some(sel)), 0);
        // 800..1700: the centre (1250) is on B.
        let sel = phys(800, 100, 900, 200);
        assert_eq!(chrome_owner(&slots, (10, 10), Some(sel)), 1);
        // 900..1400: the centre (1150) is on B; 100 px on A, 300 on B.
        let sel = phys(900, 100, 500, 200);
        assert_eq!(chrome_owner(&slots, (10, 10), Some(sel)), 1);
    }

    #[test]
    fn chrome_owner_falls_back_to_the_cursor_then_the_first() {
        let slots = two_slots();
        assert_eq!(chrome_owner(&slots, (1500, 300), None), 1);
        assert_eq!(chrome_owner(&slots, (500, 300), None), 0);
        // A selection off every monitor says nothing: the cursor decides.
        assert_eq!(chrome_owner(&slots, (1500, 300), Some(phys(5000, 5000, 100, 100))), 1);
        // The cursor is off every monitor too.
        assert_eq!(chrome_owner(&slots, (-500, 9000), None), 0);
        assert_eq!(chrome_owner(&[], (0, 0), None), 0);
    }

    #[test]
    fn chrome_owner_passes_over_a_slot_without_a_screen() {
        let frames = [phys(0, 0, 1000, 800), phys(1100, 0, 1000, 800)];
        let screens = [screen(2, rect(1100.0, 0.0, 1000.0, 800.0), 1.0)];
        let slots = pair(&frames, &screens);
        assert_eq!(slots[0].screen, None);
        assert_eq!(chrome_owner(&slots, (10, 10), Some(phys(100, 100, 200, 200))), 1);
        assert_eq!(chrome_owner(&slots, (10, 10), None), 1);
    }

    /// Two monitors side by side, logical pixels.
    fn side_by_side() -> Vec<Rect> {
        vec![rect(0.0, 0.0, 1920.0, 1080.0), rect(1920.0, 0.0, 1920.0, 1080.0)]
    }

    fn sides(strips: &[Strip]) -> Vec<Side> {
        strips.iter().map(|s| s.side).collect()
    }

    #[test]
    fn a_border_has_four_strips_just_outside_the_area() {
        let strips = recording_border(rect(100.0, 200.0, 400.0, 300.0), 3.0, 0.0, 0.0, &side_by_side());
        assert_eq!(
            strips,
            vec![
                Strip { side: Side::Top, rect: rect(97.0, 197.0, 406.0, 3.0) },
                Strip { side: Side::Bottom, rect: rect(97.0, 500.0, 406.0, 3.0) },
                Strip { side: Side::Left, rect: rect(97.0, 200.0, 3.0, 300.0) },
                Strip { side: Side::Right, rect: rect(500.0, 200.0, 3.0, 300.0) },
            ]
        );
    }

    /// The review's case: an area whose left edge is where two monitors meet keeps its left side,
    /// which lies on the left monitor.
    #[test]
    fn an_area_at_the_seam_of_two_monitors_keeps_its_left_side() {
        let strips = recording_border(rect(1920.0, 200.0, 400.0, 300.0), 3.0, 0.0, 1.0, &side_by_side());
        assert_eq!(sides(&strips), vec![Side::Top, Side::Bottom, Side::Left, Side::Right]);
        assert_eq!(strips[2].rect, rect(1916.0, 200.0, 3.0, 300.0));
    }

    #[test]
    fn a_side_off_every_screen_is_left_out() {
        // Against the desktop's left and top edges.
        let strips = recording_border(rect(0.0, 0.0, 400.0, 300.0), 3.0, 0.0, 0.0, &side_by_side());
        assert_eq!(sides(&strips), vec![Side::Right]);
        // A monitor left of the primary: negative coordinates are a screen like any other.
        let screens = [rect(-1920.0, 0.0, 1920.0, 1080.0), rect(0.0, 0.0, 1920.0, 1080.0)];
        let strips = recording_border(rect(0.0, 100.0, 400.0, 300.0), 3.0, 0.0, 0.0, &screens);
        assert_eq!(sides(&strips), vec![Side::Top, Side::Bottom, Side::Left, Side::Right]);
        assert_eq!(strips[2].rect.x, -3.0);
    }

    #[test]
    fn a_tall_top_strip_stops_at_the_top_of_its_screen() {
        // Room above: the strip ends at the area with its extra above.
        let strips = recording_border(rect(100.0, 200.0, 400.0, 300.0), 3.0, 37.0, 0.0, &side_by_side());
        assert_eq!(strips[0].rect, rect(97.0, 160.0, 406.0, 40.0));
        assert_eq!(strips[1].rect, rect(97.0, 500.0, 406.0, 40.0));
        // Ten pixels from the top: the strip starts at the screen's edge and hangs over the area.
        let strips = recording_border(rect(100.0, 10.0, 400.0, 300.0), 3.0, 37.0, 0.0, &side_by_side());
        assert_eq!(strips[0].rect, rect(97.0, 0.0, 406.0, 40.0));
    }
}

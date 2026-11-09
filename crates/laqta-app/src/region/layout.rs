//! Regions inside windows for the hover (docs/plan/LAYOUT.md). The pixel analysis runs per
//! window on the compute pool; this module merges its regions with the hover source's targets.

use laqta_domain::PhysRect;
use laqta_domain::region::{HoverTarget, TargetKind};

/// A pixel region this close (IoU) to an accessibility element is the same thing: the element
/// is kept, since it has a name.
const SAME_IOU: f64 = 0.85;

/// The windows worth analysing: every non-element target, front to back, with its rect in
/// composed-image coordinates (`bounds` is the composed image's desktop rect).
pub(crate) fn windows(targets: &[HoverTarget], bounds: PhysRect) -> Vec<(usize, PhysRect)> {
    targets
        .iter()
        .enumerate()
        .filter(|(_, t)| t.kind == TargetKind::Window)
        .map(|(i, t)| (i, PhysRect { x: t.rect.x - bounds.x, y: t.rect.y - bounds.y, ..t.rect }))
        .collect()
}

/// Rebuilds the target list with the pixel `regions` found per window (`(target index, rects in
/// desktop coordinates)`). Each window is preceded by its elements, smallest first, so the
/// smallest under the cursor wins and the wheel grows outward to the window. A pixel region is
/// dropped when a window in front overlaps it, or when an element already covers it.
pub(crate) fn merge(targets: Vec<HoverTarget>, regions: &[(usize, Vec<PhysRect>)]) -> Vec<HoverTarget> {
    let owners: Vec<usize> = targets.iter().enumerate().filter(|(_, t)| t.kind != TargetKind::Element).map(|(i, _)| i).collect();
    // Each element belongs to the frontmost window holding its centre.
    let mut groups: Vec<Vec<HoverTarget>> = vec![Vec::new(); targets.len()];
    let mut loose = Vec::new();
    for t in targets.iter().filter(|t| t.kind == TargetKind::Element) {
        let c = centre(t.rect);
        match owners.iter().find(|&&w| contains(targets[w].rect, c)) {
            Some(&w) => groups[w].push(t.clone()),
            None => loose.push(t.clone()),
        }
    }
    for (w, rects) in regions {
        let Some(window) = targets.get(*w) else { continue };
        let front: Vec<PhysRect> = owners.iter().take_while(|&&o| o != *w).map(|&o| targets[o].rect).collect();
        for &rect in rects {
            if front.iter().any(|f| overlaps(*f, rect)) || groups[*w].iter().any(|e| iou(e.rect, rect) >= SAME_IOU) {
                continue;
            }
            let element = HoverTarget { rect, kind: TargetKind::Element, title: String::new(), ..window.clone() };
            groups[*w].push(element);
        }
    }
    let mut out = loose;
    for &w in &owners {
        let mut group = std::mem::take(&mut groups[w]);
        group.sort_by_key(|t| u64::from(t.rect.width) * u64::from(t.rect.height));
        out.extend(group);
        out.push(targets[w].clone());
    }
    out
}

fn centre(r: PhysRect) -> (i32, i32) {
    (r.x.saturating_add_unsigned(r.width / 2), r.y.saturating_add_unsigned(r.height / 2))
}

fn contains(r: PhysRect, (x, y): (i32, i32)) -> bool {
    x >= r.x && y >= r.y && i64::from(x) < i64::from(r.x) + i64::from(r.width) && i64::from(y) < i64::from(r.y) + i64::from(r.height)
}

fn intersection(a: PhysRect, b: PhysRect) -> u64 {
    let x0 = i64::from(a.x.max(b.x));
    let y0 = i64::from(a.y.max(b.y));
    let x1 = (i64::from(a.x) + i64::from(a.width)).min(i64::from(b.x) + i64::from(b.width));
    let y1 = (i64::from(a.y) + i64::from(a.height)).min(i64::from(b.y) + i64::from(b.height));
    if x1 <= x0 || y1 <= y0 { 0 } else { ((x1 - x0) * (y1 - y0)) as u64 }
}

fn overlaps(a: PhysRect, b: PhysRect) -> bool {
    intersection(a, b) > 0
}

fn iou(a: PhysRect, b: PhysRect) -> f64 {
    let i = intersection(a, b) as f64;
    let area = |r: PhysRect| f64::from(r.width) * f64::from(r.height);
    let union = area(a) + area(b) - i;
    if union <= 0.0 { 0.0 } else { i / union }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    fn target(kind: TargetKind, title: &str, r: PhysRect) -> HoverTarget {
        HoverTarget { rect: r, handle: 7, title: title.to_owned(), process: "app".to_owned(), kind }
    }

    #[test]
    fn regions_go_before_their_window_smallest_first() {
        let front = target(TargetKind::Window, "Front", rect(100, 100, 400, 300));
        let back = target(TargetKind::Window, "Back", rect(0, 0, 1000, 800));
        let merged = merge(vec![front.clone(), back.clone()], &[(0, vec![rect(100, 100, 100, 300), rect(120, 120, 40, 20)])]);
        let rects: Vec<PhysRect> = merged.iter().map(|t| t.rect).collect();
        assert_eq!(rects, vec![rect(120, 120, 40, 20), rect(100, 100, 100, 300), front.rect, back.rect]);
        assert_eq!(merged[0].kind, TargetKind::Element);
        assert_eq!(merged[0].handle, 7, "a region keeps its window's handle");
    }

    #[test]
    fn regions_under_a_front_window_are_dropped() {
        let front = target(TargetKind::Window, "Front", rect(100, 100, 400, 300));
        let back = target(TargetKind::Window, "Back", rect(0, 0, 1000, 800));
        let hidden = rect(450, 150, 100, 50);
        let shown = rect(600, 500, 100, 50);
        let merged = merge(vec![front, back], &[(1, vec![hidden, shown])]);
        assert!(merged.iter().all(|t| t.rect != hidden));
        assert!(merged.iter().any(|t| t.rect == shown));
    }

    #[test]
    fn an_accessibility_element_wins_over_the_same_pixel_region() {
        let button = target(TargetKind::Element, "Send", rect(300, 300, 80, 30));
        let window = target(TargetKind::Window, "Chat", rect(0, 0, 1000, 800));
        let merged = merge(vec![button.clone(), window.clone()], &[(1, vec![rect(301, 300, 80, 31), rect(0, 0, 200, 800)])]);
        assert_eq!(
            merged,
            vec![
                button,
                HoverTarget { rect: rect(0, 0, 200, 800), kind: TargetKind::Element, title: String::new(), ..window.clone() },
                window
            ]
        );
    }

    #[test]
    fn window_rects_are_given_in_image_coordinates() {
        let targets = vec![target(TargetKind::Element, "", rect(0, 0, 5, 5)), target(TargetKind::Window, "W", rect(-1920, 100, 400, 300))];
        assert_eq!(windows(&targets, rect(-1920, 0, 3840, 1080)), vec![(1, rect(0, 100, 400, 300))]);
    }
}

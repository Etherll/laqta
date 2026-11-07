//! Family O1: PP-OCR, a DBNet detector and a CTC recogniser.
//!
//! Everything here is pure: no ONNX Runtime, no threads. The inference steps live in `ocr.rs`.
//! Settings follow PaddleOCR's defaults.

use std::collections::HashMap;

use laqta_domain::PhysRect;

use crate::tensor::{BgrImage, HALF, IMAGENET, clip_rect, resample_nchw};

/// The detector's longest side, in pixels.
pub const DET_LIMIT_SIDE: u32 = 960;
/// The detector needs both sides to be multiples of this.
pub const DET_MULTIPLE: u32 = 32;
/// Probability above which a pixel counts as text.
pub const DET_THRESHOLD: f32 = 0.3;
/// Mean probability a region needs to be kept.
pub const DET_BOX_THRESHOLD: f32 = 0.6;
/// How far a detected region grows: DBNet's unclip ratio.
pub const DET_UNCLIP_RATIO: f32 = 1.5;
/// Smallest region kept, as a bounding-box area in probability-map pixels.
pub const DET_MIN_AREA: usize = 16;
/// The recogniser's input height.
pub const REC_HEIGHT: u32 = 48;
/// The recogniser input width. Every crop is padded to it, as RapidOCR does. A crop wider than this
/// after scaling is squashed to it. RapidOCR instead grows the input width with the crop, so a long
/// screenshot line loses detail here. This is a known gap; see the report.
pub const REC_MAX_WIDTH: u32 = 320;

/// The detector input size: the longest side is at most `DET_LIMIT_SIDE`, and each side is rounded
/// to the nearest multiple of `DET_MULTIPLE` (at least one multiple).
pub fn det_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let ratio = if longest > DET_LIMIT_SIDE { DET_LIMIT_SIDE as f32 / longest as f32 } else { 1.0 };
    (round_side(width, ratio), round_side(height, ratio))
}

fn round_side(side: u32, ratio: f32) -> u32 {
    let steps = (side as f32 * ratio / DET_MULTIPLE as f32).round() as u32;
    (steps * DET_MULTIPLE).max(DET_MULTIPLE)
}

/// The detector input for a whole image: `(width, height, NCHW data)`.
pub fn det_input(image: &BgrImage) -> (u32, u32, Vec<f32>) {
    let (width, height) = det_size(image.width(), image.height());
    let whole = PhysRect { x: 0, y: 0, width: image.width(), height: image.height() };
    (width, height, resample_nchw(image, whole, width, height, IMAGENET))
}

/// A text region in probability-map coordinates, with its mean probability.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MapBox {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
    pub score: f32,
}

#[derive(Debug, Clone, Copy)]
struct Component {
    min_x: usize,
    min_y: usize,
    max_x: usize,
    max_y: usize,
    sum: f32,
    count: usize,
}

fn find(parent: &mut [usize], mut node: usize) -> usize {
    while parent[node] != node {
        parent[node] = parent[parent[node]];
        node = parent[node];
    }
    node
}

fn union(parent: &mut [usize], a: usize, b: usize) {
    let (root_a, root_b) = (find(parent, a), find(parent, b));
    if root_a != root_b {
        parent[root_b] = root_a;
    }
}

/// DB post-processing. Pixels above `DET_THRESHOLD` form 8-connected components (union-find). Each
/// component's bounding box is kept if its area is at least `DET_MIN_AREA` and its mean probability
/// is at least `DET_BOX_THRESHOLD`. Kept boxes are grown by the unclip distance, `area * ratio /
/// perimeter`, on every side, and clamped to the map. The result is sorted top to bottom, then left
/// to right.
pub fn db_boxes(prob: &[f32], width: usize, height: usize) -> Vec<MapBox> {
    if width == 0 || height == 0 || prob.len() != width * height {
        return Vec::new();
    }
    let on = |i: usize| prob[i] > DET_THRESHOLD;
    let mut parent: Vec<usize> = (0..prob.len()).collect();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !on(i) {
                continue;
            }
            if x > 0 && on(i - 1) {
                union(&mut parent, i, i - 1);
            }
            if y > 0 {
                let up = i - width;
                if x > 0 && on(up - 1) {
                    union(&mut parent, i, up - 1);
                }
                if on(up) {
                    union(&mut parent, i, up);
                }
                if x + 1 < width && on(up + 1) {
                    union(&mut parent, i, up + 1);
                }
            }
        }
    }

    let mut components: HashMap<usize, Component> = HashMap::new();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !on(i) {
                continue;
            }
            let root = find(&mut parent, i);
            let component = components.entry(root).or_insert(Component {
                min_x: x,
                min_y: y,
                max_x: x,
                max_y: y,
                sum: 0.0,
                count: 0,
            });
            component.min_x = component.min_x.min(x);
            component.min_y = component.min_y.min(y);
            component.max_x = component.max_x.max(x);
            component.max_y = component.max_y.max(y);
            component.sum += prob[i];
            component.count += 1;
        }
    }

    let mut boxes: Vec<MapBox> = components
        .values()
        .filter_map(|component| component_box(component, width, height))
        .collect();
    boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    boxes
}

fn component_box(component: &Component, width: usize, height: usize) -> Option<MapBox> {
    let box_width = component.max_x - component.min_x + 1;
    let box_height = component.max_y - component.min_y + 1;
    if box_width * box_height < DET_MIN_AREA {
        return None;
    }
    let score = component.sum / component.count as f32;
    if score < DET_BOX_THRESHOLD {
        return None;
    }
    let area = (box_width * box_height) as f32;
    let perimeter = 2.0 * (box_width + box_height) as f32;
    let distance = area * DET_UNCLIP_RATIO / perimeter;
    Some(MapBox {
        x0: (component.min_x as f32 - distance).max(0.0),
        y0: (component.min_y as f32 - distance).max(0.0),
        x1: ((component.max_x + 1) as f32 + distance).min(width as f32),
        y1: ((component.max_y + 1) as f32 + distance).min(height as f32),
        score,
    })
}

/// Maps a box from probability-map coordinates to source pixels, rounding outwards.
pub fn to_source_rect(boxed: MapBox, map_width: usize, map_height: usize, source_width: u32, source_height: u32) -> PhysRect {
    let scale_x = source_width as f32 / map_width as f32;
    let scale_y = source_height as f32 / map_height as f32;
    let x0 = (boxed.x0 * scale_x).floor().clamp(0.0, source_width.saturating_sub(1) as f32);
    let y0 = (boxed.y0 * scale_y).floor().clamp(0.0, source_height.saturating_sub(1) as f32);
    let x1 = (boxed.x1 * scale_x).ceil().min(source_width as f32);
    let y1 = (boxed.y1 * scale_y).ceil().min(source_height as f32);
    PhysRect {
        x: x0 as i32,
        y: y0 as i32,
        width: (x1 - x0).max(1.0) as u32,
        height: (y1 - y0).max(1.0) as u32,
    }
}

/// The scaled recogniser width for a crop: the height is scaled to `REC_HEIGHT` and the aspect ratio is
/// kept, up to `REC_MAX_WIDTH`.
pub fn rec_width(crop_width: u32, crop_height: u32) -> u32 {
    let ratio = crop_width as f32 / crop_height.max(1) as f32;
    ((REC_HEIGHT as f32 * ratio).ceil() as u32).clamp(1, REC_MAX_WIDTH)
}

/// The recogniser input for one text region: NCHW data of shape `[3, REC_HEIGHT, REC_MAX_WIDTH]`. The
/// scaled crop sits on the left and the rest is zero after normalisation, as RapidOCR pads. `None` if
/// the region is empty after clipping to the image.
pub fn rec_input(image: &BgrImage, rect: PhysRect) -> Option<Vec<f32>> {
    let clipped = clip_rect(image, rect)?;
    let scaled = rec_width(clipped.width, clipped.height);
    let crop = resample_nchw(image, clipped, scaled, REC_HEIGHT, HALF);
    Some(pad_right(&crop, 3 * REC_HEIGHT as usize, scaled as usize, REC_MAX_WIDTH as usize))
}

/// Pads each of `rows` rows of `from` values with zeros on the right, up to `to` values.
fn pad_right(data: &[f32], rows: usize, from: usize, to: usize) -> Vec<f32> {
    let mut out = vec![0.0; rows * to];
    for row in 0..rows {
        out[row * to..row * to + from].copy_from_slice(&data[row * from..row * from + from]);
    }
    out
}

/// The best class and its score for each time step of a `[T, classes]` score matrix.
pub fn best_per_step(scores: &[f32], classes: usize) -> Vec<(usize, f32)> {
    if classes == 0 {
        return Vec::new();
    }
    scores
        .chunks_exact(classes)
        .map(|row| {
            let mut best = (0, f32::NEG_INFINITY);
            for (index, &value) in row.iter().enumerate() {
                if value > best.1 {
                    best = (index, value);
                }
            }
            best
        })
        .collect()
}

/// CTC greedy decoding. Repeats are collapsed first, then the blank (index 0) is dropped. `classes[i]`
/// is the text for class `i`. Returns the text and the mean score of the emitted characters.
pub fn ctc_greedy(steps: &[(usize, f32)], classes: &[String]) -> (String, f32) {
    let mut text = String::new();
    let mut total = 0.0;
    let mut emitted = 0usize;
    let mut previous = None;
    for &(index, score) in steps {
        let repeat = previous == Some(index);
        previous = Some(index);
        if index == 0 || repeat {
            continue;
        }
        if let Some(character) = classes.get(index) {
            text.push_str(character);
            total += score;
            emitted += 1;
        }
    }
    let confidence = if emitted == 0 { 0.0 } else { total / emitted as f32 };
    (text, confidence)
}

/// Parses a dictionary file: one entry per line. `\r` before a newline is removed, and nothing else is
/// trimmed, so a space entry survives.
pub fn parse_dictionary(text: &str) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let body = text.strip_suffix('\n').unwrap_or(text);
    body.split('\n').map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned()).collect()
}

/// The CTC class table: class 0 is the blank (empty text), then the dictionary. If the model has one
/// class more, the last class is a space. Any other size is an error, because a mismatched dictionary
/// would give wrong text without any other sign.
pub fn ctc_classes(dictionary: &[String], model_classes: usize) -> Result<Vec<String>, String> {
    let mut classes = Vec::with_capacity(dictionary.len() + 2);
    classes.push(String::new());
    classes.extend(dictionary.iter().cloned());
    if model_classes == classes.len() {
        return Ok(classes);
    }
    if model_classes == classes.len() + 1 {
        classes.push(" ".to_owned());
        return Ok(classes);
    }
    Err(format!(
        "the model has {model_classes} output classes, but the dictionary gives {} with the blank (or {} with a space)",
        classes.len(),
        classes.len() + 1
    ))
}

/// Reading order for text regions. Regions are grouped into lines when their vertical extents overlap
/// by at least half of the shorter one. Lines run top to bottom and regions run left to right within a
/// line. Returns indices into `rects`. Right-to-left scripts are not handled here.
pub fn reading_order(rects: &[PhysRect]) -> Vec<usize> {
    let mut by_top: Vec<usize> = (0..rects.len()).collect();
    by_top.sort_by_key(|&i| (rects[i].y, rects[i].x));

    struct Line {
        top: i32,
        bottom: i32,
        members: Vec<usize>,
    }
    let mut lines: Vec<Line> = Vec::new();
    for i in by_top {
        let rect = rects[i];
        let (top, bottom) = (rect.y, rect.y + rect.height as i32);
        let target = lines.iter().position(|line| {
            let overlap = line.bottom.min(bottom) - line.top.max(top);
            let shorter = (line.bottom - line.top).min(bottom - top);
            overlap > 0 && overlap * 2 >= shorter
        });
        match target {
            Some(line) => lines[line].members.push(i),
            None => lines.push(Line { top, bottom, members: vec![i] }),
        }
    }

    let mut order = Vec::with_capacity(rects.len());
    for mut line in lines {
        line.members.sort_by_key(|&i| rects[i].x);
        order.extend(line.members);
    }
    order
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect { x, y, width, height }
    }

    fn strings(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn det_size_limits_the_longest_side_and_rounds_to_32() {
        assert_eq!(det_size(1920, 1080), (960, 544));
        assert_eq!(det_size(100, 50), (96, 64));
        assert_eq!(det_size(10, 10), (32, 32));
        assert_eq!(det_size(640, 480), (640, 480));
    }

    #[test]
    fn two_blobs_give_two_boxes_in_reading_order() {
        let (width, height) = (20usize, 12usize);
        let mut prob = vec![0.1f32; width * height];
        for y in 2..=5 {
            for x in 2..=6 {
                prob[y * width + x] = 0.9;
            }
        }
        for y in 6..=9 {
            for x in 12..=17 {
                prob[y * width + x] = 0.8;
            }
        }
        let boxes = db_boxes(&prob, width, height);
        assert_eq!(boxes.len(), 2);
        let first = boxes[0];
        assert!(first.x0 < 2.0 && first.x1 > 7.0, "unclip grows the first box: {first:?}");
        assert!(first.y0 < 2.0 && first.y1 > 6.0, "unclip grows the first box: {first:?}");
        assert!((first.score - 0.9).abs() < 1e-5);
        assert!(boxes[1].y0 > boxes[0].y0);
        assert!((boxes[1].score - 0.8).abs() < 1e-5);
    }

    #[test]
    fn a_low_mean_probability_region_is_dropped() {
        let (width, height) = (10usize, 10usize);
        let mut prob = vec![0.0f32; width * height];
        for y in 0..4 {
            for x in 0..5 {
                prob[y * width + x] = 0.5;
            }
        }
        assert!(db_boxes(&prob, width, height).is_empty());
    }

    #[test]
    fn a_tiny_region_is_dropped() {
        let (width, height) = (10usize, 10usize);
        let mut prob = vec![0.0f32; width * height];
        prob[3 * width + 3] = 0.99;
        assert!(db_boxes(&prob, width, height).is_empty());
    }

    #[test]
    fn diagonal_pixels_join_under_eight_connectivity() {
        let (width, height) = (8usize, 8usize);
        let mut prob = vec![0.0f32; width * height];
        for i in 0..5 {
            prob[i * width + i] = 0.9;
        }
        let boxes = db_boxes(&prob, width, height);
        assert_eq!(boxes.len(), 1);
    }

    #[test]
    fn a_mismatched_probability_map_gives_no_boxes() {
        assert!(db_boxes(&[0.9; 5], 2, 2).is_empty());
    }

    #[test]
    fn map_box_scales_back_to_the_source() {
        let boxed = MapBox { x0: 10.0, y0: 5.0, x1: 20.0, y1: 15.0, score: 0.9 };
        let source = to_source_rect(boxed, 100, 50, 1000, 500);
        assert_eq!(source, rect(100, 50, 100, 100));
    }

    #[test]
    fn ctc_collapses_repeats_then_drops_blanks() {
        let classes = strings(&["", "a", "b", "c", "D", "E"]);
        let steps: Vec<(usize, f32)> = [0usize, 3, 3, 0, 5].iter().map(|&i| (i, 0.5)).collect();
        let (text, confidence) = ctc_greedy(&steps, &classes);
        assert_eq!(text, "cE");
        assert!((confidence - 0.5).abs() < 1e-6);
    }

    #[test]
    fn ctc_keeps_a_repeat_split_by_a_blank() {
        let classes = strings(&["", "a", "b", "c"]);
        let steps = [(3usize, 0.9f32), (0, 0.9), (3, 0.7)];
        let (text, confidence) = ctc_greedy(&steps, &classes);
        assert_eq!(text, "cc");
        assert!((confidence - 0.8).abs() < 1e-6);
    }

    #[test]
    fn ctc_with_only_blanks_is_empty() {
        let (text, confidence) = ctc_greedy(&[(0, 0.9), (0, 0.9)], &strings(&["", "a"]));
        assert!(text.is_empty());
        assert_eq!(confidence, 0.0);
    }

    #[test]
    fn best_per_step_takes_the_argmax_of_each_row() {
        let steps = best_per_step(&[0.1, 0.7, 0.2, 0.9, 0.05, 0.05], 3);
        assert_eq!(steps, vec![(1, 0.7), (0, 0.9)]);
    }

    #[test]
    fn dictionary_keeps_spaces_and_strips_only_carriage_returns() {
        assert_eq!(parse_dictionary("a\r\nb\n"), strings(&["a", "b"]));
        assert_eq!(parse_dictionary("x\n"), strings(&["x"]));
        assert_eq!(parse_dictionary(" \n"), strings(&[" "]));
        assert!(parse_dictionary("").is_empty());
    }

    #[test]
    fn class_table_matches_the_model_output() {
        let dictionary = strings(&["a", "b"]);
        assert_eq!(ctc_classes(&dictionary, 3).expect("no space"), strings(&["", "a", "b"]));
        assert_eq!(ctc_classes(&dictionary, 4).expect("with space"), strings(&["", "a", "b", " "]));
        assert!(ctc_classes(&dictionary, 5).is_err());
    }

    #[test]
    fn reading_order_groups_a_line_before_moving_down() {
        // "a" and "b" share a line, and "b" starts one pixel higher. A plain (y, x) sort would
        // put "b" first.
        let rects = [
            rect(0, 10, 30, 20),  // a
            rect(50, 9, 30, 20),  // b
            rect(0, 50, 30, 20),  // c, the next line down
            rect(50, 52, 30, 20), // d, on c's line
        ];
        assert_eq!(reading_order(&rects), vec![0, 1, 2, 3]);
    }

    #[test]
    fn reading_order_puts_a_later_line_after_an_earlier_one() {
        let rects = [rect(40, 60, 10, 10), rect(0, 0, 10, 10)];
        assert_eq!(reading_order(&rects), vec![1, 0]);
    }

    #[test]
    fn rec_width_scales_the_height_and_caps_the_width() {
        assert_eq!(rec_width(100, 20), 240);
        assert_eq!(rec_width(10, 40), 12);
        assert_eq!(rec_width(3200, 20), REC_MAX_WIDTH);
    }

    #[test]
    fn rec_input_pads_the_scaled_crop_to_the_max_width() {
        // A 100x20 crop scales to 240x48. The input is [3, 48, 320], so columns 240 and up are zero.
        let image = BgrImage::new(100, 20, vec![128; 100 * 20 * 3]).expect("valid image");
        let data = rec_input(&image, rect(0, 0, 100, 20)).expect("non-empty crop");
        assert_eq!(data.len(), 3 * 48 * 320);
        assert!(data[239] > 0.0, "the last scaled column is inside the crop");
        assert_eq!(data[240], 0.0, "the first padded column");
        assert!(rec_input(&image, rect(200, 0, 10, 10)).is_none());
    }

    #[test]
    fn det_input_has_the_detector_shape() {
        let image = BgrImage::new(70, 40, vec![0; 70 * 40 * 3]).expect("valid image");
        let (width, height, data) = det_input(&image);
        assert_eq!((width, height), (64, 32));
        assert_eq!(data.len(), 3 * 64 * 32);
    }
}

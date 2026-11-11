//! Family O1: PP-OCR, a DBNet detector and a CTC recogniser.
//!
//! Everything here is pure: no ONNX Runtime, no threads. The inference steps live in `ocr.rs`.
//! Settings follow PaddleOCR's defaults.

use std::collections::HashMap;

use laqta_domain::PhysRect;

use crate::tensor::{BgrImage, HALF, IMAGENET, clip_rect, resample_nchw};

/// The detector's longest side, in pixels (PaddleOCR's `max_side_limit`). Larger images are scaled
/// down to it. Screenshots stay at full size, because UI text is small and shrinking it loses
/// characters.
pub const DET_MAX_SIDE: u32 = 4000;
/// The detector's shortest side, in pixels. Smaller images are scaled up to it.
pub const DET_MIN_SIDE: u32 = 64;
/// The detector needs both sides to be multiples of this.
pub const DET_MULTIPLE: u32 = 32;
/// A region whose shorter side is under this many probability-map pixels is dropped (PaddleOCR's
/// `min_size`); after the unclip, the limit is two pixels more.
pub const DET_MIN_SIDE_PX: f32 = 3.0;
/// A crop at least this many times taller than wide is vertical text, read after a 90° turn.
pub const REC_VERTICAL_RATIO: f32 = 1.5;
/// The recogniser's input height.
pub const REC_HEIGHT: u32 = 48;
/// The narrowest recogniser input. A short crop is padded to it on the right.
pub const REC_MIN_WIDTH: u32 = 320;
/// The widest recogniser input. A crop keeps its aspect ratio up to this width; only a longer one
/// is squashed.
pub const REC_WIDTH_LIMIT: u32 = 3200;

/// DBNet post-processing settings. Each detector model has its own; the defaults are PP-OCRv5's.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DbParams {
    /// Probability above which a pixel counts as text.
    pub thresh: f32,
    /// Mean probability a region needs to be kept.
    pub box_thresh: f32,
    /// How far a detected region grows.
    pub unclip_ratio: f32,
}

impl DbParams {
    /// Both thresholds must be probabilities and the unclip ratio a modest growth factor.
    pub fn check(&self) -> Result<(), String> {
        let probability = |v: f32| v > 0.0 && v < 1.0;
        if !probability(self.thresh) || !probability(self.box_thresh) {
            return Err(format!("db thresholds must be between 0 and 1, got {} and {}", self.thresh, self.box_thresh));
        }
        if !(self.unclip_ratio > 0.0 && self.unclip_ratio <= 4.0) {
            return Err(format!("db unclip_ratio must be in (0, 4], got {}", self.unclip_ratio));
        }
        Ok(())
    }
}

impl Default for DbParams {
    fn default() -> Self {
        Self { thresh: 0.3, box_thresh: 0.6, unclip_ratio: 1.5 }
    }
}

/// The detector input size: the longest side is at most `DET_MAX_SIDE`, the shortest at least
/// `DET_MIN_SIDE`, and each side is rounded to the nearest multiple of `DET_MULTIPLE` (at least one
/// multiple).
pub fn det_size(width: u32, height: u32) -> (u32, u32) {
    let longest = width.max(height).max(1);
    let shortest = width.min(height).max(1);
    let ratio = if longest > DET_MAX_SIDE {
        DET_MAX_SIDE as f32 / longest as f32
    } else if shortest < DET_MIN_SIDE {
        (DET_MIN_SIDE as f32 / shortest as f32).min(DET_MAX_SIDE as f32 / longest as f32)
    } else {
        1.0
    };
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

/// DB post-processing. Pixels above `params.thresh` form 8-connected components (union-find). Each
/// component's bounding box is kept if its area is at least `DET_MIN_AREA` and its mean probability
/// is at least `params.box_thresh`. Kept boxes are grown by the unclip distance, `area * ratio /
/// perimeter`, on every side, and clamped to the map. The result is sorted top to bottom, then left
/// to right.
pub fn db_boxes(prob: &[f32], width: usize, height: usize, params: &DbParams) -> Vec<MapBox> {
    if width == 0 || height == 0 || prob.len() != width * height {
        return Vec::new();
    }
    let on = |i: usize| prob[i] > params.thresh;
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

    // Prefix sums of the probability map, for the mean over any box in constant time.
    let mut sums = vec![0.0f64; (width + 1) * (height + 1)];
    for y in 0..height {
        let mut row = 0.0f64;
        for x in 0..width {
            row += f64::from(prob[y * width + x]);
            sums[(y + 1) * (width + 1) + x + 1] = sums[y * (width + 1) + x + 1] + row;
        }
    }
    let box_mean = |c: &Component| {
        let at = |x: usize, y: usize| sums[y * (width + 1) + x];
        let total = at(c.max_x + 1, c.max_y + 1) - at(c.min_x, c.max_y + 1) - at(c.max_x + 1, c.min_y) + at(c.min_x, c.min_y);
        (total / ((c.max_x - c.min_x + 1) * (c.max_y - c.min_y + 1)) as f64) as f32
    };

    let mut components: HashMap<usize, Component> = HashMap::new();
    for y in 0..height {
        for x in 0..width {
            let i = y * width + x;
            if !on(i) {
                continue;
            }
            let root = find(&mut parent, i);
            let component = components.entry(root).or_insert(Component { min_x: x, min_y: y, max_x: x, max_y: y });
            component.min_x = component.min_x.min(x);
            component.min_y = component.min_y.min(y);
            component.max_x = component.max_x.max(x);
            component.max_y = component.max_y.max(y);
        }
    }

    let mut boxes: Vec<MapBox> =
        components.values().filter_map(|component| component_box(component, box_mean(component), width, height, params)).collect();
    boxes.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
    boxes
}

/// One component's box, scored as PaddleOCR's `box_score_fast` does: the mean probability over the
/// whole box, not only over the pixels above the threshold.
fn component_box(component: &Component, score: f32, width: usize, height: usize, params: &DbParams) -> Option<MapBox> {
    let box_width = (component.max_x - component.min_x + 1) as f32;
    let box_height = (component.max_y - component.min_y + 1) as f32;
    if box_width.min(box_height) < DET_MIN_SIDE_PX || score < params.box_thresh {
        return None;
    }
    let area = box_width * box_height;
    let perimeter = 2.0 * (box_width + box_height);
    let distance = area * params.unclip_ratio / perimeter;
    if (box_width + 2.0 * distance).min(box_height + 2.0 * distance) < DET_MIN_SIDE_PX + 2.0 {
        return None;
    }
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
    PhysRect { x: x0 as i32, y: y0 as i32, width: (x1 - x0).max(1.0) as u32, height: (y1 - y0).max(1.0) as u32 }
}

/// The scaled recogniser width for a crop: the height is scaled to `REC_HEIGHT` and the aspect ratio is
/// kept, up to `REC_WIDTH_LIMIT`.
pub fn rec_width(crop_width: u32, crop_height: u32) -> u32 {
    let ratio = crop_width as f32 / crop_height.max(1) as f32;
    ((REC_HEIGHT as f32 * ratio).ceil() as u32).clamp(1, REC_WIDTH_LIMIT)
}

/// The recogniser input width for a scaled crop: at least `REC_MIN_WIDTH`, as PaddleOCR's
/// `max_wh_ratio` starts at 320 / 48.
pub fn rec_input_width(scaled: u32) -> u32 {
    scaled.max(REC_MIN_WIDTH)
}

/// How many text lines the recogniser reads in one run, as PaddleOCR's `rec_batch_num`.
pub const REC_BATCH: usize = 6;

/// One text region scaled for the recogniser: `(scaled width, NCHW data of shape [3, REC_HEIGHT,
/// scaled width])`, not yet padded. Long lines keep their aspect ratio instead of being squashed,
/// and a crop at least 1.5 times taller than wide is turned 90° counter-clockwise first
/// (PaddleOCR's vertical-text rule). `None` if the region is empty after clipping to the image.
pub fn rec_crop(image: &BgrImage, rect: PhysRect) -> Option<(u32, Vec<f32>)> {
    let clipped = clip_rect(image, rect)?;
    let (source, region) = if clipped.height as f32 >= REC_VERTICAL_RATIO * clipped.width as f32 {
        let turned = image.crop_rotated_ccw(clipped);
        let whole = PhysRect { x: 0, y: 0, width: turned.width(), height: turned.height() };
        (std::borrow::Cow::Owned(turned), whole)
    } else {
        (std::borrow::Cow::Borrowed(image), clipped)
    };
    let scaled = rec_width(region.width, region.height);
    Some((scaled, resample_nchw(&source, region, scaled, REC_HEIGHT, HALF)))
}

/// Pads a `rec_crop` result on the right with zeros (gray, after normalisation) to `width` columns,
/// as PaddleOCR pads every line of a batch to the batch's widest.
pub fn rec_pad(scaled: u32, crop: &[f32], width: u32) -> Vec<f32> {
    pad_right(crop, 3 * REC_HEIGHT as usize, scaled as usize, width as usize)
}

/// The recogniser input for one text region on its own: `(width, NCHW data of shape [3,
/// REC_HEIGHT, width])`, the scaled crop padded to `rec_input_width`.
pub fn rec_input(image: &BgrImage, rect: PhysRect) -> Option<(u32, Vec<f32>)> {
    let (scaled, crop) = rec_crop(image, rect)?;
    let width = rec_input_width(scaled);
    Some((width, rec_pad(scaled, &crop, width)))
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

/// Where `ctc_greedy` read each character: for every class it emits, the middle of the run of time
/// steps that repeat it, as a fraction of all the steps (0 is the left edge of the recogniser's
/// input, 1 the right), with the class's text. The texts in order are `ctc_greedy`'s text.
pub fn ctc_positions<'a>(steps: &[(usize, f32)], classes: &'a [String]) -> Vec<(f32, &'a str)> {
    let total = steps.len() as f32;
    let mut out = Vec::new();
    let mut at = 0;
    while at < steps.len() {
        let index = steps[at].0;
        let mut end = at + 1;
        while end < steps.len() && steps[end].0 == index {
            end += 1;
        }
        if index != 0
            && let Some(character) = classes.get(index)
        {
            out.push(((at + end) as f32 / 2.0 / total, character.as_str()));
        }
        at = end;
    }
    out
}

/// The words of one line with a box each, from `ctc_positions`. `visible` is the part of the
/// recogniser's input that the line's crop fills (the rest is padding), and `rect` is the line in
/// source pixels. Each character is given half the line's usual character pitch on either side of
/// where it was read, a word spans its characters, and neighbours that would overlap meet halfway.
/// Whitespace classes separate words and are in none.
pub fn ctc_words(positions: &[(f32, &str)], visible: f32, rect: PhysRect) -> Vec<(PhysRect, String)> {
    let along = |position: f32| if visible > 0.0 { (position / visible).clamp(0.0, 1.0) } else { 0.0 };
    let places: Vec<f32> = positions.iter().map(|(position, _)| along(*position)).collect();
    let mut pitches: Vec<f32> = places.windows(2).map(|pair| (pair[1] - pair[0]).abs()).filter(|pitch| *pitch > 0.0).collect();
    pitches.sort_by(f32::total_cmp);
    // A line of one character has no pitch: that character is the whole line.
    let half = pitches.get(pitches.len() / 2).map_or(1.0, |pitch| pitch / 2.0);
    let mut spans: Vec<(f32, f32, String)> = Vec::new();
    let mut open = false;
    for ((_, text), &place) in positions.iter().zip(&places) {
        if text.chars().all(char::is_whitespace) {
            open = false;
            continue;
        }
        match spans.last_mut().filter(|_| open) {
            Some(span) => {
                span.0 = span.0.min(place - half);
                span.1 = span.1.max(place + half);
                span.2.push_str(text);
            }
            None => spans.push((place - half, place + half, (*text).to_owned())),
        }
        open = true;
    }
    for i in 1..spans.len() {
        if spans[i].0 < spans[i - 1].1 {
            let middle = (spans[i].0 + spans[i - 1].1) / 2.0;
            spans[i - 1].1 = middle;
            spans[i].0 = middle;
        }
    }
    let width = rect.width as f32;
    spans
        .into_iter()
        .map(|(from, to, text)| {
            let left = (from.clamp(0.0, 1.0) * width).round() as u32;
            let right = (to.clamp(0.0, 1.0) * width).round() as u32;
            let word = PhysRect { x: rect.x.saturating_add_unsigned(left), width: right.saturating_sub(left).max(1), ..rect };
            (word, text)
        })
        .collect()
}

/// Whether `rec_crop` turns a region of this shape on its side before it is read.
pub fn rec_turns(rect: PhysRect) -> bool {
    rect.height as f32 >= REC_VERTICAL_RATIO * rect.width as f32
}

/// The words of the line `rect` with a box each, as `ctc_words` places them. A line `rec_crop`
/// turned on its side was read left to right after a quarter turn counter-clockwise, which runs
/// top to bottom down the line, so its words are placed down its height.
pub fn line_words(positions: &[(f32, &str)], visible: f32, rect: PhysRect) -> Vec<(PhysRect, String)> {
    if !rec_turns(rect) {
        return ctc_words(positions, visible, rect);
    }
    let along = PhysRect { x: 0, y: 0, width: rect.height, height: rect.width };
    ctc_words(positions, visible, along)
        .into_iter()
        .map(|(word, text)| (PhysRect { y: rect.y.saturating_add(word.x), height: word.width, ..rect }, text))
        .collect()
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
    fn det_size_keeps_screenshots_full_size_and_rounds_to_32() {
        assert_eq!(det_size(1920, 1080), (1920, 1088), "a screenshot is not shrunk");
        assert_eq!(det_size(640, 480), (640, 480));
        assert_eq!(det_size(8000, 1000), (4000, 512), "the longest side is capped");
        assert_eq!(det_size(100, 50), (128, 64), "a small image is scaled up to a 64 px short side");
        assert_eq!(det_size(10, 10), (64, 64));
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
        let boxes = db_boxes(&prob, width, height, &DbParams::default());
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
        assert!(db_boxes(&prob, width, height, &DbParams::default()).is_empty());
    }

    #[test]
    fn a_tiny_region_is_dropped() {
        let (width, height) = (10usize, 10usize);
        let mut prob = vec![0.0f32; width * height];
        prob[3 * width + 3] = 0.99;
        assert!(db_boxes(&prob, width, height, &DbParams::default()).is_empty());
    }

    #[test]
    fn diagonal_pixels_join_under_eight_connectivity() {
        let (width, height) = (8usize, 8usize);
        let mut prob = vec![0.0f32; width * height];
        for i in 0..5 {
            prob[i * width + i] = 0.9;
        }
        // The diagonal fills only a fifth of its box, so a low box threshold keeps it.
        let params = DbParams { box_thresh: 0.1, ..DbParams::default() };
        let boxes = db_boxes(&prob, width, height, &params);
        assert_eq!(boxes.len(), 1);
    }

    #[test]
    fn the_score_is_the_mean_over_the_whole_box() {
        // A 5x5 ring of 0.9 around an empty middle: 0.9 over the lit pixels, but 16 * 0.9 / 25 =
        // 0.576 over the box, under PP-OCRv5's 0.6, so PaddleOCR drops it and so does Laqta.
        let (width, height) = (10usize, 10usize);
        let mut prob = vec![0.0f32; width * height];
        for y in 2..7 {
            for x in 2..7 {
                if y == 2 || y == 6 || x == 2 || x == 6 {
                    prob[y * width + x] = 0.9;
                }
            }
        }
        assert!(db_boxes(&prob, width, height, &DbParams::default()).is_empty());
        let lenient = DbParams { box_thresh: 0.5, ..DbParams::default() };
        let boxes = db_boxes(&prob, width, height, &lenient);
        assert_eq!(boxes.len(), 1);
        assert!((boxes[0].score - 0.576).abs() < 1e-4, "{:?}", boxes[0]);
    }

    #[test]
    fn a_region_thinner_than_three_pixels_is_dropped() {
        let (width, height) = (20usize, 10usize);
        let mut prob = vec![0.0f32; width * height];
        for x in 2..18 {
            prob[4 * width + x] = 0.95;
            prob[5 * width + x] = 0.95;
        }
        assert!(db_boxes(&prob, width, height, &DbParams::default()).is_empty());
    }

    #[test]
    fn db_params_are_checked() {
        assert!(DbParams::default().check().is_ok());
        assert!(DbParams { thresh: 1.2, ..DbParams::default() }.check().is_err());
        assert!(DbParams { unclip_ratio: 0.0, ..DbParams::default() }.check().is_err());
    }

    #[test]
    fn a_mismatched_probability_map_gives_no_boxes() {
        assert!(db_boxes(&[0.9; 5], 2, 2, &DbParams::default()).is_empty());
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
    fn ctc_positions_are_the_middles_of_the_runs_and_spell_the_same_text() {
        let classes = strings(&["", "a", "b", " "]);
        // Ten steps: a a _ b _ _ space _ a _
        let steps: Vec<(usize, f32)> = [1, 1, 0, 2, 0, 0, 3, 0, 1, 0].iter().map(|&index| (index, 0.9)).collect();
        let positions = ctc_positions(&steps, &classes);
        assert_eq!(positions.iter().map(|(_, text)| *text).collect::<String>(), ctc_greedy(&steps, &classes).0);
        assert_eq!(positions.iter().map(|(at, _)| *at).collect::<Vec<_>>(), vec![0.1, 0.35, 0.65, 0.85]);
        assert!(ctc_positions(&[], &classes).is_empty());
    }

    #[test]
    fn ctc_words_span_their_characters_and_leave_the_spaces_out() {
        let line = PhysRect { x: 100, y: 10, width: 200, height: 20 };
        // "ab cd" read evenly across the crop, which fills the left half of a padded input.
        let positions = [(0.05, "a"), (0.15, "b"), (0.25, " "), (0.35, "c"), (0.45, "d")];
        let words = ctc_words(&positions, 0.5, line);
        assert_eq!(
            words,
            vec![(PhysRect { x: 100, width: 80, ..line }, "ab".to_owned()), (PhysRect { x: 220, width: 80, ..line }, "cd".to_owned())]
        );
    }

    #[test]
    fn ctc_words_that_would_overlap_meet_halfway_and_stay_inside_the_line() {
        let line = PhysRect { x: 100, y: 10, width: 200, height: 20 };
        let positions = [(0.1, "a"), (0.5, "b"), (0.55, " "), (0.6, "c"), (1.0, "d")];
        let words = ctc_words(&positions, 1.0, line);
        assert_eq!(
            words,
            vec![(PhysRect { x: 100, width: 110, ..line }, "ab".to_owned()), (PhysRect { x: 210, width: 90, ..line }, "cd".to_owned())]
        );
        // One character has no pitch to measure: it is the whole line.
        assert_eq!(ctc_words(&[(0.3, "x")], 1.0, line), vec![(line, "x".to_owned())]);
        assert!(ctc_words(&[(0.3, " ")], 1.0, line).is_empty());
    }

    #[test]
    fn the_words_of_a_line_read_on_its_side_run_down_it() {
        let line = PhysRect { x: 40, y: 100, width: 20, height: 200 };
        let positions = [(0.05, "a"), (0.15, "b"), (0.25, " "), (0.35, "c"), (0.45, "d")];
        assert_eq!(
            line_words(&positions, 0.5, line),
            vec![(PhysRect { y: 100, height: 80, ..line }, "ab".to_owned()), (PhysRect { y: 220, height: 80, ..line }, "cd".to_owned())]
        );
        // A line read across is placed along its width, as `ctc_words` does.
        let across = PhysRect { x: 100, y: 10, width: 200, height: 20 };
        assert_eq!(line_words(&positions, 0.5, across), ctc_words(&positions, 0.5, across));
    }

    #[test]
    fn a_region_read_on_its_side_is_the_one_rec_crop_turns() {
        assert!(rec_turns(PhysRect { x: 0, y: 0, width: 20, height: 30 }));
        assert!(!rec_turns(PhysRect { x: 0, y: 0, width: 21, height: 30 }));
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
        assert_eq!(rec_width(100_000, 20), REC_WIDTH_LIMIT);
    }

    #[test]
    fn rec_input_pads_a_short_crop_to_the_minimum_width() {
        // A 100x20 crop scales to 240x48. The input is [3, 48, 320], so columns 240 and up are zero.
        let image = BgrImage::new(100, 20, vec![128; 100 * 20 * 3]).expect("valid image");
        let (width, data) = rec_input(&image, rect(0, 0, 100, 20)).expect("non-empty crop");
        assert_eq!(width, 320);
        assert_eq!(data.len(), 3 * 48 * 320);
        assert!(data[239] > 0.0, "the last scaled column is inside the crop");
        assert_eq!(data[240], 0.0, "the first padded column");
        assert!(rec_input(&image, rect(200, 0, 10, 10)).is_none());
    }

    #[test]
    fn a_long_line_gets_a_wider_input_instead_of_being_squashed() {
        // 1000x20 keeps its 50:1 ratio at 48 px high: 2400 columns, not 320.
        let image = BgrImage::new(1000, 20, vec![200; 1000 * 20 * 3]).expect("valid image");
        let (width, data) = rec_input(&image, rect(0, 0, 1000, 20)).expect("non-empty crop");
        assert_eq!(width, 2400);
        assert_eq!(data.len(), 3 * 48 * 2400);
    }

    #[test]
    fn a_tall_crop_is_turned_before_reading() {
        // A 20x100 column of text is read as a 100x20 line: 240 scaled columns, padded to 320.
        let mut pixels = vec![255u8; 20 * 100 * 3];
        // Mark the top-right pixel; after a counter-clockwise turn it is the top-left one.
        let top_right = (19) * 3;
        pixels[top_right..top_right + 3].copy_from_slice(&[0, 0, 0]);
        let image = BgrImage::new(20, 100, pixels).expect("valid image");
        let turned = image.crop_rotated_ccw(rect(0, 0, 20, 100));
        assert_eq!((turned.width(), turned.height()), (100, 20));
        let (width, _) = rec_input(&image, rect(0, 0, 20, 100)).expect("non-empty crop");
        assert_eq!(width, 320);
    }

    #[test]
    fn det_input_has_the_detector_shape() {
        let image = BgrImage::new(70, 40, vec![0; 70 * 40 * 3]).expect("valid image");
        let (width, height, data) = det_input(&image);
        assert_eq!((width, height), (128, 64), "the short side is scaled up to 64");
        assert_eq!(data.len(), 3 * 128 * 64);
    }
}

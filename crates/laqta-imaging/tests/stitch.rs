//! Scrolling-capture stitching on synthetic pages. Every page row is distinct, frames are cut
//! at known offsets, and the stitched image must equal the page exactly.
#![allow(clippy::unwrap_used)]

use std::collections::HashSet;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_imaging::stitch::{StitchOptions, StitchStep, Stitcher};

const W: u32 = 64;
const ROW: usize = W as usize * 4;
const H: u32 = 100;
const PAGE_ROWS: u32 = 400;
/// Frame offsets of a plain scroll: steps of 37 rows, then a last step of 4 to reach the page end.
const OFFSETS: [u32; 10] = [0, 37, 74, 111, 148, 185, 222, 259, 296, 300];

fn splitmix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

/// A page of pseudo-random pixels. Every row is distinct, which the matcher relies on.
fn make_page(seed: u64) -> Vec<u8> {
    let mut px = Vec::with_capacity(PAGE_ROWS as usize * ROW);
    for i in 0..PAGE_ROWS * W {
        let v = splitmix(seed.wrapping_add(u64::from(i))).to_le_bytes();
        px.extend_from_slice(&[v[0], v[1], v[2], 255]);
    }
    {
        let distinct: HashSet<&[u8]> = px.chunks(ROW).collect();
        assert_eq!(distinct.len(), PAGE_ROWS as usize, "page rows must be distinct");
    }
    px
}

/// Pseudo-random pixels from an xorshift stream, unrelated to any page.
fn noise(rows: u32, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut px = Vec::with_capacity((rows * W) as usize * 4);
    for _ in 0..rows * W {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        let b = state.to_le_bytes();
        px.extend_from_slice(&[b[0], b[1], b[2], 255]);
    }
    px
}

/// A solid block of `rows` rows.
fn solid(rows: u32, px: [u8; 4]) -> Vec<u8> {
    std::iter::repeat_n(px, (rows * W) as usize).flatten().collect()
}

fn bitmap(rows: u32, px: Vec<u8>) -> Bitmap {
    Bitmap::new(W, rows, PixelFormat::Rgba8, px.into()).unwrap()
}

/// An `H`-row frame showing the page from `offset`. `header` and `footer` replace the top and bottom
/// rows: they stay in place while the page scrolls, so the page shows only between them.
fn frame(page: &[u8], offset: u32, header: &[u8], footer: &[u8]) -> Bitmap {
    let head = (header.len() / ROW) as u32;
    let foot = (footer.len() / ROW) as u32;
    let mut px = header.to_vec();
    px.extend_from_slice(&page[(offset + head) as usize * ROW..(offset + H - foot) as usize * ROW]);
    px.extend_from_slice(footer);
    bitmap(H, px)
}

fn cut_frames(page: &[u8], offsets: &[u32], header: &[u8], footer: &[u8]) -> Vec<Bitmap> {
    offsets.iter().map(|&o| frame(page, o, header, footer)).collect()
}

/// Seeds a stitcher with the first frame and pushes the rest. Returns the result and each step.
fn stitch(frames: &[Bitmap], options: StitchOptions) -> (Bitmap, Vec<StitchStep>) {
    let mut stitcher = Stitcher::new(&frames[0], options);
    let steps = frames[1..].iter().map(|f| stitcher.push(f)).collect();
    (stitcher.result(), steps)
}

fn added(new_rows: u32) -> StitchStep {
    StitchStep::Added { new_rows }
}

/// Steps of a plain scroll over `OFFSETS`: eight of 37 rows, then one of 4.
fn plain_steps() -> Vec<StitchStep> {
    let mut steps = vec![added(37); 8];
    steps.push(added(4));
    steps
}

fn swap_rb(px: &[u8]) -> Vec<u8> {
    px.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0], p[3]]).collect()
}

#[test]
fn stitch_plain_scroll_rebuilds_the_page() {
    let page = make_page(1);
    let frames = cut_frames(&page, &OFFSETS, &[], &[]);
    let (result, steps) = stitch(&frames, StitchOptions::default());
    assert_eq!(steps, plain_steps());
    assert_eq!((result.width(), result.height()), (W, PAGE_ROWS));
    assert_eq!(result.format(), PixelFormat::Rgba8);
    assert_eq!(result.pixels(), page.as_slice());
}

#[test]
fn stitch_sticky_header_is_kept_once() {
    let page = make_page(2);
    let header = solid(10, [200, 30, 30, 255]);
    let frames = cut_frames(&page, &OFFSETS, &header, &[]);
    let options = StitchOptions { ignore_top: 10, ..Default::default() };
    let (result, steps) = stitch(&frames, options);
    assert_eq!(steps, plain_steps());
    // The header once at the top, then the page rows that scrolled under it.
    let mut expected = header.clone();
    expected.extend_from_slice(&page[10 * ROW..]);
    assert_eq!((result.width(), result.height()), (W, PAGE_ROWS));
    assert_eq!(result.pixels(), expected.as_slice());
}

#[test]
fn stitch_sticky_footer_is_added_once_at_the_end() {
    let page = make_page(3);
    let footer = solid(10, [30, 30, 200, 255]);
    let offsets = [0, 37, 74, 111, 148, 185, 222, 259, 296, 310];
    let frames = cut_frames(&page, &offsets, &[], &footer);
    let options = StitchOptions { ignore_bottom: 10, ..Default::default() };
    let (result, steps) = stitch(&frames, options);
    assert_eq!(steps, [vec![added(37); 8], vec![added(14)]].concat());
    let mut expected = page.clone();
    expected.extend_from_slice(&footer);
    assert_eq!((result.width(), result.height()), (W, PAGE_ROWS + 10));
    assert_eq!(result.pixels(), expected.as_slice());
}

#[test]
fn stitch_identical_frame_is_no_change() {
    let page = make_page(4);
    let frames = cut_frames(&page, &[0, 37], &[], &[]);
    let mut stitcher = Stitcher::new(&frames[0], StitchOptions::default());
    assert_eq!(stitcher.push(&frames[0]), StitchStep::NoChange);
    assert_eq!(stitcher.push(&frames[1]), added(37));
    let before = stitcher.result();
    // The page stopped scrolling: the same frame again changes nothing.
    assert_eq!(stitcher.push(&frames[1]), StitchStep::NoChange);
    assert_eq!(stitcher.result().pixels(), before.pixels());
}

#[test]
fn stitch_unrelated_frame_is_no_match() {
    let page = make_page(5);
    let first = frame(&page, 0, &[], &[]);
    let random = bitmap(H, noise(H, 0x9e37_79b9));
    let frames = [first.clone(), random.clone()];
    let (result, steps) = stitch(&frames, StitchOptions::default());
    assert_eq!(steps, vec![StitchStep::NoMatch]);
    // The unmatched frame is appended below the first one, as a seam.
    let mut expected = first.pixels().to_vec();
    expected.extend_from_slice(random.pixels());
    assert_eq!((result.width(), result.height()), (W, 2 * H));
    assert_eq!(result.pixels(), expected.as_slice());
}

#[test]
fn stitch_jump_past_the_frame_is_no_match() {
    let page = make_page(6);
    let frames = cut_frames(&page, &[0, 150], &[], &[]);
    let (result, steps) = stitch(&frames, StitchOptions::default());
    assert_eq!(steps, vec![StitchStep::NoMatch]);
    assert_eq!(result.height(), 2 * H);
}

#[test]
fn stitch_overlap_below_min_overlap_is_no_match() {
    let page = make_page(7);
    // Five rows overlap: fewer than the default minimum of eight.
    let frames = cut_frames(&page, &[0, 95], &[], &[]);
    let (_, steps) = stitch(&frames, StitchOptions::default());
    assert_eq!(steps, vec![StitchStep::NoMatch]);
    let (result, steps) = stitch(&frames, StitchOptions { min_overlap: 4, ..Default::default() });
    assert_eq!(steps, vec![added(95)]);
    assert_eq!(result.pixels(), &page[..(95 + H) as usize * ROW]);
}

#[test]
fn stitch_match_may_start_below_the_top_up_to_max_search() {
    let page = make_page(8);
    // A caret-like row at the top of the new frame blocks a match that starts at the top.
    let mut second = frame(&page, 37, &[], &[]).pixels().to_vec();
    let caret: Vec<u8> = [1u8, 2, 3, 255].repeat(W as usize);
    second[..ROW].copy_from_slice(&caret);
    let frames = [frame(&page, 0, &[], &[]), bitmap(H, second)];

    let (_, steps) = stitch(&frames, StitchOptions { max_search: 0, ..Default::default() });
    assert_eq!(steps, vec![StitchStep::NoMatch]);

    let (result, steps) = stitch(&frames, StitchOptions { max_search: 1, ..Default::default() });
    assert_eq!(steps, vec![added(37)]);
    assert_eq!(result.pixels(), &page[..137 * ROW]);
}

#[test]
fn stitch_bgra_frames_give_an_rgba_result() {
    let page = make_page(9);
    let frames: Vec<Bitmap> = OFFSETS
        .iter()
        .map(|&o| {
            let rgba = frame(&page, o, &[], &[]);
            Bitmap::new(W, H, PixelFormat::Bgra8, swap_rb(rgba.pixels()).into()).unwrap()
        })
        .collect();
    let (result, steps) = stitch(&frames, StitchOptions::default());
    assert_eq!(steps, plain_steps());
    assert_eq!(result.format(), PixelFormat::Rgba8);
    assert_eq!(result.pixels(), page.as_slice());
}

#[test]
fn stitch_single_frame_is_the_result() {
    let page = make_page(10);
    let first = frame(&page, 0, &[], &[]);
    let stitcher = Stitcher::new(&first, StitchOptions::default());
    assert_eq!(stitcher.result().pixels(), first.pixels());
}

#[test]
fn stitch_frame_of_another_size_is_refused() {
    let page = make_page(11);
    let first = frame(&page, 0, &[], &[]);
    let mut stitcher = Stitcher::new(&first, StitchOptions::default());
    let narrow = Bitmap::new(W / 2, H, PixelFormat::Rgba8, vec![0u8; (W / 2) as usize * H as usize * 4].into()).unwrap();
    assert_eq!(stitcher.push(&narrow), StitchStep::NoMatch);
    assert_eq!(stitcher.result().pixels(), first.pixels());
}

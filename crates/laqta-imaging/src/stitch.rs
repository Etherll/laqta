//! Scrolling-capture stitching. Each frame is matched against the
//! image built so far: the bottom rows of the last frame must reappear in the new frame, at its top
//! or a few rows lower (`max_search`), and only the rows below that overlap are appended.
//!
//! Rows are compared by a 64-bit hash of their RGB values (alpha is left out). Screen captures are
//! pixel-exact, so an overlap is either found exactly or not at all.
//!
//! Two rules follow ShareX's `ScrollingCaptureManager`. A strip at each side of the frame is left
//! out of the comparison, because a scrollbar there moves with every scroll and would break every
//! match. And a frame with no overlap is joined at the offset of the best overlap seen so far (the
//! page most likely scrolled the same distance), so one bad frame does not end the capture.

use laqta_domain::{Bitmap, PixelFormat};

use crate::ops::to_rgba;

/// How frames are matched and joined.
#[derive(Clone, Debug, PartialEq)]
pub struct StitchOptions {
    /// Rows at the top that stay in place while the page scrolls (a sticky header). Kept from the
    /// first frame only.
    pub ignore_top: u32,
    /// Rows at the bottom that stay in place (a sticky footer). Added once, after the last joined
    /// frame, by [`Stitcher::result`].
    pub ignore_bottom: u32,
    /// The fewest rows that count as an overlap. 0 is treated as 1.
    pub min_overlap: u32,
    /// How far below the top of the content a match may start, in rows. 0 allows a match only at
    /// the top. The default allows a match to start anywhere.
    pub max_search: u32,
}

impl Default for StitchOptions {
    fn default() -> Self {
        Self { ignore_top: 0, ignore_bottom: 0, min_overlap: 8, max_search: u32::MAX }
    }
}

/// What [`Stitcher::push`] did with a frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StitchStep {
    /// The frame overlapped the image, and `new_rows` rows were appended below it.
    Added { new_rows: u32 },
    /// The frame shows no new rows: it equals the last frame, or its content lies within it. The
    /// page has stopped scrolling. Nothing changed.
    NoChange,
    /// No overlap of `min_overlap` rows was found. The frame's content rows were appended without a
    /// join, so the result has a seam. The caller should stop.
    NoMatch,
}

/// Joins the frames of one scrolling capture into a single tall image.
#[derive(Debug)]
pub struct Stitcher {
    width: u32,
    height: u32,
    top: u32,
    bottom: u32,
    min_overlap: usize,
    max_search: usize,
    /// Joined rows, RGBA, from the top of the first frame down to the last joined row. Excludes the
    /// bottom ignore rows of the last frame, which [`Stitcher::result`] adds.
    image: Vec<u8>,
    image_rows: u32,
    /// The last frame, RGBA.
    last: Bitmap,
    /// Hashes of the content rows of `last`.
    last_hashes: Vec<u64>,
    /// The longest overlap found so far, as (rows of the new frame it ended at, its length).
    best: Option<(usize, usize)>,
}

impl Stitcher {
    /// Starts an image from `first`. Every later frame must be the same size.
    pub fn new(first: &Bitmap, options: StitchOptions) -> Self {
        let last = to_rgba(first);
        let (width, height) = (last.width(), last.height());
        let top = options.ignore_top.min(height);
        let bottom = options.ignore_bottom.min(height - top);
        let stride = width as usize * 4;
        let content_end = height - bottom;
        let px = last.pixels();
        let image = px[..content_end as usize * stride].to_vec();
        let last_hashes = row_hashes(px, width, top, content_end);
        Self {
            best: None,
            width,
            height,
            top,
            bottom,
            min_overlap: options.min_overlap.max(1) as usize,
            max_search: options.max_search as usize,
            image,
            image_rows: content_end,
            last,
            last_hashes,
        }
    }

    /// Joins the next frame of the scroll. A frame of another size is refused: nothing is appended
    /// and the result is `NoMatch`.
    pub fn push(&mut self, frame: &Bitmap) -> StitchStep {
        if frame.width() != self.width || frame.height() != self.height {
            return StitchStep::NoMatch;
        }
        let frame = to_rgba(frame);
        let stride = self.stride();
        let content_end = self.height - self.bottom;
        let px = frame.pixels();
        let hashes = row_hashes(px, self.width, self.top, content_end);
        if hashes == self.last_hashes {
            return StitchStep::NoChange;
        }
        let overlap = find_overlap(&self.last_hashes, &hashes, self.min_overlap, self.max_search);
        if let Some((start, len)) = overlap
            && self.best.is_none_or(|(_, best_len)| len > best_len)
        {
            self.best = Some((start + len, len));
        }
        // No overlap: join where the best overlap so far ended, as ShareX's best guess does.
        let guess = self.best.map(|(end, _)| end).filter(|&end| end < hashes.len());
        let joined = overlap.map(|(start, len)| start + len).or(guess);
        let from = joined.unwrap_or(0);
        let new_rows = hashes.len() - from;
        if overlap.is_some() && new_rows == 0 {
            return StitchStep::NoChange;
        }
        let from_px = (self.top as usize + from) * stride;
        let to_px = content_end as usize * stride;
        self.image.extend_from_slice(&px[from_px..to_px]);
        self.image_rows += new_rows as u32;
        self.last = frame;
        self.last_hashes = hashes;
        if joined.is_some() { StitchStep::Added { new_rows: new_rows as u32 } } else { StitchStep::NoMatch }
    }

    /// The stitched image, Rgba8: the joined rows, then the last frame's bottom ignore rows.
    pub fn result(&self) -> Bitmap {
        let footer = &self.last.pixels()[(self.height - self.bottom) as usize * self.stride()..];
        let mut pixels = Vec::with_capacity(self.image.len() + footer.len());
        pixels.extend_from_slice(&self.image);
        pixels.extend_from_slice(footer);
        Bitmap::new(self.width, self.image_rows + self.bottom, PixelFormat::Rgba8, pixels.into())
            .unwrap_or_else(|e| unreachable!("stitched rows are whole frame rows: {e:?}"))
    }

    fn stride(&self) -> usize {
        self.width as usize * 4
    }
}

/// The overlap with the most rows, as `(start, len)`: the last `len` rows of `last` equal
/// `new[start..start + len]`, with `len >= min_overlap` and `start <= max_search`. Ties go to the
/// earlier start.
///
/// For each end `e` of `new`, the longest run of equal rows ending there is the largest overlap
/// that ends at `e`, and it starts at the smallest `start` for that end. So one pass over `new` finds
/// the best overlap over all starts.
fn find_overlap(last: &[u64], new: &[u64], min_overlap: usize, max_search: usize) -> Option<(usize, usize)> {
    let (a, b) = (last.len(), new.len());
    let mut best: Option<(usize, usize)> = None;
    for end in 1..=b {
        let len = (0..a.min(end)).take_while(|&i| new[end - 1 - i] == last[a - 1 - i]).count();
        let start = end - len;
        if len >= min_overlap && start <= max_search && best.is_none_or(|(_, best_len)| len > best_len) {
            best = Some((start, len));
        }
    }
    best
}

/// The columns left out of the comparison at each side of a frame `width` wide: a twentieth of the
/// width and at least 50 pixels, but never more than a third (ShareX's `ignoreSideOffset`).
fn side_margin(width: u32) -> usize {
    (width / 20).max(50).min(width / 3) as usize
}

/// Hashes of frame rows `from..to`, RGB only, without the side margins.
fn row_hashes(px: &[u8], width: u32, from: u32, to: u32) -> Vec<u64> {
    let (stride, side) = (width as usize * 4, side_margin(width) * 4);
    (from..to)
        .map(|y| {
            let start = y as usize * stride;
            row_hash(&px[start + side..start + stride - side])
        })
        .collect()
}

/// FNV-1a over each pixel's RGB value (the fourth byte, alpha, is skipped), then a splitmix64 mix so
/// the last multiplications reach every bit.
fn row_hash(row: &[u8]) -> u64 {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut z = row.as_chunks::<4>().0.iter().fold(FNV_OFFSET, |h, px| {
        let rgb = u64::from(px[0]) | (u64::from(px[1]) << 8) | (u64::from(px[2]) << 16);
        (h ^ rgb).wrapping_mul(FNV_PRIME)
    });
    z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    z ^ (z >> 31)
}

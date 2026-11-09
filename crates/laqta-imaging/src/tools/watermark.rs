//! Draws a mark image over a picture.

use laqta_domain::Bitmap;
use laqta_domain::effects::draw::Placement;

use super::{over, rgba_bitmap};
use crate::ops;

/// Draws `mark` over `img` with source-over. `placement` anchors the mark in the picture as the effects overlay does,
/// and `offset` moves it by pixels (+x right, +y down). Parts outside the picture are clipped. `opacity` multiplies the
/// mark's alpha and is clamped to 0..=1 (NaN counts as 1). `scale_pct` sizes the mark as a percentage of its own size:
/// 100 keeps it, and a value that is not positive or not finite keeps it too.
///
/// The result is Rgba8 at the picture's size. Runs on the compute pool, because a scaled mark is resampled.
pub fn watermark_image(img: &Bitmap, mark: &Bitmap, placement: Placement, offset: (i32, i32), opacity: f32, scale_pct: f32) -> Bitmap {
    debug_assert!(rayon::current_thread_index().is_some(), "watermark_image must run inside the compute pool");
    let base = ops::to_rgba(img);
    let mut mark = ops::to_rgba(mark);
    if base.width() == 0 || base.height() == 0 || mark.width() == 0 || mark.height() == 0 {
        return base;
    }
    if scale_pct.is_finite() && scale_pct > 0.0 && scale_pct != 100.0 {
        mark = ops::resize(&mark, percent(mark.width(), scale_pct), percent(mark.height(), scale_pct));
    }
    let opacity = if opacity.is_nan() { 1.0 } else { opacity.clamp(0.0, 1.0) };

    let (bw, bh) = (i64::from(base.width()), i64::from(base.height()));
    let (mw, mh) = (i64::from(mark.width()), i64::from(mark.height()));
    let (ax, ay) = anchor(placement, (bw, bh), (mw, mh));
    let (x0, y0) = (ax + i64::from(offset.0), ay + i64::from(offset.1));

    let mut pixels = base.pixels().to_vec();
    let dst = pixels.as_chunks_mut::<4>().0;
    let row_len = mark.width() as usize;
    for (sy, src_row) in mark.pixels().as_chunks::<4>().0.chunks_exact(row_len).enumerate() {
        let dy = y0 + sy as i64;
        if !(0..bh).contains(&dy) {
            continue;
        }
        for (sx, s) in src_row.iter().enumerate() {
            let dx = x0 + sx as i64;
            if (0..bw).contains(&dx) {
                let d = &mut dst[dy as usize * base.width() as usize + dx as usize];
                *d = over(*d, *s, opacity);
            }
        }
    }
    rgba_bitmap(base.width(), base.height(), pixels)
}

/// `len` scaled by `pct` percent, rounded, and never below 1.
fn percent(len: u32, pct: f32) -> u32 {
    (len as f32 * pct / 100.0).round().max(1.0) as u32
}

/// The top-left corner of an item of `item` size in a `canvas`, for one of nine placements.
fn anchor(placement: Placement, (cw, ch): (i64, i64), (iw, ih): (i64, i64)) -> (i64, i64) {
    let x = match placement {
        Placement::TopLeft | Placement::MiddleLeft | Placement::BottomLeft => 0,
        Placement::TopCenter | Placement::MiddleCenter | Placement::BottomCenter => (cw - iw) / 2,
        Placement::TopRight | Placement::MiddleRight | Placement::BottomRight => cw - iw,
    };
    let y = match placement {
        Placement::TopLeft | Placement::TopCenter | Placement::TopRight => 0,
        Placement::MiddleLeft | Placement::MiddleCenter | Placement::MiddleRight => (ch - ih) / 2,
        Placement::BottomLeft | Placement::BottomCenter | Placement::BottomRight => ch - ih,
    };
    (x, y)
}

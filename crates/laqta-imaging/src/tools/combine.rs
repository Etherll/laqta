//! Joins images into one strip, side by side or stacked.

use laqta_domain::Bitmap;
use laqta_domain::color::Rgba;
use rayon::prelude::*;

use super::{over, rgba_bitmap};
use crate::ops;

/// `Horizontal` puts the images side by side; `Vertical` stacks them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Orientation {
    Horizontal,
    Vertical,
}

/// Where an image sits across the strip when it is shorter (in a row) or narrower (in a column) than the strip.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Align {
    Start,
    Center,
    End,
}

/// Joins `images` in order with `spacing` pixels between neighbours. Gaps, uncovered space and transparent
/// pixels of an image show `background`.
///
/// A row is as tall as its tallest image, and a column as wide as its widest. With `auto_fill_to_largest`, an
/// image whose cross size is smaller than the largest is scaled up to that size, keeping its aspect ratio.
/// Otherwise `align` places the smaller images. No images, or a strip with no area, gives a 1x1 transparent bitmap.
///
/// # Panics
/// If the strip, or a scaled image, does not fit in `u32` pixels along an axis.
pub fn combine(images: &[Bitmap], o: Orientation, align: Align, spacing: u32, background: Rgba, auto_fill_to_largest: bool) -> Bitmap {
    debug_assert!(rayon::current_thread_index().is_some(), "combine must run inside the compute pool");
    let Some(strip) = images.iter().map(|img| cross_len(o, img)).max() else {
        return rgba_bitmap(1, 1, vec![0; 4]);
    };
    let parts: Vec<Bitmap> = images.iter().map(|img| fit_cross(ops::to_rgba(img), o, strip, auto_fill_to_largest)).collect();

    let gaps = u64::from(spacing) * (parts.len() as u64 - 1);
    let along: u64 = parts.iter().map(|p| u64::from(main_len(o, p))).sum::<u64>() + gaps;
    let along = u32::try_from(along).expect("combined strip fits in u32");
    let (width, height) = match o {
        Orientation::Horizontal => (along, strip),
        Orientation::Vertical => (strip, along),
    };
    if width == 0 || height == 0 {
        return rgba_bitmap(1, 1, vec![0; 4]);
    }

    let mut placed = Vec::with_capacity(parts.len());
    let mut pos = 0i64;
    for part in &parts {
        let across = align_offset(align, strip, cross_len(o, part));
        let (x, y) = match o {
            Orientation::Horizontal => (pos, across),
            Orientation::Vertical => (across, pos),
        };
        placed.push((x, y, part));
        pos += i64::from(main_len(o, part)) + i64::from(spacing);
    }

    let (w, h) = (width as usize, height as usize);
    let mut canvas = [background.r, background.g, background.b, background.a].repeat(w * h);
    canvas.par_chunks_mut(w * 4).enumerate().for_each(|(y, row)| {
        let row = row.as_chunks_mut::<4>().0;
        for &(x0, y0, part) in &placed {
            let sy = y as i64 - y0;
            if !(0..i64::from(part.height())).contains(&sy) {
                continue;
            }
            let line = part.width() as usize * 4;
            let start = sy as usize * line;
            for (sx, s) in part.pixels()[start..start + line].as_chunks::<4>().0.iter().enumerate() {
                let dx = x0 + sx as i64;
                if (0..w as i64).contains(&dx) {
                    let d = &mut row[dx as usize];
                    *d = over(*d, *s, 1.0);
                }
            }
        }
    });
    rgba_bitmap(width, height, canvas)
}

fn main_len(o: Orientation, img: &Bitmap) -> u32 {
    match o {
        Orientation::Horizontal => img.width(),
        Orientation::Vertical => img.height(),
    }
}

fn cross_len(o: Orientation, img: &Bitmap) -> u32 {
    match o {
        Orientation::Horizontal => img.height(),
        Orientation::Vertical => img.width(),
    }
}

/// `img`, scaled along the strip so its cross size is `strip`, when `auto_fill` is set and it is smaller.
fn fit_cross(img: Bitmap, o: Orientation, strip: u32, auto_fill: bool) -> Bitmap {
    let cross = cross_len(o, &img);
    if !auto_fill || cross == 0 || cross >= strip {
        return img;
    }
    let main = u64::from(main_len(o, &img));
    let grown = (main * u64::from(strip) + u64::from(cross) / 2) / u64::from(cross);
    let grown = u32::try_from(grown.max(1)).expect("scaled image fits in u32");
    let (w, h) = match o {
        Orientation::Horizontal => (grown, strip),
        Orientation::Vertical => (strip, grown),
    };
    ops::resize(&img, w, h)
}

/// Where an item of `inner` size starts within an `outer` span.
fn align_offset(align: Align, outer: u32, inner: u32) -> i64 {
    let slack = i64::from(outer) - i64::from(inner);
    match align {
        Align::Start => 0,
        Align::Center => slack / 2,
        Align::End => slack,
    }
}

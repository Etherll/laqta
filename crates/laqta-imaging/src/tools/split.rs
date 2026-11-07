//! Splits an image into tiles, by a grid or by tile size (M5 image splitter).

use laqta_domain::{Bitmap, PhysRect};

use crate::ops;

/// Cuts `img` into `rows` x `cols` tiles, returned row by row. Each row and column gets an equal share of the image,
/// and the remainder pixels go to the last row or column. The counts are clamped to 1 and to the image's size, so
/// every tile is at least 1 pixel on each side. An empty image gives no tiles.
pub fn split_grid(img: &Bitmap, rows: u32, cols: u32) -> Vec<Bitmap> {
    if img.width() == 0 || img.height() == 0 {
        return Vec::new();
    }
    let rows = rows.clamp(1, img.height());
    let cols = cols.clamp(1, img.width());
    let mut tiles = Vec::with_capacity(rows as usize * cols as usize);
    for (y0, y1) in spans(img.height(), rows) {
        for (x0, x1) in spans(img.width(), cols) {
            tiles.push(ops::crop(img, PhysRect { x: x0 as i32, y: y0 as i32, width: x1 - x0, height: y1 - y0 }));
        }
    }
    tiles
}

/// Cuts `img` into tiles of `width` x `height` pixels, returned row by row. The tiles on the right and bottom edges are
/// smaller when the size does not divide the image. A side of 0 keeps the whole image along that axis. An empty image
/// gives no tiles.
pub fn split_size(img: &Bitmap, width: u32, height: u32) -> Vec<Bitmap> {
    if img.width() == 0 || img.height() == 0 {
        return Vec::new();
    }
    let tile_w = if width == 0 { img.width() } else { width };
    let tile_h = if height == 0 { img.height() } else { height };
    let mut tiles = Vec::new();
    for y in (0..img.height()).step_by(tile_h as usize) {
        for x in (0..img.width()).step_by(tile_w as usize) {
            tiles.push(ops::crop(img, PhysRect { x: x as i32, y: y as i32, width: tile_w, height: tile_h }));
        }
    }
    tiles
}

/// Equal parts of `len` as half-open `(start, end)` spans. The last part takes the remainder. `parts` is at least 1
/// and at most `len`, so no part is empty.
fn spans(len: u32, parts: u32) -> Vec<(u32, u32)> {
    let step = len / parts;
    (0..parts).map(|i| (i * step, if i + 1 == parts { len } else { (i + 1) * step })).collect()
}

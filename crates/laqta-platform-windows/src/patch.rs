//! Copies of rectangular patches of a frame's pixels, used to draw the cursor onto a frame. Frames
//! are tightly packed Bgra8 rows; patch rects are frame-local and may reach past the frame's edges.

use laqta_domain::PhysRect;

const BYTES_PER_PIXEL: usize = 4;

/// The part of `a` that lies inside `b`, or `None` when they do not overlap.
pub(crate) fn overlap(a: PhysRect, b: PhysRect) -> Option<PhysRect> {
    let x0 = a.x.max(b.x);
    let y0 = a.y.max(b.y);
    let x1 = a.right().min(b.right());
    let y1 = a.bottom().min(b.bottom());
    let width = u32::try_from(i64::from(x1) - i64::from(x0))
        .ok()
        .filter(|&w| w > 0)?;
    let height = u32::try_from(i64::from(y1) - i64::from(y0))
        .ok()
        .filter(|&h| h > 0)?;
    Some(PhysRect {
        x: x0,
        y: y0,
        width,
        height,
    })
}

/// Copies the pixels of `rect` out of `src` (a `src_width` x `src_height` frame). Pixels of `rect`
/// outside the frame come back as zeros.
pub(crate) fn extract(src: &[u8], src_width: u32, src_height: u32, rect: PhysRect) -> Vec<u8> {
    let mut out = vec![0; rect.width as usize * rect.height as usize * BYTES_PER_PIXEL];
    let frame = PhysRect {
        x: 0,
        y: 0,
        width: src_width,
        height: src_height,
    };
    let Some(hit) = overlap(rect, frame) else {
        return out;
    };
    let row_bytes = hit.width as usize * BYTES_PER_PIXEL;
    let (src_x, src_y) = (hit.x as usize, hit.y as usize);
    let (out_x, out_y) = ((hit.x - rect.x) as usize, (hit.y - rect.y) as usize);
    for row in 0..hit.height as usize {
        let from = ((src_y + row) * src_width as usize + src_x) * BYTES_PER_PIXEL;
        let to = ((out_y + row) * rect.width as usize + out_x) * BYTES_PER_PIXEL;
        out[to..to + row_bytes].copy_from_slice(&src[from..from + row_bytes]);
    }
    out
}

/// Writes `patch` (the pixels of `rect`, as `extract` returns them) back into `dst`, clipped to the
/// frame. Only the colour bytes are written; each destination pixel keeps its alpha.
pub(crate) fn write_back(
    dst: &mut [u8],
    dst_width: u32,
    dst_height: u32,
    rect: PhysRect,
    patch: &[u8],
) {
    let frame = PhysRect {
        x: 0,
        y: 0,
        width: dst_width,
        height: dst_height,
    };
    let Some(hit) = overlap(rect, frame) else {
        return;
    };
    let (dst_x, dst_y) = (hit.x as usize, hit.y as usize);
    let (patch_x, patch_y) = ((hit.x - rect.x) as usize, (hit.y - rect.y) as usize);
    for row in 0..hit.height as usize {
        for col in 0..hit.width as usize {
            let from = ((patch_y + row) * rect.width as usize + patch_x + col) * BYTES_PER_PIXEL;
            let to = ((dst_y + row) * dst_width as usize + dst_x + col) * BYTES_PER_PIXEL;
            dst[to..to + 3].copy_from_slice(&patch[from..from + 3]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: u32, height: u32) -> PhysRect {
        PhysRect {
            x,
            y,
            width,
            height,
        }
    }

    /// A `width` x `height` frame whose pixel (x, y) has blue = x and green = y, red = 0, alpha 255.
    fn gradient(width: u32, height: u32) -> Vec<u8> {
        let mut px = Vec::new();
        for y in 0..height {
            for x in 0..width {
                px.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        px
    }

    #[test]
    fn overlap_clips_to_both_rects() {
        let frame = rect(0, 0, 4, 4);
        assert_eq!(overlap(rect(-2, 3, 4, 4), frame), Some(rect(0, 3, 2, 1)));
        assert_eq!(overlap(rect(1, 1, 2, 2), frame), Some(rect(1, 1, 2, 2)));
        assert_eq!(
            overlap(rect(4, 0, 2, 2), frame),
            None,
            "touching edges do not overlap"
        );
        assert_eq!(overlap(rect(-10, -10, 3, 3), frame), None);
    }

    #[test]
    fn extract_copies_an_inside_rect() {
        let src = gradient(4, 4);
        let out = extract(&src, 4, 4, rect(1, 2, 2, 1));
        assert_eq!(out, vec![1, 2, 0, 255, 2, 2, 0, 255]);
    }

    #[test]
    fn extract_zero_fills_past_the_edges() {
        let src = [5u8, 6, 7, 8].repeat(4);
        // The patch's top-left pixel sits at (-1, -1), so only its bottom-right pixel is in the frame.
        let out = extract(&src, 2, 2, rect(-1, -1, 2, 2));
        assert_eq!(out, [vec![0; 12], vec![5, 6, 7, 8]].concat());
    }

    #[test]
    fn extract_outside_the_frame_is_all_zeros() {
        let src = gradient(2, 2);
        assert_eq!(extract(&src, 2, 2, rect(5, 5, 2, 2)), vec![0; 16]);
    }

    #[test]
    fn write_back_clips_to_a_negative_origin() {
        let mut dst = vec![9; 2 * 2 * 4];
        let patch = vec![1, 2, 3, 0, 4, 5, 6, 0, 7, 8, 9, 0, 10, 11, 12, 0];
        write_back(&mut dst, 2, 2, rect(-1, -1, 2, 2), &patch);
        // Only the patch's bottom-right pixel lands inside the frame, at (0, 0).
        assert_eq!(&dst[0..3], &[10, 11, 12]);
        assert_eq!(dst[3], 9, "alpha is kept from the destination");
        assert_eq!(&dst[4..], &[9; 12]);
    }

    #[test]
    fn write_back_keeps_destination_alpha_and_bounds() {
        let mut dst = gradient(3, 3);
        let patch = vec![50, 60, 70, 0, 50, 60, 70, 0, 50, 60, 70, 0, 50, 60, 70, 0];
        write_back(&mut dst, 3, 3, rect(2, 2, 2, 2), &patch);
        let pixel = |x: usize, y: usize| {
            let i = (y * 3 + x) * 4;
            [dst[i], dst[i + 1], dst[i + 2], dst[i + 3]]
        };
        assert_eq!(pixel(2, 2), [50, 60, 70, 255]);
        assert_eq!(
            pixel(1, 1),
            [1, 1, 0, 255],
            "pixels outside the patch are untouched"
        );
        assert_eq!(pixel(2, 1), [2, 1, 0, 255]);
    }

    #[test]
    fn extract_then_write_back_round_trips() {
        let src = gradient(5, 4);
        let area = rect(3, 2, 3, 3);
        let patch = extract(&src, 5, 4, area);
        let mut dst = vec![0; 5 * 4 * 4];
        write_back(&mut dst, 5, 4, area, &patch);
        // Inside the frame the copy matches the source colours.
        for y in 2..4 {
            for x in 3..5 {
                let i = (y * 5 + x) * 4;
                assert_eq!(&dst[i..i + 3], &src[i..i + 3]);
            }
        }
    }
}

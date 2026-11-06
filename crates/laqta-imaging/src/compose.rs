use laqta_domain::{Bitmap, Frame, PhysRect, PixelFormat};
use rayon::prelude::*;

/// Places each frame at `monitor.rect.(x, y)` minus the union's origin, on a transparent canvas the
/// size of the union. Converts BGRA to RGBA. Mixed-scale rule for M0: if frames' scales differ,
/// they are laid out left to right in order of `rect.x` with top edges aligned (M1 replaces this).
pub fn compose_monitors(frames: &[Frame]) -> Bitmap {
    debug_assert!(rayon::current_thread_index().is_some(), "compose_monitors must run inside the compute pool");
    let placements = layout(frames);
    let Some(union) = placements.iter().map(|(r, _)| *r).reduce(PhysRect::union) else {
        return empty();
    };
    let (w, h) = (union.width as usize, union.height as usize);
    let mut canvas = vec![0u8; w * h * 4];
    canvas.par_chunks_mut(w * 4).enumerate().for_each(|(row, line)| {
        let y = union.y + row as i32;
        for (rect, image) in &placements {
            if y < rect.y || y >= rect.bottom() {
                continue;
            }
            let sy = (y - rect.y) as usize;
            let iw = image.width() as usize;
            let cols = iw.min(rect.width as usize);
            let src = &image.pixels()[sy * iw * 4..sy * iw * 4 + cols * 4];
            let dx = (rect.x - union.x) as usize;
            let dst = &mut line[dx * 4..dx * 4 + cols * 4];
            match image.format() {
                PixelFormat::Rgba8 => dst.copy_from_slice(src),
                PixelFormat::Bgra8 => {
                    for (d, s) in dst.as_chunks_mut::<4>().0.iter_mut().zip(src.as_chunks::<4>().0) {
                        d[0] = s[2];
                        d[1] = s[1];
                        d[2] = s[0];
                        d[3] = s[3];
                    }
                }
            }
        }
    });
    Bitmap::new(union.width, union.height, PixelFormat::Rgba8, canvas.into()).unwrap_or_else(|_| empty())
}

fn empty() -> Bitmap {
    Bitmap::new(0, 0, PixelFormat::Rgba8, Vec::new().into()).unwrap_or_else(|_| unreachable!())
}

/// Each frame's destination rect (sized to its image) and its image.
fn layout(frames: &[Frame]) -> Vec<(PhysRect, &Bitmap)> {
    let mixed = frames.windows(2).any(|w| w[0].monitor.scale != w[1].monitor.scale);
    if !mixed {
        return frames
            .iter()
            .map(|f| {
                let r = f.monitor.rect;
                (PhysRect { x: r.x, y: r.y, width: f.image.width(), height: f.image.height() }, &f.image)
            })
            .collect();
    }
    let mut ordered: Vec<&Frame> = frames.iter().collect();
    ordered.sort_by_key(|f| f.monitor.rect.x);
    let top = ordered.iter().map(|f| f.monitor.rect.y).min().unwrap_or(0);
    let mut x = ordered.first().map(|f| f.monitor.rect.x).unwrap_or(0);
    ordered
        .into_iter()
        .map(|f| {
            let r = PhysRect { x, y: top, width: f.image.width(), height: f.image.height() };
            x = r.right();
            (r, &f.image)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;
    use laqta_domain::{Monitor, MonitorId};

    fn pool() -> rayon::ThreadPool {
        #[allow(clippy::disallowed_types)]
        rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap()
    }

    fn frame(x: i32, y: i32, w: u32, h: u32, scale: f32, px: [u8; 4], format: PixelFormat) -> Frame {
        let pixels: Vec<u8> = std::iter::repeat_n(px, (w * h) as usize).flatten().collect();
        Frame {
            monitor: Monitor { id: MonitorId(format!("{x}")), name: String::new(), rect: PhysRect { x, y, width: w, height: h }, scale, primary: x == 0 },
            image: Bitmap::new(w, h, format, pixels.into()).unwrap(),
        }
    }

    fn at(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
        let i = ((y * b.width() + x) * 4) as usize;
        b.pixels()[i..i + 4].try_into().unwrap()
    }

    #[test]
    fn two_monitors_side_by_side() {
        let frames = [
            frame(0, 0, 1920, 1080, 1.0, [1, 2, 3, 255], PixelFormat::Rgba8),
            frame(1920, 0, 1920, 1080, 1.0, [9, 8, 7, 255], PixelFormat::Rgba8),
        ];
        let out = pool().install(|| compose_monitors(&frames));
        assert_eq!((out.width(), out.height()), (3840, 1080));
        assert_eq!(out.format(), PixelFormat::Rgba8);
        assert_eq!(at(&out, 0, 0), [1, 2, 3, 255]);
        assert_eq!(at(&out, 1919, 1079), [1, 2, 3, 255]);
        assert_eq!(at(&out, 1920, 0), [9, 8, 7, 255]);
    }

    #[test]
    fn bgra_to_rgba_and_transparent_gaps() {
        let frames = [
            frame(0, 0, 4, 4, 1.0, [10, 20, 30, 255], PixelFormat::Bgra8),
            frame(4, 2, 4, 4, 1.0, [10, 20, 30, 255], PixelFormat::Rgba8),
        ];
        let out = pool().install(|| compose_monitors(&frames));
        assert_eq!((out.width(), out.height()), (8, 6));
        assert_eq!(at(&out, 0, 0), [30, 20, 10, 255]);
        assert_eq!(at(&out, 5, 0), [0, 0, 0, 0]);
        assert_eq!(at(&out, 5, 5), [10, 20, 30, 255]);
    }

    #[test]
    fn mixed_scale_left_to_right_top_aligned() {
        // Logical-coordinate rects that overlap once scaled; M0 rule lays them out by x.
        let frames = [
            frame(1000, 100, 4, 2, 2.0, [2, 2, 2, 255], PixelFormat::Rgba8),
            frame(0, 0, 3, 3, 1.0, [1, 1, 1, 255], PixelFormat::Rgba8),
        ];
        let out = pool().install(|| compose_monitors(&frames));
        assert_eq!((out.width(), out.height()), (7, 3));
        assert_eq!(at(&out, 0, 0), [1, 1, 1, 255]);
        assert_eq!(at(&out, 3, 0), [2, 2, 2, 255]);
        assert_eq!(at(&out, 3, 2), [0, 0, 0, 0]);
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "compute pool")]
    fn panics_outside_pool() {
        let frames = [frame(0, 0, 1, 1, 1.0, [0; 4], PixelFormat::Rgba8)];
        let _ = compose_monitors(&frames);
    }
}

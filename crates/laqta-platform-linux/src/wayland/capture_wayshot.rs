use std::sync::Arc;

use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::PortError;
use libwayshot::WayshotConnection;

use super::layout::{OutputLayout, max_scale, to_physical};

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("wayshot: {e}"))
}

/// The outputs' logical layout. wayshot reports no scale, so `scale` is 1.0 here; `capture_all`
/// fills it in from the captured pixels.
pub fn layout() -> Result<Vec<OutputLayout>, PortError> {
    let conn = WayshotConnection::new().map_err(fail)?;
    Ok(conn.get_all_outputs().iter().map(to_layout).collect())
}

/// Starts reading the outputs' layout on the blocking pool. A capture starts it first and awaits it
/// once it has its picture, so the Wayland round trip runs while the compositor takes the picture
/// instead of after it. It is read fresh every time: a layout kept from an earlier capture could not
/// tell two identical monitors that swapped places. A failed read is an empty layout (one frame).
pub fn layout_soon() -> blocking::Task<Vec<OutputLayout>> {
    blocking::unblock(|| layout().unwrap_or_default())
}

/// Whether a `w` x `h` picture is the layout's union at one uniform factor, which is what
/// `capture_portal::split` assumes. Rounded logical sizes leave a little slack.
pub(crate) fn fits(w: u32, h: u32, layout: &[OutputLayout]) -> bool {
    let Some(union) = layout.iter().map(|o| o.logical).reduce(PhysRect::union) else { return false };
    if w == 0 || h == 0 || union.width == 0 || union.height == 0 {
        return false;
    }
    let (fx, fy) = (f64::from(w) / f64::from(union.width), f64::from(h) / f64::from(union.height));
    (fx - fy).abs() <= 0.02 * fx.max(fy)
}

fn to_layout(o: &libwayshot::output::OutputInfo) -> OutputLayout {
    let r = o.logical_region.inner;
    OutputLayout {
        name: o.name.clone(),
        description: o.description.clone(),
        logical: PhysRect { x: r.position.x, y: r.position.y, width: r.size.width, height: r.size.height },
        scale: 1.0,
    }
}

/// Captures every output into physical virtual-desktop space. An output's scale is its
/// captured width over its logical width. The space uses the largest scale, so a frame whose
/// capture is smaller is resampled to its physical size.
pub fn capture_all() -> Result<Vec<Frame>, PortError> {
    let started = std::time::Instant::now();
    let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1000.0;
    let conn = WayshotConnection::new().map_err(fail)?;
    tracing::debug!(target: "laqta::perf", ms = ms(started), "wayshot connected");
    let outputs = conn.get_all_outputs().to_vec();
    if outputs.is_empty() {
        return Err(PortError::Failed("wayshot: no outputs".into()));
    }
    let mut layouts = Vec::with_capacity(outputs.len());
    let mut images = Vec::with_capacity(outputs.len());
    for o in &outputs {
        let mut layout = to_layout(o);
        let shot = std::time::Instant::now();
        let img = conn.screenshot_single_output(o, false).map_err(fail)?;
        tracing::debug!(target: "laqta::perf", ms = ms(shot), output = %o.name, "wayshot screenshot");
        let convert = std::time::Instant::now();
        let img = img.into_rgba8();
        tracing::debug!(target: "laqta::perf", ms = ms(convert), "wayshot into_rgba8");
        layout.scale = if layout.logical.width > 0 { img.width() as f32 / layout.logical.width as f32 } else { 1.0 };
        layouts.push(layout);
        images.push(img);
    }
    let s_max = max_scale(&layouts);
    let mut frames = Vec::with_capacity(layouts.len());
    for (i, (layout, img)) in layouts.iter().zip(images).enumerate() {
        let rect = to_physical(layout.logical, s_max);
        let (w, h) = (img.width(), img.height());
        let raw = img.into_raw();
        let rgba = if (w, h) == (rect.width, rect.height) { raw } else { scale_nearest(&raw, w, h, rect.width, rect.height) };
        let image = Bitmap::new(rect.width, rect.height, PixelFormat::Rgba8, Arc::from(rgba)).map_err(fail)?;
        frames.push(Frame {
            monitor: Monitor {
                id: MonitorId(layout.name.clone()),
                name: if layout.description.is_empty() { layout.name.clone() } else { layout.description.clone() },
                rect,
                scale: s_max,
                primary: i == 0,
            },
            image,
        });
    }
    tracing::debug!(target: "laqta::perf", ms = ms(started), "wayshot capture_all");
    Ok(frames)
}

/// Nearest-neighbour resize of tightly packed RGBA8 pixels from `w` x `h` to `new_w` x `new_h`.
pub(crate) fn scale_nearest(rgba: &[u8], w: u32, h: u32, new_w: u32, new_h: u32) -> Vec<u8> {
    let (w, h, new_w, new_h) = (w as usize, h as usize, new_w as usize, new_h as usize);
    if w == 0 || h == 0 {
        return vec![0; new_w * new_h * 4];
    }
    let mut out = Vec::with_capacity(new_w * new_h * 4);
    for y in 0..new_h {
        let row = (y * h / new_h) * w;
        for x in 0..new_w {
            let i = (row + x * w / new_w) * 4;
            out.extend_from_slice(&rgba[i..i + 4]);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn output(x: i32, width: u32, height: u32) -> OutputLayout {
        OutputLayout { name: String::new(), description: String::new(), logical: PhysRect { x, y: 0, width, height }, scale: 1.0 }
    }

    #[test]
    fn a_layout_fits_a_picture_of_its_union_at_one_factor() {
        let two = [output(0, 1920, 1080), output(1920, 1280, 1080)];
        assert!(fits(3200, 1080, &two), "factor 1");
        assert!(fits(6400, 2160, &two), "factor 2");
        assert!(fits(4800, 1620, &two), "factor 1.5");
        assert!(fits(3200, 1079, &two), "a rounded edge");
    }

    #[test]
    fn a_layout_that_does_not_match_the_picture_does_not_fit() {
        let one = [output(0, 1920, 1080)];
        assert!(!fits(3840, 1080, &one), "an output was added");
        assert!(!fits(1920, 1080, &[]), "no layout");
        assert!(!fits(0, 0, &one), "no picture");
        assert!(!fits(1920, 1080, &[output(0, 0, 0)]), "an empty output");
    }

    #[test]
    fn scale_nearest_upscales_by_integer_factor() {
        let px = [1, 2, 3, 255, 4, 5, 6, 255];
        // 2x1 → 4x1: each pixel doubled.
        assert_eq!(scale_nearest(&px, 2, 1, 4, 1), [1, 2, 3, 255, 1, 2, 3, 255, 4, 5, 6, 255, 4, 5, 6, 255]);
    }

    #[test]
    fn scale_nearest_handles_non_integer_ratio() {
        let px = [1, 1, 1, 255, 2, 2, 2, 255];
        // 2x1 → 3x1: source columns 0, 0, 1.
        assert_eq!(scale_nearest(&px, 2, 1, 3, 1), [1, 1, 1, 255, 1, 1, 1, 255, 2, 2, 2, 255]);
        // 1x2 → 1x3: source rows 0, 0, 1.
        assert_eq!(scale_nearest(&px, 1, 2, 1, 3), [1, 1, 1, 255, 1, 1, 1, 255, 2, 2, 2, 255]);
    }

    #[test]
    fn scale_nearest_identity() {
        let px = [9, 8, 7, 255, 6, 5, 4, 255, 3, 2, 1, 255, 0, 0, 0, 0];
        assert_eq!(scale_nearest(&px, 2, 2, 2, 2), px);
    }
}

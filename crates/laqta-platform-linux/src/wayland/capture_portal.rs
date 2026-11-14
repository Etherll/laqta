use std::path::PathBuf;
use std::sync::Arc;

use ashpd::desktop::screenshot::Screenshot;
use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};
use laqta_ports::PortError;

use super::capture_wayshot;
use super::layout::{OutputLayout, to_physical};

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("screenshot portal: {e}"))
}

/// Screenshot portal with `interactive = false`. The portal returns one composed PNG; it is decoded,
/// the file deleted, and the image split per output using the `wl_output` layout. No restore token
/// exists or is stored (ADR 019).
pub async fn capture_all() -> Result<Vec<Frame>, PortError> {
    let layout = capture_wayshot::layout_soon();
    let response = Screenshot::request()
        .interactive(false)
        .modal(false)
        .send()
        .await
        .map_err(|e| PortError::Unsupported(format!("screenshot portal: {e}")))?
        .response()
        .map_err(|e| match e {
            ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled) => PortError::Cancelled,
            other => fail(other),
        })?;
    let path = file_path(response.uri().as_str()).ok_or_else(|| fail("not a file URI"))?;
    let (w, h, rgba) = blocking::unblock(move || {
        let bytes = std::fs::read(&path).map_err(fail);
        let _ = std::fs::remove_file(&path);
        decode_png(&bytes?)
    })
    .await?;
    let layout = layout.await;
    blocking::unblock(move || Ok(split(w, h, PixelFormat::Rgba8, rgba, &layout))).await
}

/// `file:///a%20b.png` → `/a b.png`.
pub(crate) fn file_path(uri: &str) -> Option<PathBuf> {
    let rest = uri.strip_prefix("file://")?;
    let rest = rest.strip_prefix("localhost").unwrap_or(rest);
    let bytes = rest.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(b) = u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).ok()?, 16)
        {
            out.push(b);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(out)))
}

fn decode_png(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), PortError> {
    let mut dec = png::Decoder::new(std::io::Cursor::new(bytes));
    dec.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16 | png::Transformations::ALPHA);
    let mut reader = dec.read_info().map_err(fail)?;
    let mut buf = vec![0; reader.output_buffer_size().ok_or_else(|| fail("image too large"))?];
    let info = reader.next_frame(&mut buf).map_err(fail)?;
    buf.truncate(info.buffer_size());
    let rgba = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::GrayscaleAlpha => buf.as_chunks::<2>().0.iter().flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        other => return Err(fail(format!("unexpected colour type {other:?}"))),
    };
    Ok((info.width, info.height, rgba))
}

/// Splits the composed image into one frame per output. The image covers the union of the
/// logical rects at one uniform factor (image width / union width), which is every output's scale,
/// so physical rects are the logical rects times that factor. Without a layout, one frame. The
/// pixels stay in `format`; only whole rows are copied.
pub(crate) fn split(w: u32, h: u32, format: PixelFormat, rgba: Vec<u8>, layout: &[OutputLayout]) -> Vec<Frame> {
    let whole = |rgba: Vec<u8>| Frame {
        monitor: Monitor {
            id: MonitorId("portal".into()),
            name: "Screen".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, format, Arc::from(rgba)).unwrap_or_else(|_| unreachable!("decoder size")),
    };
    let Some(union) = layout.iter().map(|o| o.logical).reduce(PhysRect::union) else { return vec![whole(rgba)] };
    if union.width == 0 {
        return vec![whole(rgba)];
    }
    if !capture_wayshot::fits(w, h, layout) {
        tracing::warn!(w, h, ?union, "the outputs' layout does not match the picture; its frames may be cut wrongly");
    }
    let f = w as f64 / union.width as f64;
    let mut frames = Vec::new();
    for (i, o) in layout.iter().enumerate() {
        let x0 = (((o.logical.x - union.x) as f64) * f).round() as u32;
        let y0 = (((o.logical.y - union.y) as f64) * f).round() as u32;
        let cw = ((o.logical.width as f64 * f).round() as u32).min(w.saturating_sub(x0));
        let ch = ((o.logical.height as f64 * f).round() as u32).min(h.saturating_sub(y0));
        if cw == 0 || ch == 0 {
            continue;
        }
        let mut px = Vec::with_capacity((cw * ch * 4) as usize);
        for row in y0..y0 + ch {
            let s = ((row * w + x0) * 4) as usize;
            px.extend_from_slice(&rgba[s..s + (cw * 4) as usize]);
        }
        let Ok(image) = Bitmap::new(cw, ch, format, Arc::from(px)) else { continue };
        let at = to_physical(o.logical, f as f32);
        frames.push(Frame {
            monitor: Monitor {
                id: MonitorId(o.name.clone()),
                name: o.description.clone(),
                rect: PhysRect { x: at.x, y: at.y, width: cw, height: ch },
                scale: f as f32,
                primary: i == 0,
            },
            image,
        });
    }
    if frames.is_empty() { vec![whole(rgba)] } else { frames }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn out(name: &str, x: i32, width: u32, height: u32) -> OutputLayout {
        OutputLayout { name: name.into(), description: name.to_lowercase(), logical: PhysRect { x, y: 0, width, height }, scale: 1.0 }
    }

    #[test]
    fn uri_to_path() {
        assert_eq!(file_path("file:///tmp/a%20b.png"), Some(PathBuf::from("/tmp/a b.png")));
        assert_eq!(file_path("https://x"), None);
    }

    #[test]
    fn split_two_outputs_at_scale_two() {
        let layout = [out("A", 0, 2, 1), out("B", 2, 1, 1)];
        let (w, h) = (6, 2);
        let rgba: Vec<u8> = (0..w * h).flat_map(|i| [i as u8, 0, 0, 255]).collect();
        let frames = split(w, h, PixelFormat::Rgba8, rgba, &layout);
        assert_eq!(frames.len(), 2);
        assert_eq!((frames[0].image.width(), frames[0].image.height()), (4, 2));
        assert_eq!((frames[1].image.width(), frames[1].image.height()), (2, 2));
        assert_eq!(frames[1].image.pixels()[0], 4);
        assert_eq!(frames[0].monitor.rect, PhysRect { x: 0, y: 0, width: 4, height: 2 });
        assert_eq!(frames[1].monitor.rect, PhysRect { x: 4, y: 0, width: 2, height: 2 });
        assert_eq!(frames[1].monitor.scale, 2.0);
    }

    #[test]
    fn split_places_frames_from_the_logical_origin() {
        // Union (-2, 0, 3, 1) at factor 6 / 3 = 2: A starts left of the origin.
        let layout = [out("A", -2, 2, 1), out("B", 0, 1, 1)];
        let frames = split(6, 2, PixelFormat::Rgba8, vec![0; 6 * 2 * 4], &layout);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].monitor.rect, PhysRect { x: -4, y: 0, width: 4, height: 2 });
        assert_eq!(frames[1].monitor.rect, PhysRect { x: 0, y: 0, width: 2, height: 2 });
    }

    #[test]
    fn split_keeps_the_pixel_format_and_bytes() {
        let layout = [out("A", 0, 1, 1), out("B", 1, 1, 1)];
        let bgra = vec![3, 2, 1, 255, 6, 5, 4, 255];
        let frames = split(2, 1, PixelFormat::Bgra8, bgra, &layout);
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|f| f.image.format() == PixelFormat::Bgra8));
        assert_eq!(frames[0].image.pixels(), [3, 2, 1, 255]);
        assert_eq!(frames[1].image.pixels(), [6, 5, 4, 255]);
        let whole = split(2, 1, PixelFormat::Bgra8, vec![0; 8], &[]);
        assert_eq!((whole.len(), whole[0].image.format()), (1, PixelFormat::Bgra8));
    }
}

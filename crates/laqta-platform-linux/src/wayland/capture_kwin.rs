//! KDE Plasma: the whole workspace from KWin's `ScreenShot2` D-Bus interface, without the pointer.
//! Plasma's Screenshot portal always draws the pointer into the picture, so the frozen frame under
//! the region overlay showed a second, stuck pointer. KWin writes raw pixels into a pipe and
//! describes them in its reply; the picture is split per output like the portal's.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::{AsFd as _, OwnedFd};
use std::time::Duration;

use async_io::Async;
use futures_lite::AsyncReadExt as _;

use ashpd::zbus::Connection;
use ashpd::zbus::{Fd, OwnedVariant, Variant};
use futures_lite::future;
use laqta_domain::Frame;
use laqta_ports::PortError;

use super::{capture_portal, capture_wayshot};

/// How long KWin may take to answer and write the picture.
const BUDGET: Duration = Duration::from_secs(3);

fn fail(e: impl std::fmt::Display) -> PortError {
    PortError::Failed(format!("KWin screenshot: {e}"))
}

pub async fn capture_all() -> Result<Vec<Frame>, PortError> {
    future::or(capture(), async {
        async_io::Timer::after(BUDGET).await;
        Err(fail("no answer in time"))
    })
    .await
}

async fn capture() -> Result<Vec<Frame>, PortError> {
    let conn = Connection::session().await.map_err(fail)?;
    let (reader, writer) = std::io::pipe().map_err(fail)?;
    // Read on the reactor, not a blocking thread: when the time budget drops this future, the read
    // end closes with it instead of leaving a thread waiting on KWin.
    let mut reader = Async::new(File::from(OwnedFd::from(reader))).map_err(fail)?;
    let pixels = async move {
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).await.map(|_| buf)
    };
    let options: HashMap<&str, Variant<'_>> =
        HashMap::from([("include-cursor", Variant::from(false)), ("native-resolution", Variant::from(true))]);
    let call = async {
        let reply = conn
            .call_method(
                Some("org.kde.KWin"),
                "/org/kde/KWin/ScreenShot2",
                Some("org.kde.KWin.ScreenShot2"),
                "CaptureWorkspace",
                &(options, Fd::from(writer.as_fd())),
            )
            .await;
        // KWin holds its own copy of the write end; ours must go for the read to see the end.
        drop(writer);
        reply
    };
    // KWin may write before it replies, so the read runs from the start: a full pipe would stall it.
    let (reply, pixels) = future::zip(call, pixels).await;
    let results: HashMap<String, OwnedVariant> = reply.map_err(fail)?.body().deserialize().map_err(fail)?;
    let bytes = pixels.map_err(fail)?;
    let get = |key: &str| results.get(key).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| fail(format!("no {key} in the reply")));
    let (w, h, stride, format) = (get("width")?, get("height")?, get("stride")?, get("format")?);
    let rgba = to_rgba(format, w, h, stride, bytes).ok_or_else(|| fail(format!("unsupported image format {format}")))?;
    let layout = blocking::unblock(capture_wayshot::layout).await.unwrap_or_default();
    Ok(capture_portal::split(w, h, rgba, &layout))
}

/// Tightly packed, opaque RGBA from KWin's raw `QImage` bytes, converted in place so a workspace
/// of several 4K outputs is never held twice. `format` is the `QImage::Format` number; the 32-bit
/// formats KWin uses are handled, anything else is `None`.
fn to_rgba(format: u32, w: u32, h: u32, stride: u32, mut bytes: Vec<u8>) -> Option<Vec<u8>> {
    // RGB32, ARGB32 and ARGB32_Premultiplied are 0xAARRGGBB words: B, G, R, A in memory on the
    // little-endian machines Plasma runs on. RGBX8888, RGBA8888 and its premultiplied form are
    // R, G, B, A bytes. A screenshot is opaque, so alpha is set rather than unpremultiplied.
    let bgra = match format {
        4..=6 => true,
        16..=18 => false,
        _ => return None,
    };
    let (w, h, stride) = (w as usize, h as usize, stride as usize);
    if w == 0 || h == 0 || stride < w * 4 || bytes.len() < stride * h.saturating_sub(1) + w * 4 {
        return None;
    }
    // Rows move forward only (a packed row never starts past its padded one), so each copy is safe.
    let packed = w * 4;
    for row in 1..h {
        bytes.copy_within(row * stride..row * stride + packed, row * packed);
    }
    bytes.truncate(packed * h);
    for p in bytes.as_chunks_mut::<4>().0 {
        if bgra {
            p.swap(0, 2);
        }
        p[3] = 255;
    }
    bytes.shrink_to_fit();
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_rows_become_packed_opaque_rgba() {
        // 2x2 with a 12-byte stride (4 bytes of padding per row).
        let rgbx = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, 7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9];
        assert_eq!(to_rgba(16, 2, 2, 12, rgbx.to_vec()), Some(vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]));
        let bgra = [3, 2, 1, 128];
        assert_eq!(to_rgba(6, 1, 1, 4, bgra.to_vec()), Some(vec![1, 2, 3, 255]));
        assert_eq!(to_rgba(13, 1, 1, 4, bgra.to_vec()), None, "RGB888 is not handled");
        assert_eq!(to_rgba(16, 2, 2, 12, rgbx[..16].to_vec()), None, "short data");
        assert_eq!(to_rgba(16, 0, 0, 0, Vec::new()), None, "an empty picture, not a panic");
    }
}

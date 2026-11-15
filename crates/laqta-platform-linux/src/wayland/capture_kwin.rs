//! KDE Plasma: the whole workspace from KWin's `ScreenShot2` D-Bus interface, without the pointer.
//! Plasma's Screenshot portal always draws the pointer into the picture, so the frozen frame under
//! the region overlay showed a second, stuck pointer. KWin writes raw pixels into a pipe and
//! describes them in its reply; the picture is split per output like the portal's.

use std::collections::HashMap;
use std::ffi::c_int;
use std::fs::File;
use std::os::fd::{AsFd as _, AsRawFd as _, OwnedFd};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use async_io::Async;
use futures_lite::AsyncReadExt as _;

use ashpd::zbus::Connection;
use ashpd::zbus::{Fd, OwnedVariant, Variant};
use futures_lite::future;
use laqta_domain::{Frame, PixelFormat};
use laqta_ports::PortError;

use super::{capture_portal, capture_wayshot};

/// How long KWin may take to answer and write the picture.
const BUDGET: Duration = Duration::from_secs(3);

/// The pipe's capacity while KWin fills it: Linux's default 64 KiB makes a 4K picture cost hundreds
/// of wake-ups, and 1 MiB is what an unprivileged process may ask for (`fs.pipe-max-size`).
const PIPE_SIZE: usize = 1 << 20;

/// Bytes the next picture is expected to take: set from the size in KWin's reply, so the buffer is
/// reserved once instead of growing by doubling. The previous capture's size is a good first guess.
static EXPECTED: AtomicUsize = AtomicUsize::new(0);

/// Upper bound on a reserved buffer, so a bad reply cannot ask for gigabytes.
const MAX_EXPECTED: usize = 1 << 30;

const F_SETPIPE_SZ: c_int = 1031;

unsafe extern "C" {
    fn fcntl(fd: c_int, cmd: c_int, ...) -> c_int;
}

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

/// Reads the pipe to its end, into a buffer reserved from `EXPECTED` as soon as that is known.
async fn read_all(mut reader: Async<File>) -> std::io::Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = vec![0u8; PIPE_SIZE];
    loop {
        let expected = EXPECTED.load(Ordering::Relaxed).min(MAX_EXPECTED);
        if expected > buf.capacity() {
            buf.reserve_exact(expected - buf.len());
        }
        match reader.read(&mut chunk).await? {
            0 => return Ok(buf),
            n => buf.extend_from_slice(&chunk[..n]),
        }
    }
}

async fn capture() -> Result<Vec<Frame>, PortError> {
    let started = Instant::now();
    let ms = |t: Instant| t.elapsed().as_secs_f64() * 1000.0;
    let layout = capture_wayshot::layout_soon();
    let conn = Connection::session().await.map_err(fail)?;
    let (reader, writer) = std::io::pipe().map_err(fail)?;
    // SAFETY: `fcntl` on a pipe end this function owns, with an int argument. It can fail (the
    // limit, an old kernel); the pipe then keeps its default size, so the result is ignored.
    let _ = unsafe { fcntl(reader.as_raw_fd(), F_SETPIPE_SZ, PIPE_SIZE as c_int) };
    // Read on the reactor, not a blocking thread: when the time budget drops this future, the read
    // end closes with it instead of leaving a thread waiting on KWin.
    let reader = Async::new(File::from(OwnedFd::from(reader))).map_err(fail)?;
    let pixels = read_all(reader);
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
        let results: HashMap<String, OwnedVariant> = reply.map_err(fail)?.body().deserialize().map_err(fail)?;
        let get = |key: &str| results.get(key).and_then(|v| u32::try_from(v).ok()).ok_or_else(|| fail(format!("no {key} in the reply")));
        let picture = (get("width")?, get("height")?, get("stride")?, get("format")?);
        // The reader may still be going: tell it how much to expect.
        EXPECTED.store((picture.2 as usize).saturating_mul(picture.1 as usize), Ordering::Relaxed);
        Ok::<_, PortError>(picture)
    };
    // KWin may write before it replies, so the read runs from the start: a full pipe would stall it.
    let (picture, pixels) = future::zip(call, pixels).await;
    let (w, h, stride, format) = picture?;
    let bytes = pixels.map_err(fail)?;
    let mut lap = Instant::now();
    tracing::debug!(target: "laqta::perf", ms = ms(started), bytes = bytes.len(), "kwin dbus call and pipe read");
    let (pixel_format, pixels) =
        to_packed(format, w, h, stride, bytes).ok_or_else(|| fail(format!("unsupported image format {format}")))?;
    tracing::debug!(target: "laqta::perf", ms = ms(lap), "kwin pixel repack");
    lap = Instant::now();
    let layout = layout.await;
    tracing::debug!(target: "laqta::perf", ms = ms(lap), outputs = layout.len(), "kwin layout");
    lap = Instant::now();
    let frames = capture_portal::split(w, h, pixel_format, pixels, &layout);
    tracing::debug!(target: "laqta::perf", ms = ms(lap), frames = frames.len(), "kwin split");
    tracing::debug!(target: "laqta::perf", ms = ms(started), "kwin capture");
    Ok(frames)
}

/// Tightly packed, opaque pixels from KWin's raw `QImage` bytes, repacked in place so a workspace
/// of several 4K outputs is never held twice. `format` is the `QImage::Format` number; the 32-bit
/// formats KWin uses are handled, anything else is `None`. The channel order is kept (BGRA stays
/// BGRA, which is what the GPU takes), so no pixel is swapped: only the padding is dropped and
/// alpha set.
fn to_packed(format: u32, w: u32, h: u32, stride: u32, mut bytes: Vec<u8>) -> Option<(PixelFormat, Vec<u8>)> {
    // RGB32, ARGB32 and ARGB32_Premultiplied are 0xAARRGGBB words: B, G, R, A in memory on the
    // little-endian machines Plasma runs on. RGBX8888, RGBA8888 and its premultiplied form are
    // R, G, B, A bytes. A screenshot is opaque, so alpha is set rather than unpremultiplied.
    let pixel_format = match format {
        4..=6 => PixelFormat::Bgra8,
        16..=18 => PixelFormat::Rgba8,
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
        p[3] = 255;
    }
    bytes.shrink_to_fit();
    Some((pixel_format, bytes))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use laqta_domain::PhysRect;

    use super::super::layout::OutputLayout;
    use super::*;

    #[test]
    fn raw_rows_become_packed_opaque_pixels() {
        // 2x2 with a 12-byte stride (4 bytes of padding per row).
        let rgbx = [1, 2, 3, 0, 4, 5, 6, 0, 9, 9, 9, 9, 7, 8, 9, 0, 10, 11, 12, 0, 9, 9, 9, 9];
        assert_eq!(
            to_packed(16, 2, 2, 12, rgbx.to_vec()),
            Some((PixelFormat::Rgba8, vec![1, 2, 3, 255, 4, 5, 6, 255, 7, 8, 9, 255, 10, 11, 12, 255]))
        );
        let bgra = [3, 2, 1, 128];
        assert_eq!(to_packed(6, 1, 1, 4, bgra.to_vec()), Some((PixelFormat::Bgra8, vec![3, 2, 1, 255])));
        assert_eq!(to_packed(13, 1, 1, 4, bgra.to_vec()), None, "RGB888 is not handled");
        assert_eq!(to_packed(16, 2, 2, 12, rgbx[..16].to_vec()), None, "short data");
        assert_eq!(to_packed(16, 0, 0, 0, Vec::new()), None, "an empty picture, not a panic");
    }

    #[test]
    fn a_bgra_picture_reaches_its_frames_without_a_swap() {
        // Two 1x1 outputs side by side, KWin's 0xAARRGGBB words (B, G, R, A in memory) with padding.
        let raw = [3, 2, 1, 128, 6, 5, 4, 0];
        let (format, pixels) = to_packed(5, 2, 1, 8, raw.to_vec()).unwrap();
        assert_eq!(format, PixelFormat::Bgra8);
        let layout = |name: &str, x| OutputLayout {
            name: name.into(),
            description: String::new(),
            logical: PhysRect { x, y: 0, width: 1, height: 1 },
            scale: 1.0,
        };
        let frames = capture_portal::split(2, 1, format, pixels, &[layout("A", 0), layout("B", 1)]);
        assert_eq!(frames.len(), 2);
        // Bgra8 is what `bgra8_pixels` copies as is, so these bytes are what the GPU gets.
        assert!(frames.iter().all(|f| f.image.format() == PixelFormat::Bgra8));
        assert_eq!(frames[0].image.pixels(), [3, 2, 1, 255]);
        assert_eq!(frames[1].image.pixels(), [6, 5, 4, 255]);
    }
}

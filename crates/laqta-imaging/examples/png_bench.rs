//! Median of 20 compose-and-encode runs of a 3840x2160 frame (M0 §7.3).
#![allow(clippy::unwrap_used, clippy::disallowed_types, clippy::disallowed_methods)]

use std::time::Instant;

use laqta_domain::{Bitmap, Frame, Monitor, MonitorId, PhysRect, PixelFormat};

fn main() {
    let (w, h) = (3840u32, 2160u32);
    // A screenshot-like image: flat panels, gradients and some noisy "text" rows.
    let mut px = vec![0u8; (w * h * 4) as usize];
    for y in 0..h {
        for x in 0..w {
            let i = ((y * w + x) * 4) as usize;
            let panel = (x / 480 + y / 270) % 3;
            let v = match panel {
                0 => [0x33, 0x66, 0x99],
                1 => [(x % 256) as u8, (y % 256) as u8, 0x80],
                _ => {
                    if (y % 18) < 12 && ((x * 7919 + y * 104729) % 5) < 2 {
                        [20, 20, 20]
                    } else {
                        [240, 240, 240]
                    }
                }
            };
            px[i..i + 4].copy_from_slice(&[v[2], v[1], v[0], 255]);
        }
    }
    let frame = Frame {
        monitor: Monitor {
            id: MonitorId("bench".into()),
            name: "bench".into(),
            rect: PhysRect { x: 0, y: 0, width: w, height: h },
            scale: 1.0,
            primary: true,
        },
        image: Bitmap::new(w, h, PixelFormat::Bgra8, px.into()).unwrap(),
    };
    let pool = rayon::ThreadPoolBuilder::new().build().unwrap();
    for (name, level) in [("balanced", png::Compression::Balanced), ("fast", png::Compression::Fast)] {
        let mut times = Vec::new();
        let mut size = 0;
        for _ in 0..20 {
            let t = Instant::now();
            let bytes = pool.install(|| {
                let c = laqta_imaging::compose_monitors(std::slice::from_ref(&frame));
                laqta_imaging::png::encode_with(&c, level).unwrap()
            });
            times.push(t.elapsed());
            size = bytes.len();
        }
        times.sort();
        println!("{name}: median {:?} for 3840x2160 compose+encode, {size} bytes", times[10]);
    }
}

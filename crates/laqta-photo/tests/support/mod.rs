#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used, dead_code)]

use std::path::PathBuf;

use laqta_domain::{Bitmap, PixelFormat};

/// A `w` x `h` bitmap of one straight RGBA colour.
pub fn solid(w: u32, h: u32, color: [u8; 4]) -> Bitmap {
    let px: Vec<u8> = std::iter::repeat_n(color, w as usize * h as usize).flatten().collect();
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

/// A bitmap whose pixel at `(x, y)` is `f(x, y)`.
pub fn from_fn(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 4]) -> Bitmap {
    let mut px = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            px.extend_from_slice(&f(x, y));
        }
    }
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

/// The straight RGBA pixel at `(x, y)`.
pub fn px(b: &Bitmap, x: u32, y: u32) -> [u8; 4] {
    let i = (y as usize * b.width() as usize + x as usize) * 4;
    let p = &b.pixels()[i..i + 4];
    [p[0], p[1], p[2], p[3]]
}

/// Runs `f` inside a rayon pool, because the imaging kernels assert they run in one.
pub fn in_pool<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(f)
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.png"))
}

/// Compares with `tests/snapshots/<name>.png` exactly. Writes the file when it is missing or when
/// `LAQTA_BLESS=1` is set.
pub fn snapshot(name: &str, b: &Bitmap) {
    assert_eq!(b.format(), PixelFormat::Rgba8);
    let path = snapshot_path(name);
    let bless = std::env::var_os("LAQTA_BLESS").is_some_and(|v| v == "1");
    if bless || !path.exists() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let file = std::fs::File::create(&path).unwrap();
        let mut enc = png::Encoder::new(std::io::BufWriter::new(file), b.width(), b.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(b.pixels()).unwrap();
        return;
    }
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(&path).unwrap()));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (b.width(), b.height()), "snapshot {name}: size");
    assert!(&buf[..info.buffer_size()] == b.pixels(), "snapshot {name} differs; rerun with LAQTA_BLESS=1 if the change is intended");
}

#![allow(clippy::disallowed_methods, clippy::disallowed_types, clippy::unwrap_used, dead_code)]

use std::path::PathBuf;

use laqta_domain::{Bitmap, PixelFormat};
use laqta_editor::{Editor, EditorInput, Fonts, Mods, PointerButton};

/// A 64x48 base: a gradient with a few solid blocks, so effects have something to work on.
pub fn base() -> Bitmap {
    let (w, h) = (64u32, 48u32);
    let mut px = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let block = (x / 16 + y / 12) % 2 == 0;
            let (r, g, b) = if block { (x as u8 * 4, y as u8 * 5, 160) } else { (230, 230 - x as u8, y as u8 * 3) };
            px.extend_from_slice(&[r, g, b, 255]);
        }
    }
    Bitmap::new(w, h, PixelFormat::Rgba8, px.into()).unwrap()
}

pub fn small_image(color: [u8; 4]) -> Bitmap {
    let px: Vec<u8> = std::iter::repeat_n(color, 64).flatten().collect();
    Bitmap::new(8, 8, PixelFormat::Rgba8, px.into()).unwrap()
}

pub fn editor() -> Editor {
    Editor::new(base(), Fonts::empty())
}

/// Runs `f` inside a rayon pool (the imaging kernels assert it).
pub fn in_pool<T: Send>(f: impl FnOnce() -> T + Send) -> T {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(2).build().unwrap();
    pool.install(f)
}

pub fn drag(e: &mut Editor, from: (f32, f32), to: (f32, f32), mods: Mods) {
    e.handle(EditorInput::PointerDown { x: from.0, y: from.1, button: PointerButton::Left, mods });
    let mid = ((from.0 + to.0) / 2.0, (from.1 + to.1) / 2.0);
    e.handle(EditorInput::PointerMove { x: mid.0, y: mid.1, mods });
    e.handle(EditorInput::PointerMove { x: to.0, y: to.1, mods });
    e.handle(EditorInput::PointerUp { x: to.0, y: to.1, button: PointerButton::Left, mods });
}

pub fn click(e: &mut Editor, at: (f32, f32)) {
    drag(e, at, at, Mods::default());
}

fn snapshot_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/snapshots").join(format!("{name}.png"))
}

/// Compares with `tests/snapshots/<name>.png` exactly; writes it when missing or `LAQTA_BLESS=1`.
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

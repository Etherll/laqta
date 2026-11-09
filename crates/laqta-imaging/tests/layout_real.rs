//! `laqta_imaging::layout` on a real screenshot: runs when `LAQTA_LAYOUT_IMAGE` names an image
//! and prints the regions it finds (for tuning; no fixture is checked in).

#![allow(clippy::disallowed_methods, clippy::unwrap_used)]

use laqta_domain::PhysRect;
use laqta_imaging::Codecs;
use laqta_imaging::layout::regions;

#[test]
fn real_screenshot() {
    let Some(path) = std::env::var_os("LAQTA_LAYOUT_IMAGE") else { return };
    let bytes = std::fs::read(path).unwrap();
    let image = Codecs::builtin().decode(&bytes).unwrap();
    let start = std::time::Instant::now();
    let found = regions(&image, PhysRect { x: 0, y: 0, width: image.width(), height: image.height() });
    eprintln!("{} regions in {:?}", found.len(), start.elapsed());
    for r in &found {
        eprintln!("{} {} {} {}", r.x, r.y, r.width, r.height);
    }
    assert!(!found.is_empty());
}

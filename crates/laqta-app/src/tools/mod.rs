//! Application logic for the M5 tools (M5 D1, ARCHITECTURE §2.2). The pure parts are plain values: colour formats,
//! ruler maths, the colour history. The rest are functions that take the `Ports` they use, so file access goes through
//! `files`, pixel work runs on the compute pool, and platform calls go through the ports. The tool windows in
//! `laqta-ui` call these functions.

mod color;
mod common;
mod hash;
mod images;
mod indexer;
mod ocr;
mod qr;
mod ruler;
mod video;
mod window;

pub use color::{ColorFormats, ColorHistory, parse_color};
pub use hash::{HashKind, compare_hash, hash_file, hash_text};
pub use images::{
    combine_files, compare_files, convert_files, make_icon, read_metadata, resize_files, split_file, strip_metadata_files, thumbnail_files,
    watermark_files,
};
pub use indexer::{IndexFormat, IndexOptions, index_folder};
pub use ocr::ocr_image;
pub use qr::{qr_decode_file, qr_decode_image, qr_decode_screen, qr_image};
pub use ruler::{Measurement, measure, px_to_cm, px_to_inches};
pub use video::{convert_video, make_gif, trim_video, video_thumbnails};
pub use window::{WindowToggles, inspect_active, toggle_borderless_active, toggle_topmost_active};

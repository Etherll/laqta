//! Pure pixel work (ARCHITECTURE §2.1). Never builds a thread pool: public parallel functions must
//! run inside the compute pool handed out by `Spawner::run_compute`.
#![forbid(unsafe_code)]

pub mod beautify;
mod bmp;
mod codecs;
mod compose;
pub mod effects;
mod ops;
pub mod png;
pub mod qr;
pub mod stitch;
pub mod tools;

pub use codecs::{CodecError, CodecExtension, Codecs, EncodeOptions, sniff};
pub use compose::compose_monitors;
pub use ops::{bgra8_pixels, crop, resize, thumbnail, upscale_nearest};
pub use png::{EncodeError, encode_png};

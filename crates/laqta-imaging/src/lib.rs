//! Pure pixel work (ARCHITECTURE §2.1). Never builds a thread pool: public parallel functions must
//! run inside the compute pool handed out by `Spawner::run_compute`.
#![forbid(unsafe_code)]

mod compose;
pub mod png;

pub use compose::compose_monitors;
pub use png::{EncodeError, encode_png};

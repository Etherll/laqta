//! Port traits and the plain types they carry (ARCHITECTURE §2.3, ADR 024).

pub mod desktop;
pub mod error;
pub mod runtime;
pub mod screen;
pub mod task;

#[cfg(feature = "fakes")]
pub mod fakes;

pub use desktop::{ActionCallback, Clipboard, HotkeyRegistrar, Shell, Tray, TrayItem, TrayMenu};
pub use error::PortError;
pub use runtime::{Clock, Spawner};
pub use screen::ScreenSource;
pub use task::{AppError, TaskEvent, TaskId, TaskStatus};

/// A boxed future, so async ports work behind `Arc<dyn Port>`.
pub type BoxFuture<'a, T> = std::pin::Pin<Box<dyn std::future::Future<Output = T> + Send + 'a>>;

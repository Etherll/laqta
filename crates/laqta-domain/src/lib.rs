//! Data and rules that need no machine (ARCHITECTURE §2.1). Pure: no I/O, threads, env or clock.
#![forbid(unsafe_code)]

pub mod action;
pub mod capture;
pub mod geometry;
pub mod history;
pub mod hotkey;
pub mod image;
pub mod naming;
pub mod region;
pub mod settings;
pub mod workflow;

pub use action::Action;
pub use geometry::{Monitor, MonitorId, PhysRect};
pub use hotkey::{Key, KeyChord, Modifiers};
pub use image::{Bitmap, BitmapError, Frame, PixelFormat};
pub use settings::Settings;
pub use workflow::WorkflowId;

//! uRec recording engine.
//!
//! Std-only and platform-neutral: it builds and supervises the FFmpeg pipeline that does
//! capture and encoding. Windows-specific pieces (audio capture, hotkeys, monitor and window
//! enumeration) live in the app and feed this crate plain data.

pub mod edit;
pub mod encoder;
pub mod health;
pub mod hotkey;
pub mod mp4check;
pub mod naming;
pub mod pipeline;
pub mod probe;
pub mod process;
pub mod replay;
pub mod session;
pub mod sha256;
pub mod verify;
pub mod video;

pub use encoder::{Backend, Encoder};
pub use pipeline::{AudioInput, CaptureMethod, CaptureTarget, Container, FramePath, Output, Plan, Rect};
pub use session::{Session, StartError, StartRequest};
pub use video::{Codec, FrameRate, Preset, Resolution, VideoSettings};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

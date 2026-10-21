//! GPUI-agnostic terminal I/O for gpui-tui: input parsing, capability probing, and kitty
//! graphics output.

pub mod capabilities;
pub mod geometry;
pub mod graphics;
pub mod input;
mod output_capture;
pub mod picker;
pub mod presenter;
mod restore;
pub mod sequences;
pub mod session;
mod shared_memory;
mod tiles;
mod tty;
mod writer;

pub use capabilities::Capabilities;
pub use geometry::Geometry;
pub use presenter::{EncodedFrame, Frame, Presenter, Transport};
pub use restore::restore_terminal;
pub use session::{Session, SessionOptions};

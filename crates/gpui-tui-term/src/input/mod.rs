//! Parsing of terminal input: keys, mouse, paste, focus, resize, and query replies.

mod csi;
mod event;
mod keys;
mod parser;
#[cfg(test)]
mod tests;

pub use event::*;
pub use parser::Parser;

//! What the terminal supports, detected by sending queries and reading the replies.

use crate::graphics::{self, QueryMedium};
use crate::input::{ColorScheme, Reply, Rgb};

/// Image ID of the kitty graphics support query.
pub const GRAPHICS_QUERY_ID: u32 = 31;
/// Image ID of the shared-memory reachability query.
pub const SHARED_MEMORY_QUERY_ID: u32 = 32;

pub const MODE_SGR_PIXELS: u16 = 1016;
pub const MODE_SYNCHRONIZED_OUTPUT: u16 = 2026;
pub const MODE_COLOR_SCHEME_UPDATES: u16 = 2031;
pub const MODE_IN_BAND_RESIZE: u16 = 2048;

const PROBED_MODES: [u16; 4] = [
    MODE_SGR_PIXELS,
    MODE_SYNCHRONIZED_OUTPUT,
    MODE_COLOR_SCHEME_UPDATES,
    MODE_IN_BAND_RESIZE,
];

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Capabilities {
    pub kitty_graphics: bool,
    /// The terminal can read frames from POSIX shared memory (it runs on this machine).
    pub shared_memory: bool,
    pub kitty_keyboard: bool,
    pub sgr_pixels: bool,
    pub synchronized_output: bool,
    pub color_scheme_updates: bool,
    pub in_band_resize: bool,
    pub background: Option<Rgb>,
    /// Cell size in pixels as `(width, height)`, from `CSI 16 t`.
    pub cell_size: Option<(u16, u16)>,
}

impl Capabilities {
    /// Builds the capabilities from the replies to [`probe_queries`].
    pub fn from_replies<'a>(replies: impl IntoIterator<Item = &'a Reply>) -> Self {
        let mut capabilities = Self::default();
        for reply in replies {
            capabilities.apply(reply);
        }
        capabilities
    }

    fn apply(&mut self, reply: &Reply) {
        match reply {
            Reply::Graphics(graphics) if graphics.image_id == GRAPHICS_QUERY_ID => {
                self.kitty_graphics = graphics.is_ok();
            }
            Reply::Graphics(graphics) if graphics.image_id == SHARED_MEMORY_QUERY_ID => {
                self.shared_memory = graphics.is_ok();
            }
            Reply::KeyboardFlags(_) => self.kitty_keyboard = true,
            Reply::Mode { mode, state } => {
                let supported = state.is_supported();
                match *mode {
                    MODE_SGR_PIXELS => self.sgr_pixels = supported,
                    MODE_SYNCHRONIZED_OUTPUT => self.synchronized_output = supported,
                    MODE_COLOR_SCHEME_UPDATES => self.color_scheme_updates = supported,
                    MODE_IN_BAND_RESIZE => self.in_band_resize = supported,
                    _ => {}
                }
            }
            Reply::Background(rgb) => self.background = Some(*rgb),
            Reply::CellSize { width, height } if *width > 0 && *height > 0 => {
                self.cell_size = Some((*width, *height));
            }
            _ => {}
        }
    }

    pub fn color_scheme(&self) -> Option<ColorScheme> {
        self.background.map(Rgb::color_scheme)
    }
}

/// Queries that detect the capabilities, terminated by a primary device attributes request.
/// Terminals answer in order, so the DA1 reply marks the end of all answers.
///
/// `shared_memory_name` names a one-pixel segment the caller created for the shm probe.
pub fn probe_queries(shared_memory_name: Option<&str>) -> Vec<u8> {
    let mut out = Vec::new();
    graphics::query(&mut out, GRAPHICS_QUERY_ID, QueryMedium::Direct);
    if let Some(name) = shared_memory_name {
        graphics::query(
            &mut out,
            SHARED_MEMORY_QUERY_ID,
            QueryMedium::SharedMemory { name },
        );
    }
    // Kitty keyboard protocol flags.
    out.extend_from_slice(b"\x1b[?u");
    for mode in PROBED_MODES {
        out.extend_from_slice(format!("\x1b[?{mode}$p").as_bytes());
    }
    // Default background color and cell size in pixels.
    out.extend_from_slice(b"\x1b]11;?\x1b\\");
    out.extend_from_slice(b"\x1b[16t");
    // Primary device attributes: the end marker.
    out.extend_from_slice(b"\x1b[c");
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::{GraphicsReply, ModeState, Parser, TermEvent};

    fn replies(bytes: &[u8]) -> Vec<Reply> {
        Parser::new()
            .feed(bytes)
            .into_iter()
            .filter_map(|event| match event {
                TermEvent::Reply(reply) => Some(reply),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn kitty_like_terminal() {
        let replies = replies(
            b"\x1b_Gi=31;OK\x1b\\\x1b_Gi=32;OK\x1b\\\x1b[?0u\x1b[?1016;2$y\x1b[?2026;2$y\
              \x1b[?2031;2$y\x1b[?2048;2$y\x1b]11;rgb:1e1e/1e1e/2e2e\x1b\\\x1b[6;18;9t\x1b[?62;c",
        );
        let capabilities = Capabilities::from_replies(&replies);
        assert_eq!(
            capabilities,
            Capabilities {
                kitty_graphics: true,
                shared_memory: true,
                kitty_keyboard: true,
                sgr_pixels: true,
                synchronized_output: true,
                color_scheme_updates: true,
                in_band_resize: true,
                background: Some(Rgb {
                    red: 0x1e,
                    green: 0x1e,
                    blue: 0x2e
                }),
                cell_size: Some((9, 18)),
            }
        );
        assert_eq!(capabilities.color_scheme(), Some(ColorScheme::Dark));
    }

    #[test]
    fn terminal_without_graphics_or_shared_memory() {
        let replies = [
            Reply::Graphics(GraphicsReply {
                image_id: SHARED_MEMORY_QUERY_ID,
                message: "ENOENT:not found".to_owned(),
            }),
            Reply::Mode {
                mode: MODE_SGR_PIXELS,
                state: ModeState::NotRecognized,
            },
            Reply::PrimaryDeviceAttributes,
        ];
        let capabilities = Capabilities::from_replies(&replies);
        assert!(!capabilities.kitty_graphics);
        assert!(!capabilities.shared_memory);
        assert!(!capabilities.sgr_pixels);
    }

    #[test]
    fn probe_ends_with_device_attributes_request() {
        let queries = probe_queries(Some("/probe"));
        assert!(queries.ends_with(b"\x1b[c"));
        let text = String::from_utf8_lossy(&queries);
        assert!(text.contains("a=q,i=32"));
        assert!(text.contains("\x1b[?2048$p"));
    }
}

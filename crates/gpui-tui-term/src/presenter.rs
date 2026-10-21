//! Turns rendered frames into terminal output, with backpressure from in-flight fences.

use std::collections::VecDeque;
use std::io::{self, Write as _};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::graphics::{self, CellArea, Image, Medium, PixelFormat, QueryMedium};
use crate::input::GraphicsReply;
use crate::sequences::{self, CURSOR_HOME, SYNCHRONIZED_UPDATE_BEGIN, SYNCHRONIZED_UPDATE_END};
use crate::shared_memory::SegmentPool;
use crate::tiles::{Tile, TileLayout, tile_differs, tile_pixels};

const FRAME_IMAGE_ID: u32 = 1;
const FRAME_PLACEMENT_ID: u32 = 1;
/// Image ID of the first tile; tile `n` uses `TILE_IMAGE_BASE + n`.
const TILE_IMAGE_BASE: u32 = 100;
/// Image ID of the query sent after each frame. Its reply means the terminal processed the
/// frame.
pub const FENCE_IMAGE_ID: u32 = 1_000_001;
/// Frames sent but not yet acknowledged before we stop rendering new ones.
const MAX_FRAMES_IN_FLIGHT: usize = 2;
/// A fence without a reply after this long is considered lost.
const FENCE_TIMEOUT: Duration = Duration::from_secs(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Transport {
    /// Frames go through POSIX shared memory; only the segment name crosses the pty.
    SharedMemory,
    /// Frames are zlib-compressed and base64-encoded into the escape sequence, a tile at a
    /// time. Used when the terminal runs on another machine.
    Direct,
}

/// An RGBA frame, rows top to bottom, no padding.
#[derive(Clone, Copy, Debug)]
pub struct Frame<'a> {
    pub pixels: &'a [u8],
    pub width: u32,
    pub height: u32,
}

pub struct Presenter {
    transport: Transport,
    segments: SegmentPool,
    previous: Option<PreviousFrame>,
    fences_in_flight: VecDeque<Instant>,
}

struct PreviousFrame {
    pixels: Vec<u8>,
    width: u32,
    height: u32,
    area: CellArea,
}

impl PreviousFrame {
    /// Whether `frame` has the same size and is shown over the same cells.
    fn has_layout(&self, frame: Frame<'_>, area: CellArea) -> bool {
        self.width == frame.width && self.height == frame.height && self.area == area
    }
}

/// A frame ready for the terminal.
#[derive(Debug)]
pub struct EncodedFrame {
    pub bytes: Vec<u8>,
    /// Images sent: the whole frame, or the tiles that changed.
    pub images_sent: usize,
    /// Images that make up the whole frame.
    pub images_total: usize,
}

struct Images {
    sent: usize,
    total: usize,
}

impl Presenter {
    pub fn new(transport: Transport) -> Self {
        Self {
            transport,
            segments: SegmentPool::new("frame"),
            previous: None,
            fences_in_flight: VecDeque::new(),
        }
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Whether a new frame may be sent now. Lost fences expire after a timeout.
    pub fn is_ready(&mut self, now: Instant) -> bool {
        while self
            .fences_in_flight
            .front()
            .is_some_and(|sent_at| now.duration_since(*sent_at) > FENCE_TIMEOUT)
        {
            self.fences_in_flight.pop_front();
        }
        self.fences_in_flight.len() < MAX_FRAMES_IN_FLIGHT
    }

    /// When the oldest in-flight fence will expire, if any.
    pub fn next_fence_expiry(&self) -> Option<Instant> {
        self.fences_in_flight
            .front()
            .map(|sent_at| *sent_at + FENCE_TIMEOUT)
    }

    /// Consumes a graphics reply. Returns whether it acknowledged a frame.
    pub fn handle_reply(&mut self, reply: &GraphicsReply) -> bool {
        if reply.image_id != FENCE_IMAGE_ID {
            return false;
        }
        self.fences_in_flight.pop_front();
        true
    }

    /// Forces the next frame to be sent even if its pixels did not change, e.g. after the
    /// terminal cleared the screen.
    pub fn invalidate(&mut self) {
        self.previous = None;
    }

    /// Encodes `frame` drawn over `area`. Returns `None` if nothing changed since the last
    /// frame.
    ///
    /// Over shared memory the whole frame is sent. Inline, only the tiles that changed are
    /// sent. When the frame size or area changes, all images are deleted and sent again.
    pub fn encode(
        &mut self,
        frame: Frame<'_>,
        area: CellArea,
        now: Instant,
    ) -> io::Result<Option<EncodedFrame>> {
        let previous_pixels = self
            .previous
            .as_ref()
            .filter(|previous| previous.has_layout(frame, area))
            .map(|previous| previous.pixels.as_slice());

        let mut out = Vec::new();
        out.extend_from_slice(SYNCHRONIZED_UPDATE_BEGIN);
        if previous_pixels.is_none() {
            graphics::delete_all(&mut out);
        }
        let images = match self.transport {
            Transport::SharedMemory => {
                if previous_pixels == Some(frame.pixels) {
                    return Ok(None);
                }
                write_whole_frame(&mut out, &mut self.segments, frame, area)?;
                Images { sent: 1, total: 1 }
            }
            Transport::Direct => {
                let layout = TileLayout::new(frame.width, frame.height, area);
                let changed: Vec<Tile> = layout
                    .tiles()
                    .filter(|tile| {
                        previous_pixels.is_none_or(|previous| tile_differs(previous, frame, tile))
                    })
                    .collect();
                if changed.is_empty() {
                    return Ok(None);
                }
                for tile in &changed {
                    write_tile(&mut out, frame, tile)?;
                }
                Images {
                    sent: changed.len(),
                    total: layout.len(),
                }
            }
        };
        out.extend_from_slice(SYNCHRONIZED_UPDATE_END);
        graphics::query(&mut out, FENCE_IMAGE_ID, QueryMedium::Direct);

        self.fences_in_flight.push_back(now);
        self.remember(frame, area);
        Ok(Some(EncodedFrame {
            bytes: out,
            images_sent: images.sent,
            images_total: images.total,
        }))
    }

    fn remember(&mut self, frame: Frame<'_>, area: CellArea) {
        let mut pixels = self
            .previous
            .take()
            .map(|previous| previous.pixels)
            .unwrap_or_default();
        pixels.clear();
        pixels.extend_from_slice(frame.pixels);
        self.previous = Some(PreviousFrame {
            pixels,
            width: frame.width,
            height: frame.height,
            area,
        });
    }
}

fn write_whole_frame(
    out: &mut Vec<u8>,
    segments: &mut SegmentPool,
    frame: Frame<'_>,
    area: CellArea,
) -> io::Result<()> {
    let image = Image {
        image_id: FRAME_IMAGE_ID,
        placement_id: FRAME_PLACEMENT_ID,
        width: frame.width,
        height: frame.height,
        format: PixelFormat::Rgba,
    };
    let name = segments.write(frame.pixels)?;
    out.extend_from_slice(CURSOR_HOME);
    graphics::transmit_and_place(out, &image, area, Medium::SharedMemory { name: &name });
    Ok(())
}

/// Sends one tile, compressed, and places it over its cells.
fn write_tile(out: &mut Vec<u8>, frame: Frame<'_>, tile: &Tile) -> io::Result<()> {
    let (format, pixels) = tile_pixels(frame, tile);
    let compressed = compress(&pixels)?;
    let tile_index = u32::try_from(tile.index).map_err(io::Error::other)?;
    let image = Image {
        image_id: TILE_IMAGE_BASE + tile_index,
        placement_id: FRAME_PLACEMENT_ID,
        width: tile.column.pixels,
        height: tile.row.pixels,
        format,
    };
    let area = CellArea {
        columns: tile.column.cells,
        rows: tile.row.cells,
    };
    sequences::move_cursor(out, tile.row.cell_start, tile.column.cell_start);
    let medium = Medium::Direct {
        data: &compressed,
        compressed: true,
    };
    graphics::transmit_and_place(out, &image, area, medium);
    Ok(())
}

fn compress(pixels: &[u8]) -> io::Result<Vec<u8>> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(pixels)?;
    encoder.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    const AREA: CellArea = CellArea {
        columns: 2,
        rows: 1,
    };

    fn frame(pixels: &[u8]) -> Frame<'_> {
        Frame {
            pixels,
            width: 1,
            height: 1,
        }
    }

    fn fence_reply() -> GraphicsReply {
        GraphicsReply {
            image_id: FENCE_IMAGE_ID,
            message: "OK".to_owned(),
        }
    }

    #[test]
    fn identical_frames_are_skipped_until_invalidated() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let now = Instant::now();
        assert!(
            presenter
                .encode(frame(&[1, 2, 3, 255]), AREA, now)?
                .is_some()
        );
        assert!(
            presenter
                .encode(frame(&[1, 2, 3, 255]), AREA, now)?
                .is_none()
        );
        assert!(
            presenter
                .encode(frame(&[9, 2, 3, 255]), AREA, now)?
                .is_some()
        );

        presenter.invalidate();
        assert!(
            presenter
                .encode(frame(&[9, 2, 3, 255]), AREA, now)?
                .is_some()
        );
        Ok(())
    }

    #[test]
    fn frames_in_flight_limit_rendering_until_fences_return() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let now = Instant::now();
        presenter.encode(frame(&[1, 0, 0, 255]), AREA, now)?;
        assert!(presenter.is_ready(now));
        presenter.encode(frame(&[2, 0, 0, 255]), AREA, now)?;
        assert!(!presenter.is_ready(now));

        assert!(presenter.handle_reply(&fence_reply()));
        assert!(presenter.is_ready(now));
        Ok(())
    }

    #[test]
    fn lost_fences_expire() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let sent_at = Instant::now();
        presenter.encode(frame(&[1, 0, 0, 255]), AREA, sent_at)?;
        presenter.encode(frame(&[2, 0, 0, 255]), AREA, sent_at)?;
        assert!(!presenter.is_ready(sent_at));
        assert!(presenter.is_ready(sent_at + FENCE_TIMEOUT * 2));
        Ok(())
    }

    #[test]
    fn frame_output_is_synchronized_and_followed_by_a_fence() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let encoded = presenter.encode(frame(&[1, 2, 3, 255]), AREA, Instant::now())?;
        let text = encoded
            .map(|encoded| String::from_utf8_lossy(&encoded.bytes).into_owned())
            .unwrap_or_default();
        assert!(text.starts_with("\x1b[?2026h\x1b_Ga=d,d=A,q=2\x1b\\\x1b[1;1H\x1b_Ga=T,"));
        assert!(text.contains("\x1b[?2026l\x1b_Ga=q,i=1000001,"));
        Ok(())
    }

    /// A 300×40 opaque frame over 30×4 cells of 10×10 pixels.
    fn grey_frame() -> Vec<u8> {
        vec![200; 300 * 40 * 4]
    }

    fn wide(pixels: &[u8]) -> Frame<'_> {
        Frame {
            pixels,
            width: 300,
            height: 40,
        }
    }

    const WIDE_AREA: CellArea = CellArea {
        columns: 30,
        rows: 4,
    };

    #[test]
    fn inline_frames_send_only_changed_tiles() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let now = Instant::now();
        let mut pixels = grey_frame();
        let first = presenter.encode(wide(&pixels), WIDE_AREA, now)?;
        let first = first.ok_or_else(|| io::Error::other("first frame skipped"))?;
        assert_eq!(first.images_sent, first.images_total);
        assert!(first.images_total > 1);

        // Change the bottom-right pixel.
        let last = pixels.len() - 4;
        pixels[last] = 0;
        let second = presenter.encode(wide(&pixels), WIDE_AREA, now)?;
        let second = second.ok_or_else(|| io::Error::other("changed frame skipped"))?;
        assert_eq!(second.images_sent, 1);
        let text = String::from_utf8_lossy(&second.bytes);
        assert!(!text.contains("a=d"), "tiles must be replaced, not deleted");
        assert!(text.contains(",f=24,"), "opaque tiles are sent as RGB");
        assert!(text.contains(&format!(
            ",i={},",
            TILE_IMAGE_BASE as usize + first.images_total - 1
        )));
        Ok(())
    }

    #[test]
    fn new_area_deletes_old_images_and_resends_everything() -> io::Result<()> {
        let mut presenter = Presenter::new(Transport::Direct);
        let now = Instant::now();
        let pixels = grey_frame();
        presenter.encode(wide(&pixels), WIDE_AREA, now)?;
        let moved = CellArea {
            columns: 15,
            rows: 2,
        };
        let resized = presenter.encode(wide(&pixels), moved, now)?;
        let resized = resized.ok_or_else(|| io::Error::other("resized frame skipped"))?;
        assert_eq!(resized.images_sent, resized.images_total);
        assert!(String::from_utf8_lossy(&resized.bytes).contains("a=d,d=A"));
        Ok(())
    }

    #[test]
    fn other_graphics_replies_are_not_fences() {
        let mut presenter = Presenter::new(Transport::Direct);
        let reply = GraphicsReply {
            image_id: 31,
            message: "OK".to_owned(),
        };
        assert!(!presenter.handle_reply(&reply));
    }
}

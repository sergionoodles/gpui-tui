//! Turns rendered frames into terminal output, with backpressure from in-flight fences.

use std::collections::VecDeque;
use std::io::{self, Write as _};
use std::time::{Duration, Instant};

use flate2::Compression;
use flate2::write::ZlibEncoder;

use crate::graphics::{self, CellArea, Image, Medium, PixelFormat, QueryMedium};
use crate::input::GraphicsReply;
use crate::sequences::{CURSOR_HOME, SYNCHRONIZED_UPDATE_BEGIN, SYNCHRONIZED_UPDATE_END};
use crate::shared_memory::SegmentPool;

const FRAME_IMAGE_ID: u32 = 1;
const FRAME_PLACEMENT_ID: u32 = 1;
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
    /// Frames are zlib-compressed and base64-encoded into the escape sequence.
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

    /// Encodes `frame` drawn over `area`. Returns `None` if it is identical to the last frame.
    pub fn encode(
        &mut self,
        frame: Frame<'_>,
        area: CellArea,
        now: Instant,
    ) -> io::Result<Option<Vec<u8>>> {
        if self.is_unchanged(frame, area) {
            return Ok(None);
        }

        let image = Image {
            image_id: FRAME_IMAGE_ID,
            placement_id: FRAME_PLACEMENT_ID,
            width: frame.width,
            height: frame.height,
            format: PixelFormat::Rgba,
        };
        let mut out = Vec::new();
        out.extend_from_slice(SYNCHRONIZED_UPDATE_BEGIN);
        out.extend_from_slice(CURSOR_HOME);
        match self.transport {
            Transport::SharedMemory => {
                let name = self.segments.write(frame.pixels)?;
                graphics::transmit_and_place(
                    &mut out,
                    &image,
                    area,
                    Medium::SharedMemory { name: &name },
                );
            }
            Transport::Direct => {
                let compressed = compress(frame.pixels)?;
                let medium = Medium::Direct {
                    data: &compressed,
                    compressed: true,
                };
                graphics::transmit_and_place(&mut out, &image, area, medium);
            }
        }
        out.extend_from_slice(SYNCHRONIZED_UPDATE_END);
        graphics::query(&mut out, FENCE_IMAGE_ID, QueryMedium::Direct);

        self.fences_in_flight.push_back(now);
        self.remember(frame, area);
        Ok(Some(out))
    }

    fn is_unchanged(&self, frame: Frame<'_>, area: CellArea) -> bool {
        self.previous.as_ref().is_some_and(|previous| {
            previous.width == frame.width
                && previous.height == frame.height
                && previous.area == area
                && previous.pixels == frame.pixels
        })
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
        let out = presenter
            .encode(frame(&[1, 2, 3, 255]), AREA, Instant::now())?
            .unwrap_or_default();
        let text = String::from_utf8_lossy(&out);
        assert!(text.starts_with("\x1b[?2026h\x1b[H\x1b_Ga=T,"));
        assert!(text.contains("\x1b[?2026l\x1b_Ga=q,i=1000001,"));
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

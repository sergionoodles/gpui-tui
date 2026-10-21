use std::time::Duration;

use anyhow::Result;
use gpui::{Bounds, DisplayId, Pixels, PlatformDisplay, Point, Size};

const DISPLAY_ID: u64 = 1;
/// Stable identifier of the one terminal display.
const DISPLAY_UUID: u128 = 0x6770_7569_2d74_7569_0000_0000_0000_0001;

/// The terminal's text area, presented to GPUI as a display the size of the window.
#[derive(Debug)]
pub(crate) struct TuiDisplay {
    size: Size<Pixels>,
    refresh_interval: Duration,
}

impl TuiDisplay {
    pub(crate) fn new(size: Size<Pixels>, refresh_interval: Duration) -> Self {
        Self {
            size,
            refresh_interval,
        }
    }
}

impl PlatformDisplay for TuiDisplay {
    fn id(&self) -> DisplayId {
        DisplayId::new(DISPLAY_ID)
    }

    fn uuid(&self) -> Result<uuid::Uuid> {
        Ok(uuid::Uuid::from_u128(DISPLAY_UUID))
    }

    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), self.size)
    }

    fn refresh_interval(&self) -> Option<Duration> {
        Some(self.refresh_interval)
    }
}

//! Frame statistics, printed once per second when `GPUI_TUI_STATS=1`.

use std::time::{Duration, Instant};

const REPORT_INTERVAL: Duration = Duration::from_secs(1);
const BYTES_PER_KIB: f64 = 1024.0;
const MILLISECONDS_PER_SECOND: f64 = 1000.0;

/// What it took to show one frame.
#[derive(Clone, Copy, Debug)]
pub(crate) struct FrameSample {
    pub render: Duration,
    pub encode: Duration,
    pub bytes: usize,
    pub images_sent: usize,
    pub images_total: usize,
}

/// Sums samples over a reporting interval.
#[derive(Debug, Default)]
pub(crate) struct FrameStats {
    interval_start: Option<Instant>,
    frames: u32,
    render: Duration,
    encode: Duration,
    bytes: usize,
    images_sent: usize,
    images_total: usize,
}

impl FrameStats {
    /// Adds a sample. Returns a report line, and starts a new interval, once an interval has
    /// passed.
    pub(crate) fn record(&mut self, sample: FrameSample, now: Instant) -> Option<String> {
        let interval_start = *self.interval_start.get_or_insert(now);
        self.frames += 1;
        self.render += sample.render;
        self.encode += sample.encode;
        self.bytes += sample.bytes;
        self.images_sent += sample.images_sent;
        self.images_total += sample.images_total;

        let elapsed = now.duration_since(interval_start);
        if elapsed < REPORT_INTERVAL {
            return None;
        }
        let report = self.report(elapsed);
        *self = Self {
            interval_start: Some(now),
            ..Self::default()
        };
        Some(report)
    }

    fn report(&self, elapsed: Duration) -> String {
        let frames = f64::from(self.frames);
        let seconds = elapsed.as_secs_f64();
        let average_ms = |total: Duration| total.as_secs_f64() * MILLISECONDS_PER_SECOND / frames;
        let kib = self.bytes as f64 / BYTES_PER_KIB;
        format!(
            "gpui-tui: {:.1} fps, render {:.1} ms, encode {:.1} ms, {:.1} KiB/frame, \
             {:.0} KiB/s, images {}/{}",
            frames / seconds,
            average_ms(self.render),
            average_ms(self.encode),
            kib / frames,
            kib / seconds,
            self.images_sent,
            self.images_total,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(bytes: usize) -> FrameSample {
        FrameSample {
            render: Duration::from_millis(4),
            encode: Duration::from_millis(2),
            bytes,
            images_sent: 1,
            images_total: 10,
        }
    }

    #[test]
    fn reports_averages_once_per_interval() {
        let mut stats = FrameStats::default();
        let start = Instant::now();
        assert_eq!(stats.record(sample(1024), start), None);
        assert_eq!(
            stats.record(sample(3072), start + Duration::from_millis(500)),
            None
        );
        let report = stats.record(sample(2048), start + REPORT_INTERVAL);
        assert_eq!(
            report.as_deref(),
            Some(
                "gpui-tui: 3.0 fps, render 4.0 ms, encode 2.0 ms, 2.0 KiB/frame, 6 KiB/s, \
                 images 3/30"
            )
        );

        // The next interval starts from the report.
        assert_eq!(
            stats.record(
                sample(1024),
                start + REPORT_INTERVAL * 2 - Duration::from_millis(1)
            ),
            None
        );
    }
}

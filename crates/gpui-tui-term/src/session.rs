//! A terminal session: raw mode, capability probe, and the output writer.

use std::io;
use std::os::fd::BorrowedFd;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use anyhow::{Context as _, bail};

use crate::capabilities::{self, Capabilities};
use crate::geometry::Geometry;
use crate::input::{GridSize, Parser, Reply, TermEvent};
use crate::output_capture::OutputCapture;
use crate::presenter::Transport;
use crate::restore::{self, RestoreState};
use crate::sequences;
use crate::shared_memory;
use crate::tty::Tty;
use crate::writer::OutputWriter;

const DEFAULT_PROBE_TIMEOUT: Duration = Duration::from_secs(2);
const READ_BUFFER_SIZE: usize = 64 * 1024;
/// One RGB pixel for the shared-memory probe.
const PROBE_PIXEL: [u8; 3] = [0, 0, 0];

#[derive(Clone, Debug)]
pub struct SessionOptions {
    /// Use this transport instead of the one the probe picks.
    pub transport: Option<Transport>,
    /// Append stdout and stderr here instead of replaying them after exit.
    pub log_path: Option<PathBuf>,
    pub probe_timeout: Duration,
}

impl Default for SessionOptions {
    fn default() -> Self {
        Self {
            transport: None,
            log_path: None,
            probe_timeout: DEFAULT_PROBE_TIMEOUT,
        }
    }
}

pub struct Session {
    tty: Tty,
    capabilities: Capabilities,
    geometry: Geometry,
    transport: Transport,
    writer: OutputWriter,
    /// User input that arrived while probing.
    pending_events: Vec<TermEvent>,
    read_buffer: Vec<u8>,
}

impl Session {
    /// Takes over the terminal. It is restored when the session drops, on panic, or by
    /// [`restore::restore_terminal`].
    pub fn start(options: &SessionOptions) -> anyhow::Result<Self> {
        let tty = Tty::open()?;
        restore::install(RestoreState {
            output: tty.duplicate_output()?,
            original_mode: tty.original_mode().clone(),
            exit_sequence: Vec::new(),
            captured_output: None,
        });
        // From here on, an early return drops `guard` and restores the terminal.
        let guard = RestoreOnDrop;

        let (capabilities, pending_events) = probe(&tty, options.probe_timeout)?;
        if !capabilities.kitty_graphics {
            bail!(
                "this terminal does not support the kitty graphics protocol; \
                 gpui-tui needs kitty or Ghostty"
            );
        }

        let capture = OutputCapture::start(options.log_path.as_deref())
            .context("failed to redirect stdout and stderr")?;
        restore::set_output_capture(capture);

        let geometry = read_geometry(&tty, capabilities.cell_size)?;
        let transport = options.transport.unwrap_or(if capabilities.shared_memory {
            Transport::SharedMemory
        } else {
            Transport::Direct
        });

        restore::set_exit_sequence(sequences::exit(&capabilities));
        tty.write_all(&sequences::enter(&capabilities))?;
        let writer = OutputWriter::spawn(tty.duplicate_output()?)?;

        std::mem::forget(guard);
        Ok(Self {
            tty,
            capabilities,
            geometry,
            transport,
            writer,
            pending_events,
            read_buffer: vec![0; READ_BUFFER_SIZE],
        })
    }

    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }

    pub fn geometry(&self) -> Geometry {
        self.geometry
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    pub fn input_fd(&self) -> BorrowedFd<'_> {
        self.tty.input_fd()
    }

    /// Input that arrived during the probe, to be handled before anything else.
    pub fn take_pending_events(&mut self) -> Vec<TermEvent> {
        std::mem::take(&mut self.pending_events)
    }

    /// Queues output for the terminal.
    pub fn send(&self, bytes: Vec<u8>) {
        self.writer.send(bytes);
    }

    /// Reads all available input into `parser`.
    pub fn read_events(&mut self, parser: &mut Parser) -> io::Result<Vec<TermEvent>> {
        let mut events = Vec::new();
        loop {
            let count = self.tty.read_available(&mut self.read_buffer)?;
            if count == 0 {
                return Ok(events);
            }
            events.extend(parser.feed(&self.read_buffer[..count]));
        }
    }

    /// Re-reads the window size. Returns whether the geometry changed.
    pub fn refresh_geometry(&mut self) -> io::Result<bool> {
        let geometry = read_geometry(&self.tty, self.capabilities.cell_size)?;
        Ok(self.replace_geometry(geometry))
    }

    /// Applies an in-band resize report. Returns whether the geometry changed.
    pub fn apply_resize(&mut self, size: GridSize) -> bool {
        let geometry = Geometry::new(
            size.cols,
            size.rows,
            size.width_px,
            size.height_px,
            self.capabilities.cell_size,
        );
        self.replace_geometry(geometry)
    }

    fn replace_geometry(&mut self, geometry: Geometry) -> bool {
        let changed = geometry != self.geometry;
        self.geometry = geometry;
        changed
    }

    /// Flushes pending output and restores the terminal.
    pub fn finish(&mut self) {
        self.writer.finish();
        restore::restore_terminal();
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.finish();
    }
}

struct RestoreOnDrop;

impl Drop for RestoreOnDrop {
    fn drop(&mut self) {
        restore::restore_terminal();
    }
}

fn read_geometry(tty: &Tty, reported_cell_size: Option<(u16, u16)>) -> io::Result<Geometry> {
    let size = tty.window_size()?;
    Ok(Geometry::new(
        size.ws_col,
        size.ws_row,
        size.ws_xpixel,
        size.ws_ypixel,
        reported_cell_size,
    ))
}

/// Sends the capability queries and collects replies until the device attributes reply (or the
/// timeout). Returns the capabilities and any user input that arrived meanwhile.
fn probe(tty: &Tty, timeout: Duration) -> anyhow::Result<(Capabilities, Vec<TermEvent>)> {
    let probe_segment = format!("/gpui-tui-{}-probe", std::process::id());
    let shared_memory_name = match shared_memory::write_segment(&probe_segment, &PROBE_PIXEL) {
        Ok(()) => Some(probe_segment.as_str()),
        Err(error) => {
            log::warn!("shared memory unavailable, frames will be sent inline: {error}");
            None
        }
    };
    tty.write_all(&capabilities::probe_queries(shared_memory_name))?;

    let replies = collect_probe_replies(tty, timeout);
    shared_memory::unlink_segment(&probe_segment);
    let (replies, other_events) = replies?;
    Ok((Capabilities::from_replies(&replies), other_events))
}

fn collect_probe_replies(tty: &Tty, timeout: Duration) -> io::Result<(Vec<Reply>, Vec<TermEvent>)> {
    let deadline = Instant::now() + timeout;
    let mut parser = Parser::new();
    let mut buffer = vec![0; READ_BUFFER_SIZE];
    let mut replies = Vec::new();
    let mut other_events = Vec::new();

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            log::warn!("terminal did not answer all capability queries in time");
            return Ok((replies, other_events));
        }
        if !tty.wait_for_input(remaining)? {
            continue;
        }
        let count = tty.read_available(&mut buffer)?;
        for event in parser.feed(&buffer[..count]) {
            match event {
                TermEvent::Reply(Reply::PrimaryDeviceAttributes) => {
                    return Ok((replies, other_events));
                }
                TermEvent::Reply(reply) => replies.push(reply),
                other => other_events.push(other),
            }
        }
    }
}

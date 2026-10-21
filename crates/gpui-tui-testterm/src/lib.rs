//! A fake kitty-compatible terminal for end-to-end tests of gpui-tui apps.
//!
//! It runs the app on a pseudo-terminal, answers the capability queries, decodes the frames it
//! sends (shared memory or direct), acknowledges them, and lets tests inject input.

mod kitty;
mod output;
mod pty;

use std::collections::HashSet;
use std::io::{Read as _, Write as _};
use std::process::{Command, ExitStatus};
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result, bail};
use image::RgbaImage;
use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::termios::Winsize;

use crate::kitty::Assembler;
use crate::output::{OutputParser, Sequence};
use crate::pty::Pty;

const MODE_SGR_PIXELS: u16 = 1016;
const MODE_IN_BAND_RESIZE: u16 = 2048;
const MODE_ALTERNATE_SCREEN: u16 = 1049;
/// Modes this terminal claims to support when asked with DECRQM.
const SUPPORTED_MODES: [u16; 3] = [MODE_SGR_PIXELS, 2026, MODE_IN_BAND_RESIZE];
const READ_CHUNK: usize = 64 * 1024;

#[derive(Clone, Debug)]
pub struct TerminalConfig {
    pub columns: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
    /// Whether the kitty graphics protocol is supported at all.
    pub kitty_graphics: bool,
    /// Whether frames in shared memory can be read (true for a local terminal).
    pub shared_memory: bool,
    /// Reported default background, as `rgb:` hex components.
    pub background: [u8; 3],
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            columns: 80,
            rows: 24,
            cell_width: 10,
            cell_height: 20,
            kitty_graphics: true,
            shared_memory: true,
            background: [0x1e, 0x1e, 0x2e],
        }
    }
}

/// The latest frame, as the terminal would display it.
pub struct Screen {
    pub image: RgbaImage,
    pub columns: u32,
    pub rows: u32,
}

impl Screen {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.image.get_pixel(x, y).0
    }
}

pub struct FakeTerminal {
    pty: Pty,
    config: TerminalConfig,
    parser: OutputParser,
    assembler: Assembler,
    screen: Option<Screen>,
    frame_count: usize,
    enabled_modes: HashSet<u16>,
    restored: bool,
    exit_status: Option<ExitStatus>,
    errors: Vec<String>,
    /// Text printed since the screen was last cleared.
    text: Vec<u8>,
}

impl FakeTerminal {
    pub fn spawn(command: Command, config: TerminalConfig) -> Result<Self> {
        let pty = Pty::spawn(command, winsize(&config))?;
        Ok(Self {
            pty,
            config,
            parser: OutputParser::new(),
            assembler: Assembler::default(),
            screen: None,
            frame_count: 0,
            enabled_modes: HashSet::new(),
            restored: false,
            exit_status: None,
            errors: Vec::new(),
            text: Vec::new(),
        })
    }

    pub fn screen(&self) -> Option<&Screen> {
        self.screen.as_ref()
    }

    /// Process ID of the app.
    pub fn app_pid(&self) -> u32 {
        self.pty.child.id()
    }

    /// Keeps processing output for `duration`, e.g. to observe an idle app.
    pub fn pump_for(&mut self, duration: Duration) -> Result<()> {
        let deadline = Instant::now() + duration;
        loop {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() || !self.pump(remaining)? {
                return Ok(());
            }
        }
    }

    pub fn frame_count(&self) -> usize {
        self.frame_count
    }

    /// Whether the app switched back from the alternate screen (restored the terminal).
    pub fn is_restored(&self) -> bool {
        self.restored
    }

    /// Text the app printed since it last cleared the screen, without any styling.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.text).into_owned()
    }

    /// Problems decoding the app's output, such as a frame that could not be read.
    pub fn errors(&self) -> &[String] {
        &self.errors
    }

    /// Reads and processes output until `condition` holds.
    pub fn wait_until(
        &mut self,
        timeout: Duration,
        description: &str,
        mut condition: impl FnMut(&Self) -> bool,
    ) -> Result<()> {
        let deadline = Instant::now() + timeout;
        while !condition(self) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                bail!(
                    "timed out waiting for {description}; errors: {:?}",
                    self.errors
                );
            }
            if !self.pump(remaining.min(Duration::from_millis(50)))? {
                if condition(self) {
                    return Ok(());
                }
                bail!(
                    "the app exited while waiting for {description}; errors: {:?}",
                    self.errors
                );
            }
        }
        Ok(())
    }

    /// Waits for the next frame after the current one.
    pub fn wait_for_new_frame(&mut self, timeout: Duration) -> Result<()> {
        let count = self.frame_count;
        self.wait_until(timeout, "a new frame", |terminal| {
            terminal.frame_count > count
        })
    }

    pub fn wait_for_exit(&mut self, timeout: Duration) -> Result<ExitStatus> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.pty.child.try_wait()? {
                // Drain the rest of the output, e.g. the restore sequence.
                while self.pump(Duration::from_millis(10))? {}
                self.exit_status = Some(status);
                return Ok(status);
            }
            if Instant::now() >= deadline {
                bail!("timed out waiting for the app to exit");
            }
            self.pump(Duration::from_millis(20))?;
        }
    }

    pub fn send(&mut self, bytes: &[u8]) -> Result<()> {
        self.pty.controller.write_all(bytes)?;
        Ok(self.pty.controller.flush()?)
    }

    /// Clicks the left button at a pixel position of the screen.
    pub fn click(&mut self, x: u32, y: u32) -> Result<()> {
        let (column, row) = self.mouse_coordinates(x, y);
        self.send(format!("\x1b[<35;{column};{row}M").as_bytes())?;
        self.send(format!("\x1b[<0;{column};{row}M").as_bytes())?;
        self.send(format!("\x1b[<0;{column};{row}m").as_bytes())
    }

    /// Types a character, kitty keyboard protocol style (with associated text).
    pub fn type_char(&mut self, character: char) -> Result<()> {
        let code = u32::from(character);
        self.send(format!("\x1b[{code};1;{code}u").as_bytes())
    }

    pub fn press_enter(&mut self) -> Result<()> {
        self.send(b"\x1b[13u")
    }

    pub fn press_control(&mut self, character: char) -> Result<()> {
        let code = u32::from(character);
        self.send(format!("\x1b[{code};5u").as_bytes())
    }

    /// Changes the grid size and reports it like a terminal does (SIGWINCH, mode 2048).
    pub fn resize(&mut self, columns: u16, rows: u16) -> Result<()> {
        self.config.columns = columns;
        self.config.rows = rows;
        self.pty.resize(winsize(&self.config))?;
        if self.enabled_modes.contains(&MODE_IN_BAND_RESIZE) {
            let report = format!(
                "\x1b[48;{rows};{columns};{};{}t",
                u32::from(rows) * u32::from(self.config.cell_height),
                u32::from(columns) * u32::from(self.config.cell_width),
            );
            self.send(report.as_bytes())?;
        }
        Ok(())
    }

    fn mouse_coordinates(&self, x: u32, y: u32) -> (u32, u32) {
        if self.enabled_modes.contains(&MODE_SGR_PIXELS) {
            (x + 1, y + 1)
        } else {
            (
                x / u32::from(self.config.cell_width) + 1,
                y / u32::from(self.config.cell_height) + 1,
            )
        }
    }

    /// Reads available output once. Returns false when the app closed the terminal.
    fn pump(&mut self, timeout: Duration) -> Result<bool> {
        let timeout = Timespec::try_from(timeout)?;
        let mut fds = [PollFd::new(&self.pty.controller, PollFlags::IN)];
        let ready = rustix::event::poll(&mut fds, Some(&timeout))?;
        if ready == 0 {
            return Ok(true);
        }
        let mut buffer = vec![0; READ_CHUNK];
        let count = match self.pty.controller.read(&mut buffer) {
            Ok(0) => return Ok(false),
            Ok(count) => count,
            // EIO: every process closed the terminal side.
            Err(error) if error.raw_os_error() == Some(rustix::io::Errno::IO.raw_os_error()) => {
                return Ok(false);
            }
            Err(error) => return Err(error).context("failed to read app output"),
        };
        for sequence in self.parser.feed(&buffer[..count]) {
            self.handle(sequence)?;
        }
        Ok(true)
    }

    fn handle(&mut self, sequence: Sequence) -> Result<()> {
        match sequence {
            Sequence::Csi { body, final_byte } => self.handle_csi(&body, final_byte),
            Sequence::Osc(payload) if payload == b"11;?" => {
                let [red, green, blue] = self.config.background;
                let reply = format!(
                    "\x1b]11;rgb:{red:02x}{red:02x}/{green:02x}{green:02x}/{blue:02x}{blue:02x}\x1b\\"
                );
                self.send(reply.as_bytes())
            }
            Sequence::Osc(_) => Ok(()),
            Sequence::Apc(payload) => self.handle_graphics(&payload),
            Sequence::Print(byte) => {
                self.text.push(byte);
                Ok(())
            }
        }
    }

    fn handle_csi(&mut self, body: &[u8], final_byte: u8) -> Result<()> {
        let body = String::from_utf8_lossy(body);
        match (body.as_ref(), final_byte) {
            ("?", b'u') => self.send(b"\x1b[?0u"),
            ("16", b't') => {
                let reply = format!(
                    "\x1b[6;{};{}t",
                    self.config.cell_height, self.config.cell_width
                );
                self.send(reply.as_bytes())
            }
            ("", b'c') => self.send(b"\x1b[?62;22c"),
            ("2", b'J') => {
                self.text.clear();
                Ok(())
            }
            (query, b'p') if query.starts_with('?') && query.ends_with('$') => {
                let mode: u16 = query
                    .trim_start_matches('?')
                    .trim_end_matches('$')
                    .parse()
                    .unwrap_or(0);
                let state = if SUPPORTED_MODES.contains(&mode) {
                    2
                } else {
                    0
                };
                self.send(format!("\x1b[?{mode};{state}$y").as_bytes())
            }
            (modes, b'h' | b'l') if modes.starts_with('?') => {
                for mode in modes.trim_start_matches('?').split(';') {
                    let Ok(mode) = mode.parse::<u16>() else {
                        continue;
                    };
                    if final_byte == b'h' {
                        self.enabled_modes.insert(mode);
                    } else {
                        self.enabled_modes.remove(&mode);
                        self.restored |= mode == MODE_ALTERNATE_SCREEN;
                    }
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }

    fn handle_graphics(&mut self, payload: &[u8]) -> Result<()> {
        let Some(command) = kitty::Command::parse(payload) else {
            return Ok(());
        };
        let Some(command) = self.assembler.push(command) else {
            return Ok(());
        };
        match command.action() {
            "q" => {
                let medium_supported = match command.key("t") {
                    Some("s") => self.config.shared_memory,
                    _ => self.config.kitty_graphics,
                };
                let readable = medium_supported && kitty::load_payload(&command).is_ok();
                let message = if readable {
                    "OK"
                } else {
                    "ENOENT:not readable"
                };
                let reply = format!("\x1b_Gi={};{message}\x1b\\", command.image_id());
                self.send(reply.as_bytes())
            }
            "T" | "t" => {
                match kitty::decode_image(&command) {
                    Ok(decoded) => {
                        self.screen = Some(Screen {
                            image: decoded.image,
                            columns: decoded.columns,
                            rows: decoded.rows,
                        });
                        self.frame_count += 1;
                    }
                    Err(error) => self.errors.push(format!("{error:#}")),
                }
                Ok(())
            }
            "d" => {
                self.screen = None;
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl Drop for FakeTerminal {
    /// Kills an app a failed test left running.
    fn drop(&mut self) {
        if self.exit_status.is_some() || matches!(self.pty.child.try_wait(), Ok(Some(_))) {
            return;
        }
        if let Err(error) = self
            .pty
            .child
            .kill()
            .and_then(|()| self.pty.child.wait().map(drop))
        {
            eprintln!("failed to stop the app under test: {error}");
        }
    }
}

fn winsize(config: &TerminalConfig) -> Winsize {
    Winsize {
        ws_row: config.rows,
        ws_col: config.columns,
        ws_xpixel: config.columns * config.cell_width,
        ws_ypixel: config.rows * config.cell_height,
    }
}

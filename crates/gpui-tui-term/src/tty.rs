//! The controlling terminal: raw mode, size, and non-blocking input.

use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::{AsFd, BorrowedFd, OwnedFd};
use std::time::Duration;

use rustix::event::{PollFd, PollFlags, Timespec};
use rustix::fs::{Mode, OFlags};
use rustix::termios::{self, OptionalActions, Termios, Winsize};

const TTY_PATH: &str = "/dev/tty";

/// The controlling terminal, switched to raw mode while this value lives.
///
/// Input and output use separate open file descriptions so that input can be non-blocking
/// while output (on the writer thread) blocks.
pub struct Tty {
    input: File,
    output: File,
    original_mode: Termios,
}

impl Tty {
    pub fn open() -> io::Result<Self> {
        let input = open_tty(OFlags::NONBLOCK)?;
        let output = open_tty(OFlags::empty())?;
        let original_mode = termios::tcgetattr(&output)?;

        let mut raw_mode = original_mode.clone();
        raw_mode.make_raw();
        termios::tcsetattr(&output, OptionalActions::Now, &raw_mode)?;

        Ok(Self {
            input: File::from(input),
            output: File::from(output),
            original_mode,
        })
    }

    pub fn original_mode(&self) -> &Termios {
        &self.original_mode
    }

    pub fn input_fd(&self) -> BorrowedFd<'_> {
        self.input.as_fd()
    }

    /// A new handle to the output, for the writer thread and the restore path.
    pub fn duplicate_output(&self) -> io::Result<OwnedFd> {
        Ok(rustix::io::dup(&self.output)?)
    }

    pub fn window_size(&self) -> io::Result<Winsize> {
        Ok(termios::tcgetwinsize(&self.output)?)
    }

    /// Writes synchronously. Only for use before the writer thread starts.
    pub fn write_all(&self, bytes: &[u8]) -> io::Result<()> {
        let mut output = &self.output;
        output.write_all(bytes)?;
        output.flush()
    }

    /// Reads whatever input is available without blocking. Returns 0 when there is none.
    pub fn read_available(&self, buffer: &mut [u8]) -> io::Result<usize> {
        read_nonblocking(&self.input, buffer)
    }

    /// Waits until input is available or `timeout` passes. Returns whether input is available.
    pub fn wait_for_input(&self, timeout: Duration) -> io::Result<bool> {
        let timeout = Timespec::try_from(timeout).map_err(io::Error::other)?;
        let mut fds = [PollFd::new(&self.input, PollFlags::IN)];
        match rustix::event::poll(&mut fds, Some(&timeout)) {
            Ok(ready) => Ok(ready > 0),
            Err(rustix::io::Errno::INTR) => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

/// Reads without blocking. Returns 0 when no input is waiting, and an error when the other side
/// hung up (e.g. the SSH connection dropped), so the app quits instead of polling a dead
/// terminal forever.
fn read_nonblocking(mut input: impl io::Read, buffer: &mut [u8]) -> io::Result<usize> {
    match input.read(buffer) {
        Ok(0) if !buffer.is_empty() => Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "the terminal was closed",
        )),
        Ok(count) => Ok(count),
        Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(0),
        Err(error) if error.kind() == io::ErrorKind::Interrupted => Ok(0),
        Err(error) => Err(error),
    }
}

fn open_tty(extra_flags: OFlags) -> io::Result<OwnedFd> {
    let flags = OFlags::RDWR | OFlags::NOCTTY | OFlags::CLOEXEC | extra_flags;
    rustix::fs::open(TTY_PATH, flags, Mode::empty()).map_err(|error| {
        io::Error::new(
            io::ErrorKind::NotFound,
            format!("cannot open {TTY_PATH} (is this running in a terminal?): {error}"),
        )
    })
}

/// Restores a terminal mode saved by [`Tty::original_mode`].
pub fn restore_mode(output: impl AsFd, mode: &Termios) -> io::Result<()> {
    Ok(termios::tcsetattr(output, OptionalActions::Now, mode)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hang_up_is_an_error_not_an_empty_read() -> io::Result<()> {
        let (reader, mut writer) = io::pipe()?;
        writer.write_all(b"x")?;
        drop(writer);

        let mut buffer = [0; 8];
        assert_eq!(read_nonblocking(&reader, &mut buffer)?, 1);
        let error = read_nonblocking(&reader, &mut buffer).err();
        assert_eq!(
            error.map(|error| error.kind()),
            Some(io::ErrorKind::UnexpectedEof)
        );
        Ok(())
    }
}

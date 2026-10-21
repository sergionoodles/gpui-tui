//! Redirects stdout and stderr away from the terminal while it shows frames.
//!
//! Anything the app prints (logs, panic messages) would land on top of the image. Output goes
//! to a log file if one is configured, otherwise into an anonymous temporary file that is
//! replayed on the real stderr after the terminal is restored. Terminal I/O itself goes through
//! `/dev/tty`, so it is unaffected.

use std::fs::{File, OpenOptions};
use std::io::{self, Seek as _, SeekFrom};
use std::os::fd::OwnedFd;
use std::path::Path;

pub struct OutputCapture {
    original_stdout: OwnedFd,
    original_stderr: OwnedFd,
    destination: File,
    replay_on_finish: bool,
}

impl OutputCapture {
    pub fn start(log_path: Option<&Path>) -> io::Result<Self> {
        let (destination, replay_on_finish) = match log_path {
            Some(path) => {
                let file = OpenOptions::new().create(true).append(true).open(path)?;
                (file, false)
            }
            None => (anonymous_temp_file()?, true),
        };
        let original_stdout = rustix::io::dup(io::stdout())?;
        let original_stderr = rustix::io::dup(io::stderr())?;
        rustix::stdio::dup2_stdout(&destination)?;
        rustix::stdio::dup2_stderr(&destination)?;

        Ok(Self {
            original_stdout,
            original_stderr,
            destination,
            replay_on_finish,
        })
    }

    /// Points stdout and stderr back where they were and replays what was captured.
    pub fn finish(mut self) -> io::Result<()> {
        rustix::stdio::dup2_stdout(&self.original_stdout)?;
        rustix::stdio::dup2_stderr(&self.original_stderr)?;
        if self.replay_on_finish {
            self.destination.seek(SeekFrom::Start(0))?;
            io::copy(&mut self.destination, &mut io::stderr())?;
        }
        Ok(())
    }
}

/// A read-write file with no name, so nothing is left behind if the process dies.
fn anonymous_temp_file() -> io::Result<File> {
    let path = std::env::temp_dir().join(format!("gpui-tui-output-{}.log", std::process::id()));
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&path)?;
    std::fs::remove_file(&path)?;
    Ok(file)
}

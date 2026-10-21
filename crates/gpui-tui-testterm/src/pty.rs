//! Spawning a child process on a new pseudo-terminal.

use std::fs::File;
use std::os::fd::OwnedFd;
use std::os::unix::process::CommandExt as _;
use std::process::{Child, Command};

use anyhow::{Context as _, Result};
use rustix::fs::{Mode, OFlags};
use rustix::pty::{self, OpenptFlags};
use rustix::termios::{self, Winsize};

/// The controller side of a pty with a child running on it.
pub(crate) struct Pty {
    pub(crate) controller: File,
    pub(crate) child: Child,
}

impl Pty {
    /// Runs `command` with the pty as its controlling terminal and stdio.
    pub(crate) fn spawn(mut command: Command, size: Winsize) -> Result<Self> {
        let controller =
            pty::openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY | OpenptFlags::CLOEXEC)
                .context("failed to open a pty")?;
        pty::grantpt(&controller)?;
        pty::unlockpt(&controller)?;
        let name = pty::ptsname(&controller, Vec::new())?;
        termios::tcsetwinsize(&controller, size)?;

        let terminal: OwnedFd = rustix::fs::open(
            name.as_c_str(),
            OFlags::RDWR | OFlags::NOCTTY,
            Mode::empty(),
        )?;
        let terminal = File::from(terminal);
        command
            .stdin(terminal.try_clone()?)
            .stdout(terminal.try_clone()?)
            .stderr(terminal);
        // SAFETY: setsid and the TIOCSCTTY ioctl are async-signal-safe syscalls, as required
        // between fork and exec; nothing here allocates or takes locks.
        unsafe {
            command.pre_exec(|| {
                rustix::process::setsid()?;
                rustix::process::ioctl_tiocsctty(std::io::stdin())?;
                Ok(())
            });
        }
        let child = command.spawn().context("failed to spawn the child")?;

        Ok(Self {
            controller: File::from(controller),
            child,
        })
    }

    pub(crate) fn resize(&self, size: Winsize) -> Result<()> {
        Ok(termios::tcsetwinsize(&self.controller, size)?)
    }
}

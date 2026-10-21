//! Background thread that owns terminal output, so slow writes never block the UI thread.

use std::fs::File;
use std::io::{self, Write as _};
use std::os::fd::OwnedFd;
use std::sync::mpsc::{self, SyncSender};
use std::thread::{self, JoinHandle};

/// Writes queued before `send` blocks. Frames are large, so keep this small.
const QUEUE_CAPACITY: usize = 4;

pub struct OutputWriter {
    sender: Option<SyncSender<Vec<u8>>>,
    thread: Option<JoinHandle<()>>,
}

impl OutputWriter {
    pub fn spawn(output: OwnedFd) -> io::Result<Self> {
        let (sender, receiver) = mpsc::sync_channel::<Vec<u8>>(QUEUE_CAPACITY);
        let mut output = File::from(output);
        let thread = thread::Builder::new()
            .name("gpui-tui-writer".to_owned())
            .spawn(move || {
                for bytes in receiver {
                    if let Err(error) = output.write_all(&bytes).and_then(|()| output.flush()) {
                        log::error!("terminal output failed, stopping writer: {error}");
                        return;
                    }
                }
            })?;

        Ok(Self {
            sender: Some(sender),
            thread: Some(thread),
        })
    }

    /// Queues bytes for the terminal, in order. Blocks while the queue is full.
    pub fn send(&self, bytes: Vec<u8>) {
        if bytes.is_empty() {
            return;
        }
        let Some(sender) = &self.sender else {
            return;
        };
        if sender.send(bytes).is_err() {
            log::debug!("dropping terminal output: the writer thread has stopped");
        }
    }

    /// Flushes everything queued and stops the thread.
    pub fn finish(&mut self) {
        self.sender.take();
        if let Some(thread) = self.thread.take()
            && thread.join().is_err()
        {
            log::error!("the terminal writer thread panicked");
        }
    }
}

impl Drop for OutputWriter {
    fn drop(&mut self) {
        self.finish();
    }
}

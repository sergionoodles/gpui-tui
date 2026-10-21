//! Puts the terminal back the way we found it: on normal exit and from the panic hook.

use std::fs::File;
use std::io::Write as _;
use std::os::fd::OwnedFd;
use std::sync::{Mutex, Once};

use rustix::termios::Termios;

use crate::output_capture::OutputCapture;
use crate::tty;

/// Everything needed to restore the terminal without access to the session.
pub(crate) struct RestoreState {
    pub output: OwnedFd,
    pub original_mode: Termios,
    pub exit_sequence: Vec<u8>,
    pub captured_output: Option<OutputCapture>,
}

static RESTORE_STATE: Mutex<Option<RestoreState>> = Mutex::new(None);
static PANIC_HOOK: Once = Once::new();

pub(crate) fn install(state: RestoreState) {
    *lock() = Some(state);
    install_panic_hook();
}

pub(crate) fn set_exit_sequence(exit_sequence: Vec<u8>) {
    if let Some(state) = lock().as_mut() {
        state.exit_sequence = exit_sequence;
    }
}

pub(crate) fn set_output_capture(capture: OutputCapture) {
    if let Some(state) = lock().as_mut() {
        state.captured_output = Some(capture);
    }
}

/// Restores the terminal once; later calls do nothing.
pub fn restore_terminal() {
    let Some(state) = lock().take() else {
        return;
    };
    let mut output = File::from(state.output);
    if let Err(error) = output
        .write_all(&state.exit_sequence)
        .and_then(|()| output.flush())
    {
        log::error!("failed to reset terminal modes: {error}");
    }
    if let Err(error) = tty::restore_mode(&output, &state.original_mode) {
        log::error!("failed to restore the terminal mode: {error}");
    }
    if let Some(capture) = state.captured_output
        && let Err(error) = capture.finish()
    {
        eprintln!("gpui-tui: failed to restore stdout and stderr: {error}");
    }
}

/// Restores the terminal before the panic message is printed, so it is readable.
fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let previous_hook = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            restore_terminal();
            previous_hook(info);
        }));
    });
}

fn lock() -> std::sync::MutexGuard<'static, Option<RestoreState>> {
    RESTORE_STATE
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

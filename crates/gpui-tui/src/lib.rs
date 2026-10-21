//! Run a GPUI app inside a terminal that supports the kitty graphics protocol (kitty, Ghostty).
//!
//! GPUI renders each frame offscreen; the frame is shown as an image in the terminal, and
//! terminal keyboard and mouse input is fed back to GPUI.
//!
//! ```no_run
//! let (use_tui, _args) = gpui_tui::take_tui_flag(std::env::args_os());
//! let app = if use_tui {
//!     gpui_tui::application()
//! } else {
//!     gpui_platform::application()
//! };
//! # mod gpui_platform { pub fn application() -> gpui::Application { unimplemented!() } }
//! ```

mod cursor;
mod dispatcher;
mod display;
mod input;
mod options;
mod platform;
mod runtime;
mod window;

use std::rc::Rc;

pub use gpui_tui_term::Transport;
pub use options::{Options, TUI_ENV, TUI_FLAG, take_tui_flag};

/// A GPUI application that runs in the current terminal, configured from the environment
/// (see [`Options::from_env`]).
///
/// Exits the process with an error message if there is no suitable terminal. Use
/// [`try_application`] to handle that case yourself.
pub fn application() -> gpui::Application {
    match try_application(Options::from_env()) {
        Ok(application) => application,
        Err(error) => {
            eprintln!("gpui-tui: {error:#}");
            std::process::exit(1);
        }
    }
}

/// A GPUI application that runs in the current terminal.
///
/// Takes over the terminal immediately; it is restored when the application quits or panics.
/// Fails if there is no terminal or it does not support the kitty graphics protocol.
pub fn try_application(options: Options) -> anyhow::Result<gpui::Application> {
    let platform = platform::TuiPlatform::new(options)?;
    Ok(gpui::Application::with_platform(Rc::new(platform)))
}

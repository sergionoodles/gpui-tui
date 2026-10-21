//! Configuration of the terminal platform, from code or environment variables.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

use gpui_tui_term::Transport;

/// Command-line flag that selects the terminal platform.
pub const TUI_FLAG: &str = "--tui";
/// Environment variable that selects the terminal platform when set to `1`.
pub const TUI_ENV: &str = "GPUI_TUI";

const SCALE_ENV: &str = "GPUI_TUI_SCALE";
const TRANSPORT_ENV: &str = "GPUI_TUI_TRANSPORT";
const LOG_ENV: &str = "GPUI_TUI_LOG";
const MAX_FPS_ENV: &str = "GPUI_TUI_MAX_FPS";
const RENDER_SCALE_ENV: &str = "GPUI_TUI_RENDER_SCALE";
const STATS_ENV: &str = "GPUI_TUI_STATS";
/// Frame rate cap when frames go through shared memory (the terminal is on this machine).
const LOCAL_MAX_FPS: u32 = 60;
/// Frame rate cap when frames are sent inline, usually over SSH: fewer frames to render in
/// software and to send.
const REMOTE_MAX_FPS: u32 = 30;

#[derive(Clone, Debug)]
pub struct Options {
    /// GPUI scale factor. `None` guesses from the cell height, as terminals do not report DPI.
    pub scale_factor: Option<f32>,
    /// Frame rate cap. `None` means 60 over shared memory and 30 when frames are sent inline.
    pub max_frames_per_second: Option<u32>,
    /// Fraction of the terminal's resolution to render at, in `(0, 1]`; the terminal scales
    /// the frames up. Lower values send fewer bytes and render faster, but look blurrier.
    ///
    /// `None` renders at full resolution over shared memory. When frames are sent inline, it
    /// renders one device pixel per GPUI pixel, which halves each side on a high-density
    /// display.
    pub render_scale: Option<f32>,
    /// Print frame statistics (fps, render and encode times, bytes) to stderr every second.
    pub stats: bool,
    /// Quit when Ctrl+C reaches the platform without the app handling it.
    pub quit_on_unhandled_ctrl_c: bool,
    /// Frame transport. `None` picks shared memory when the terminal can read it.
    pub transport: Option<Transport>,
    /// Append stdout and stderr to this file. `None` buffers them and prints them after exit.
    pub log_path: Option<PathBuf>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            scale_factor: None,
            max_frames_per_second: None,
            render_scale: None,
            stats: false,
            quit_on_unhandled_ctrl_c: true,
            transport: None,
            log_path: None,
        }
    }
}

impl Options {
    /// Defaults overridden by `GPUI_TUI_SCALE`, `GPUI_TUI_TRANSPORT` (`shm` or `direct`),
    /// `GPUI_TUI_MAX_FPS`, `GPUI_TUI_RENDER_SCALE`, `GPUI_TUI_STATS=1`, and `GPUI_TUI_LOG`.
    pub fn from_env() -> Self {
        Self::from_vars(|name| std::env::var(name).ok())
    }

    fn from_vars(var: impl Fn(&str) -> Option<String>) -> Self {
        let defaults = Self::default();
        let scale_factor = var(SCALE_ENV)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|scale| *scale > 0.0);
        let transport = var(TRANSPORT_ENV).and_then(|value| match value.as_str() {
            "shm" => Some(Transport::SharedMemory),
            "direct" => Some(Transport::Direct),
            _ => {
                log::warn!("ignoring unknown {TRANSPORT_ENV} value {value:?}");
                None
            }
        });
        let max_frames_per_second = var(MAX_FPS_ENV)
            .and_then(|value| value.parse::<u32>().ok())
            .filter(|fps| *fps > 0);
        let render_scale = var(RENDER_SCALE_ENV)
            .and_then(|value| value.parse::<f32>().ok())
            .filter(|scale| *scale > 0.0 && *scale <= 1.0);

        Self {
            scale_factor,
            max_frames_per_second,
            render_scale,
            stats: var(STATS_ENV).is_some_and(|value| value == "1"),
            transport,
            log_path: var(LOG_ENV).map(PathBuf::from),
            ..defaults
        }
    }

    /// The frame rate cap for frames sent over `transport`.
    pub(crate) fn frame_rate_for(&self, transport: Transport) -> u32 {
        let default = match transport {
            Transport::SharedMemory => LOCAL_MAX_FPS,
            Transport::Direct => REMOTE_MAX_FPS,
        };
        self.max_frames_per_second.unwrap_or(default).max(1)
    }

    /// The render scale for frames sent over `transport` to a display with `display_scale`
    /// device pixels per GPUI pixel.
    pub(crate) fn render_scale_for(&self, transport: Transport, display_scale: f32) -> f32 {
        if let Some(render_scale) = self.render_scale {
            return render_scale;
        }
        match transport {
            Transport::SharedMemory => 1.0,
            Transport::Direct => (1.0 / display_scale).min(1.0),
        }
    }
}

/// Whether the terminal platform was requested, plus `args` without the `--tui` flag.
///
/// Requested means `--tui` is among the arguments or `GPUI_TUI=1` is set. The first argument
/// (the program name) is never treated as the flag.
pub fn take_tui_flag(args: impl IntoIterator<Item = OsString>) -> (bool, Vec<OsString>) {
    split_tui_flag(args, std::env::var_os(TUI_ENV).as_deref())
}

fn split_tui_flag(
    args: impl IntoIterator<Item = OsString>,
    env_value: Option<&OsStr>,
) -> (bool, Vec<OsString>) {
    let mut args = args.into_iter();
    let program = args.next();
    let mut flag_present = false;
    let mut remaining: Vec<OsString> = program.into_iter().collect();
    for arg in args {
        if arg == TUI_FLAG {
            flag_present = true;
        } else {
            remaining.push(arg);
        }
    }
    let env_enabled = env_value.is_some_and(|value| value == "1");
    (flag_present || env_enabled, remaining)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<OsString> {
        values.iter().map(OsString::from).collect()
    }

    #[test]
    fn flag_is_removed_and_reported() {
        let (requested, rest) = split_tui_flag(args(&["app", "--tui", "workspace"]), None);
        assert!(requested);
        assert_eq!(rest, args(&["app", "workspace"]));
    }

    #[test]
    fn environment_variable_requests_tui() {
        let (requested, rest) = split_tui_flag(args(&["app"]), Some(OsStr::new("1")));
        assert!(requested);
        assert_eq!(rest, args(&["app"]));

        let (requested, _) = split_tui_flag(args(&["app"]), Some(OsStr::new("0")));
        assert!(!requested);
    }

    #[test]
    fn program_name_is_never_the_flag() {
        let (requested, rest) = split_tui_flag(args(&["--tui"]), None);
        assert!(!requested);
        assert_eq!(rest, args(&["--tui"]));
    }

    #[test]
    fn environment_overrides() {
        let options = Options::from_vars(|name| match name {
            SCALE_ENV => Some("1.5".to_owned()),
            TRANSPORT_ENV => Some("direct".to_owned()),
            MAX_FPS_ENV => Some("0".to_owned()),
            RENDER_SCALE_ENV => Some("1.5".to_owned()),
            STATS_ENV => Some("1".to_owned()),
            _ => None,
        });
        assert_eq!(options.scale_factor, Some(1.5));
        assert_eq!(options.transport, Some(Transport::Direct));
        assert_eq!(options.max_frames_per_second, None);
        assert_eq!(
            options.render_scale, None,
            "render scale above 1 is ignored"
        );
        assert!(options.stats);
        assert_eq!(options.log_path, None);
    }

    #[test]
    fn inline_frames_default_to_fewer_frames_and_pixels() {
        let options = Options::default();
        assert_eq!(
            options.frame_rate_for(Transport::SharedMemory),
            LOCAL_MAX_FPS
        );
        assert_eq!(options.frame_rate_for(Transport::Direct), REMOTE_MAX_FPS);

        assert_eq!(options.render_scale_for(Transport::SharedMemory, 2.0), 1.0);
        assert_eq!(options.render_scale_for(Transport::Direct, 2.0), 0.5);
        assert_eq!(options.render_scale_for(Transport::Direct, 1.0), 1.0);
    }

    #[test]
    fn explicit_settings_apply_to_every_transport() {
        let options = Options {
            max_frames_per_second: Some(20),
            render_scale: Some(0.75),
            ..Options::default()
        };
        assert_eq!(options.frame_rate_for(Transport::SharedMemory), 20);
        assert_eq!(options.render_scale_for(Transport::SharedMemory, 2.0), 0.75);
        assert_eq!(options.render_scale_for(Transport::Direct, 1.0), 0.75);
    }
}

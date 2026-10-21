//! End-to-end tests: a GPUI app runs on gpui-tui inside a fake kitty-compatible terminal.
//!
//! The test binary re-runs itself as the app (GPUI must own the main thread, so this uses a
//! custom harness). The app shows two halves: clicking the left one, typing, and picking a file
//! change their colors, which the tests read back from the decoded frames.

use std::path::PathBuf;
use std::process::{Command, ExitCode};
use std::time::Duration;

use anyhow::{Context as _, Result, ensure};
use gpui::{
    App, Context, FocusHandle, KeyDownEvent, PathPromptOptions, Rgba, Window, WindowOptions, div,
    prelude::*, rgb,
};
use gpui_tui_testterm::{FakeTerminal, Screen, TerminalConfig};

const CHILD_ENV: &str = "GPUI_TUI_E2E_APP";
const TIMEOUT: Duration = Duration::from_secs(20);
const COLOR_TOLERANCE: u8 = 2;
/// Time for startup work (font loading, first frames) to finish before measuring idle.
const SETTLE_TIME: Duration = Duration::from_millis(500);
const IDLE_TIME: Duration = Duration::from_secs(2);
/// CPU an idle app may use over `IDLE_TIME`: 2.5%.
const MAX_IDLE_CPU: Duration = Duration::from_millis(50);

const LEFT_IDLE: u32 = 0xc0_30_30;
const LEFT_CLICKED: u32 = 0x30_c0_30;
const RIGHT_IDLE: u32 = 0x40_40_40;
const RIGHT_TYPED: u32 = 0x30_30_c0;
const RIGHT_PICKED: u32 = 0xc0_c0_30;
/// Typing this key makes the app ask for a file.
const PICK_FILE_KEY: char = 'o';
const PICKED_FILE: &str = "chosen.txt";

struct Halves {
    focus_handle: FocusHandle,
    left_clicked: bool,
    typed: bool,
    picked_file: Option<String>,
}

impl Halves {
    fn pick_file(&mut self, cx: &mut Context<Self>) {
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else {
                return;
            };
            let picked_file = paths
                .first()
                .and_then(|path| path.file_name())
                .map(|name| name.to_string_lossy().into_owned());
            let updated = this.update(cx, |this, cx| {
                this.picked_file = picked_file;
                cx.notify();
            });
            if let Err(error) = updated {
                eprintln!("the view was closed before the file was picked: {error}");
            }
        })
        .detach();
    }
}

impl Render for Halves {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let left = if self.left_clicked {
            LEFT_CLICKED
        } else {
            LEFT_IDLE
        };
        let right = match (&self.picked_file, self.typed) {
            (Some(_), _) => RIGHT_PICKED,
            (None, true) => RIGHT_TYPED,
            (None, false) => RIGHT_IDLE,
        };
        div()
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                this.typed = true;
                if event.keystroke.key == PICK_FILE_KEY.to_string() {
                    this.pick_file(cx);
                }
                cx.notify();
            }))
            .size_full()
            .flex()
            .child(
                div()
                    .id("left")
                    .w_1_2()
                    .h_full()
                    .bg(rgb(left))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.left_clicked = true;
                        cx.notify();
                    })),
            )
            .child(div().w_1_2().h_full().bg(rgb(right)))
    }
}

fn run_app() {
    gpui_tui::application().run(|cx: &mut App| {
        let opened = cx.open_window(WindowOptions::default(), |window, cx| {
            cx.new(|cx| {
                let focus_handle = cx.focus_handle();
                window.focus(&focus_handle, cx);
                Halves {
                    focus_handle,
                    left_clicked: false,
                    typed: false,
                    picked_file: None,
                }
            })
        });
        if opened.is_err() {
            cx.quit();
        }
    });
}

fn app_command(name: &str) -> Result<Command> {
    let log_path = std::env::temp_dir().join(format!("gpui-tui-e2e-{name}.log"));
    let mut command = Command::new(std::env::current_exe()?);
    command
        .env(CHILD_ENV, "1")
        .env("GPUI_TUI_LOG", &log_path)
        .env_remove("GPUI_TUI_SCALE")
        .env_remove("GPUI_TUI_TRANSPORT")
        .env_remove("GPUI_TUI_RENDER_SCALE")
        .env_remove("GPUI_TUI_MAX_FPS");
    Ok(command)
}

fn spawn_app(config: TerminalConfig, name: &str) -> Result<FakeTerminal> {
    FakeTerminal::spawn(app_command(name)?, config)
}

/// A folder removed on drop.
struct TempFolder(PathBuf);

impl TempFolder {
    fn new(name: &str, files: &[&str]) -> Result<Self> {
        let path = std::env::temp_dir().join(format!("{name}-{}", std::process::id()));
        std::fs::create_dir_all(&path)?;
        let folder = Self(path);
        for file in files {
            std::fs::write(folder.0.join(file), b"")?;
        }
        Ok(folder)
    }
}

impl Drop for TempFolder {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("failed to remove {}: {error}", self.0.display());
        }
    }
}

/// User plus system CPU time of a process, from `/proc/<pid>/stat`.
fn cpu_time(pid: u32) -> Result<Duration> {
    // Fields after the command name, which is parenthesized and may contain spaces.
    const UTIME_FIELD: usize = 11;
    const STIME_FIELD: usize = 12;
    // The kernel's USER_HZ, which is 100 on every mainstream Linux configuration.
    const CLOCK_TICKS_PER_SECOND: u64 = 100;

    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let after_command = stat.rsplit_once(')').context("malformed stat")?.1;
    let fields: Vec<&str> = after_command.split_whitespace().collect();
    let ticks = |index: usize| -> Result<u64> {
        Ok(fields.get(index).context("missing stat field")?.parse()?)
    };
    let total_ticks = ticks(UTIME_FIELD)? + ticks(STIME_FIELD)?;
    Ok(Duration::from_millis(
        total_ticks * 1000 / CLOCK_TICKS_PER_SECOND,
    ))
}

fn color_matches(actual: [u8; 4], expected: u32) -> bool {
    let Rgba { r, g, b, .. } = rgb(expected);
    let expected = [r, g, b].map(|channel| (channel * 255.0).round() as u8);
    actual[..3]
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(expected) <= COLOR_TOLERANCE)
}

fn pixel_matches(screen: Option<&Screen>, x: u32, y: u32, expected: u32) -> bool {
    screen.is_some_and(|screen| {
        x < screen.image.width()
            && y < screen.image.height()
            && color_matches(screen.pixel(x, y), expected)
    })
}

/// Renders, reacts to a click, to typing, and to a resize, then quits on Ctrl+C.
fn interaction_over_shared_memory() -> Result<()> {
    run_interaction(TerminalConfig::default(), "shared-memory")
}

/// Same, with frames sent inline as a remote terminal would need.
fn interaction_over_direct_transmission() -> Result<()> {
    let config = TerminalConfig {
        shared_memory: false,
        ..TerminalConfig::default()
    };
    run_interaction(config, "direct")
}

/// Inline frames on a high-density terminal are rendered at half resolution and scaled up by
/// the terminal; clicks must still land where they appear.
fn interaction_over_direct_transmission_at_half_resolution() -> Result<()> {
    let config = TerminalConfig {
        shared_memory: false,
        cell_width: 16,
        cell_height: 32,
        ..TerminalConfig::default()
    };
    run_interaction(config, "direct-half-resolution")
}

fn run_interaction(config: TerminalConfig, name: &str) -> Result<()> {
    let (columns, rows) = (u32::from(config.columns), u32::from(config.rows));
    let (cell_width, cell_height) = (u32::from(config.cell_width), u32::from(config.cell_height));
    let width = columns * cell_width;
    let height = rows * cell_height;
    let (left_x, right_x, middle_y) = (width / 4, width * 3 / 4, height / 2);
    let mut terminal = spawn_app(config, name)?;

    terminal.wait_until(TIMEOUT, "the first frame", |terminal| {
        pixel_matches(terminal.screen(), left_x, middle_y, LEFT_IDLE)
            && pixel_matches(terminal.screen(), right_x, middle_y, RIGHT_IDLE)
    })?;
    let screen = terminal.screen().context("no frame")?;
    ensure!(
        (screen.image.width(), screen.image.height()) == (width, height),
        "frame is {}x{}, expected {width}x{height}",
        screen.image.width(),
        screen.image.height()
    );
    ensure!(
        (screen.columns, screen.rows) == (columns, rows),
        "frame placed over the wrong cells"
    );

    // Idle: nothing changes, so no frames are sent and the app barely uses the CPU.
    terminal.pump_for(SETTLE_TIME)?;
    let frames_before = terminal.frame_count();
    let cpu_before = cpu_time(terminal.app_pid())?;
    terminal.pump_for(IDLE_TIME)?;
    let idle_cpu = cpu_time(terminal.app_pid())?.saturating_sub(cpu_before);
    ensure!(
        terminal.frame_count() == frames_before,
        "{} frames sent while idle",
        terminal.frame_count() - frames_before
    );
    ensure!(
        idle_cpu <= MAX_IDLE_CPU,
        "used {idle_cpu:?} of CPU while idle"
    );

    terminal.click(left_x, middle_y)?;
    terminal.wait_until(TIMEOUT, "the click to recolor the left half", |terminal| {
        pixel_matches(terminal.screen(), left_x, middle_y, LEFT_CLICKED)
    })?;

    terminal.type_char('x')?;
    terminal.wait_until(TIMEOUT, "typing to recolor the right half", |terminal| {
        pixel_matches(terminal.screen(), right_x, middle_y, RIGHT_TYPED)
    })?;

    let (resized_columns, resized_rows) = (100, 30);
    terminal.resize(resized_columns, resized_rows)?;
    let resized_width = u32::from(resized_columns) * cell_width;
    let resized_height = u32::from(resized_rows) * cell_height;
    terminal.wait_until(TIMEOUT, "a frame at the new size", |terminal| {
        terminal.screen().is_some_and(|screen| {
            screen.image.width() == resized_width && screen.image.height() == resized_height
        })
    })?;

    terminal.press_control('c')?;
    let status = terminal.wait_for_exit(TIMEOUT)?;
    ensure!(status.success(), "app exited with {status}");
    ensure!(terminal.is_restored(), "terminal was not restored on exit");
    ensure!(
        terminal.errors().is_empty(),
        "decode errors: {:?}",
        terminal.errors()
    );
    Ok(())
}

/// The app asks for a file: the picker replaces the app on screen, typing filters it, Enter
/// answers the app, and the app's window comes back.
fn file_picker_answers_the_app() -> Result<()> {
    let config = TerminalConfig::default();
    let (right_x, middle_y) = (
        u32::from(config.columns) * u32::from(config.cell_width) * 3 / 4,
        u32::from(config.rows) * u32::from(config.cell_height) / 2,
    );
    let folder = TempFolder::new("gpui-tui-e2e-picker", &[PICKED_FILE, "other.txt"])?;
    let mut command = app_command("picker")?;
    command.current_dir(&folder.0);
    let mut terminal = FakeTerminal::spawn(command, config)?;
    terminal.wait_until(TIMEOUT, "the first frame", |terminal| {
        pixel_matches(terminal.screen(), right_x, middle_y, RIGHT_IDLE)
    })?;

    terminal.type_char(PICK_FILE_KEY)?;
    terminal.wait_until(TIMEOUT, "the file picker", |terminal| {
        terminal.screen().is_none()
            && terminal.text().contains("Open a file")
            && terminal.text().contains("other.txt")
    })?;

    for character in "chos".chars() {
        terminal.type_char(character)?;
    }
    terminal.press_enter()?;
    terminal.wait_until(TIMEOUT, "the app to receive the picked file", |terminal| {
        pixel_matches(terminal.screen(), right_x, middle_y, RIGHT_PICKED)
    })?;

    terminal.press_control('c')?;
    let status = terminal.wait_for_exit(TIMEOUT)?;
    ensure!(status.success(), "app exited with {status}");
    ensure!(terminal.is_restored(), "terminal was not restored on exit");
    Ok(())
}

/// Without kitty graphics support the app exits with an error and restores the terminal.
fn unsupported_terminal_is_rejected() -> Result<()> {
    let config = TerminalConfig {
        kitty_graphics: false,
        ..TerminalConfig::default()
    };
    let mut terminal = spawn_app(config, "unsupported")?;
    let status = terminal.wait_for_exit(TIMEOUT)?;
    ensure!(!status.success(), "app should fail without kitty graphics");
    ensure!(terminal.frame_count() == 0, "no frame should be sent");
    Ok(())
}

type TestFn = fn() -> Result<()>;

fn main() -> ExitCode {
    if std::env::var_os(CHILD_ENV).is_some() {
        run_app();
        return ExitCode::SUCCESS;
    }

    let tests: [(&str, TestFn); 5] = [
        (
            "interaction_over_shared_memory",
            interaction_over_shared_memory,
        ),
        (
            "interaction_over_direct_transmission",
            interaction_over_direct_transmission,
        ),
        (
            "interaction_over_direct_transmission_at_half_resolution",
            interaction_over_direct_transmission_at_half_resolution,
        ),
        ("file_picker_answers_the_app", file_picker_answers_the_app),
        (
            "unsupported_terminal_is_rejected",
            unsupported_terminal_is_rejected,
        ),
    ];
    let filter = std::env::args().nth(1).filter(|arg| !arg.starts_with('-'));
    let mut failures = 0;
    for (name, test) in tests {
        if filter
            .as_ref()
            .is_some_and(|filter| !name.contains(filter.as_str()))
        {
            continue;
        }
        match test() {
            Ok(()) => println!("test {name} ... ok"),
            Err(error) => {
                failures += 1;
                println!("test {name} ... FAILED\n  {error:#}");
            }
        }
    }
    if failures == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

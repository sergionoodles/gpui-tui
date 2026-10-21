//! State shared by the platform and its windows: the terminal session, the window stack, and
//! frame pacing.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::time::{Duration, Instant};

use calloop::timer::{TimeoutAction, Timer};
use calloop::{LoopHandle, LoopSignal};
use gpui::{CursorStyle, Pixels, Point, Size, WindowAppearance, point, px, size};
use gpui_tui_term::input::{ColorScheme, MouseEvent, Parser, Reply, TermEvent};
use gpui_tui_term::{Frame, Geometry, Presenter, Session, sequences};

use crate::cursor::pointer_shape_name;
use crate::options::Options;
use crate::window::WindowShared;

pub(crate) use file_picker::{PickerReply, open_picker, save_picker};

/// How long to wait for the rest of an escape sequence before treating ESC as a key press.
const LONE_ESCAPE_TIMEOUT: Duration = Duration::from_millis(20);
/// Cells at least this tall are assumed to be on a high-density display.
const HIGH_DENSITY_CELL_HEIGHT: u16 = 24;

pub(crate) struct Runtime {
    session: RefCell<Session>,
    parser: RefCell<Parser>,
    presenter: RefCell<Presenter>,
    /// Open windows, bottom to top. Only the top one is shown and receives input.
    windows: RefCell<Vec<Rc<WindowShared>>>,
    /// Shown instead of the windows while the app waits for paths.
    file_picker: RefCell<Option<file_picker::ActivePicker>>,
    appearance: Cell<WindowAppearance>,
    cursor_style: Cell<Option<CursorStyle>>,
    options: Options,
    frame_interval: Duration,
    last_frame_at: Cell<Option<Instant>>,
    frame_timer_armed: Cell<bool>,
    loop_handle: LoopHandle<'static, ()>,
    loop_signal: LoopSignal,
    frame_ping: calloop::ping::Ping,
    this: Weak<Runtime>,
}

impl Runtime {
    pub(crate) fn new(
        session: Session,
        options: Options,
        loop_handle: LoopHandle<'static, ()>,
        loop_signal: LoopSignal,
        frame_ping: calloop::ping::Ping,
    ) -> Rc<Self> {
        let presenter = Presenter::new(session.transport());
        let appearance = match session.capabilities().color_scheme() {
            Some(ColorScheme::Light) => WindowAppearance::Light,
            Some(ColorScheme::Dark) | None => WindowAppearance::Dark,
        };
        let frame_interval = Duration::from_secs(1) / options.max_frames_per_second.max(1);

        Rc::new_cyclic(|this| Self {
            session: RefCell::new(session),
            parser: RefCell::new(Parser::new()),
            presenter: RefCell::new(presenter),
            windows: RefCell::new(Vec::new()),
            file_picker: RefCell::new(None),
            appearance: Cell::new(appearance),
            cursor_style: Cell::new(None),
            options,
            frame_interval,
            last_frame_at: Cell::new(None),
            frame_timer_armed: Cell::new(false),
            loop_handle,
            loop_signal,
            frame_ping,
            this: this.clone(),
        })
    }

    pub(crate) fn options(&self) -> &Options {
        &self.options
    }

    pub(crate) fn frame_interval(&self) -> Duration {
        self.frame_interval
    }

    pub(crate) fn geometry(&self) -> Geometry {
        self.session.borrow().geometry()
    }

    pub(crate) fn scale_factor(&self) -> f32 {
        let geometry = self.geometry();
        self.options
            .scale_factor
            .unwrap_or(if geometry.cell_height >= HIGH_DENSITY_CELL_HEIGHT {
                2.0
            } else {
                1.0
            })
    }

    /// The window size in GPUI logical pixels.
    pub(crate) fn content_size(&self) -> Size<Pixels> {
        let geometry = self.geometry();
        let scale = self.scale_factor();
        size(
            px(geometry.pixel_width() as f32 / scale),
            px(geometry.pixel_height() as f32 / scale),
        )
    }

    pub(crate) fn appearance(&self) -> WindowAppearance {
        self.appearance.get()
    }

    pub(crate) fn send(&self, bytes: Vec<u8>) {
        self.session.borrow().send(bytes);
    }

    /// Converts a mouse event position to window coordinates.
    pub(crate) fn mouse_position(&self, event: &MouseEvent) -> Point<Pixels> {
        let geometry = self.geometry();
        let (x, y) = if self.session.borrow().capabilities().sgr_pixels {
            (event.column as f32, event.row as f32)
        } else {
            geometry.cell_center(event.column, event.row)
        };
        let scale = self.scale_factor();
        point(px(x / scale), px(y / scale))
    }

    pub(crate) fn quit(&self) {
        self.loop_signal.stop();
        self.loop_signal.wakeup();
    }

    /// Flushes output and gives the terminal back to the user.
    pub(crate) fn finish(&self) {
        self.session.borrow_mut().finish();
    }

    pub(crate) fn take_pending_events(&self) -> Vec<TermEvent> {
        self.session.borrow_mut().take_pending_events()
    }

    pub(crate) fn input_fd(&self) -> std::io::Result<std::os::fd::OwnedFd> {
        self.session.borrow().input_fd().try_clone_to_owned()
    }

    pub(crate) fn has_in_band_resize(&self) -> bool {
        self.session.borrow().capabilities().in_band_resize
    }

    // Output

    pub(crate) fn set_cursor_style(&self, style: CursorStyle) {
        if self.cursor_style.replace(Some(style)) == Some(style) {
            return;
        }
        let mut out = Vec::new();
        sequences::pointer_shape(&mut out, pointer_shape_name(style));
        self.send(out);
    }

    pub(crate) fn set_title(&self, title: &str) {
        let mut out = Vec::new();
        sequences::title(&mut out, title);
        self.send(out);
    }

    pub(crate) fn copy_to_clipboard(&self, text: &str) {
        let mut out = Vec::new();
        sequences::copy_to_clipboard(&mut out, text);
        self.send(out);
    }

    /// Sends a rendered frame of the top window.
    pub(crate) fn present(&self, frame: Frame<'_>) {
        if self.is_showing_file_picker() {
            return;
        }
        let area = self.geometry().cell_area();
        let encoded = self
            .presenter
            .borrow_mut()
            .encode(frame, area, Instant::now());
        match encoded {
            Ok(Some(bytes)) => self.send(bytes),
            Ok(None) => {}
            Err(error) => log::error!("failed to encode frame: {error}"),
        }
    }

    // Frames

    /// Called by a window's frame waker: GPUI has something to draw.
    pub(crate) fn wake_frame_loop(&self) {
        self.frame_ping.ping();
    }

    /// Delivers a frame request to the top window when it wants one, the terminal has caught
    /// up, and the frame rate allows it. Otherwise arms a timer to retry.
    pub(crate) fn pump_frames(&self) {
        if self.is_showing_file_picker() {
            return;
        }
        let Some(window) = self.active_window() else {
            return;
        };
        if !window.wants_frame() {
            return;
        }
        let now = Instant::now();
        if !self.presenter.borrow_mut().is_ready(now) {
            // A fence reply calls back here; the timer covers lost replies.
            let expiry = self.presenter.borrow().next_fence_expiry();
            if let Some(expiry) = expiry {
                self.arm_frame_timer(expiry);
            }
            return;
        }
        if let Some(last_frame_at) = self.last_frame_at.get() {
            let next_frame_at = last_frame_at + self.frame_interval;
            if now < next_frame_at {
                self.arm_frame_timer(next_frame_at);
                return;
            }
        }
        self.last_frame_at.set(Some(now));
        window.request_frame();
    }

    fn arm_frame_timer(&self, deadline: Instant) {
        if self.frame_timer_armed.replace(true) {
            return;
        }
        let runtime = self.this.clone();
        let inserted =
            self.loop_handle
                .insert_source(Timer::from_deadline(deadline), move |_, _, _| {
                    if let Some(runtime) = runtime.upgrade() {
                        runtime.frame_timer_armed.set(false);
                        runtime.pump_frames();
                    }
                    TimeoutAction::Drop
                });
        if let Err(error) = inserted {
            self.frame_timer_armed.set(false);
            log::error!("failed to schedule a frame: {error}");
        }
    }

    // Input

    /// Reads and handles all available terminal input.
    pub(crate) fn handle_input_ready(&self) {
        let events = {
            let mut parser = self.parser.borrow_mut();
            self.session.borrow_mut().read_events(&mut parser)
        };
        match events {
            Ok(events) => self.handle_events(events),
            Err(error) => {
                log::error!("terminal input failed, quitting: {error}");
                self.quit();
            }
        }
        if self.parser.borrow().has_pending_escape() {
            self.arm_escape_timer();
        }
    }

    pub(crate) fn handle_events(&self, events: Vec<TermEvent>) {
        for event in events {
            self.handle_event(event);
        }
    }

    fn handle_event(&self, event: TermEvent) {
        match event {
            TermEvent::Reply(reply) => self.handle_reply(reply),
            TermEvent::Resize(grid) => {
                let changed = self.session.borrow_mut().apply_resize(grid);
                if changed {
                    self.handle_geometry_change();
                }
            }
            TermEvent::Key(_) | TermEvent::Mouse(_) | TermEvent::Paste(_)
                if self.is_showing_file_picker() =>
            {
                self.handle_file_picker_input(event);
            }
            input => {
                if let Some(window) = self.active_window() {
                    window.handle_terminal_input(input);
                }
            }
        }
    }

    fn handle_reply(&self, reply: Reply) {
        match reply {
            Reply::Graphics(graphics) => {
                let acknowledged = self.presenter.borrow_mut().handle_reply(&graphics);
                if acknowledged {
                    self.pump_frames();
                }
            }
            Reply::ColorScheme(scheme) => self.set_color_scheme(scheme),
            Reply::Background(rgb) => self.set_color_scheme(rgb.color_scheme()),
            _ => {}
        }
    }

    fn set_color_scheme(&self, scheme: ColorScheme) {
        let appearance = match scheme {
            ColorScheme::Dark => WindowAppearance::Dark,
            ColorScheme::Light => WindowAppearance::Light,
        };
        if self.appearance.replace(appearance) == appearance {
            return;
        }
        let windows = self.windows.borrow().clone();
        for window in windows {
            window.appearance_changed();
        }
    }

    /// Re-reads the terminal size; used when the terminal has no in-band resize reports.
    pub(crate) fn poll_geometry(&self) {
        let changed = self.session.borrow_mut().refresh_geometry();
        match changed {
            Ok(true) => self.handle_geometry_change(),
            Ok(false) => {}
            Err(error) => log::warn!("failed to read the terminal size: {error}"),
        }
    }

    fn handle_geometry_change(&self) {
        self.presenter.borrow_mut().invalidate();
        let content_size = self.content_size();
        let scale = self.scale_factor();
        let windows = self.windows.borrow().clone();
        for window in windows {
            window.resized(content_size, scale);
        }
        if self.is_showing_file_picker() {
            self.resize_file_picker();
        }
    }

    fn arm_escape_timer(&self) {
        let runtime = self.this.clone();
        let inserted = self.loop_handle.insert_source(
            Timer::from_duration(LONE_ESCAPE_TIMEOUT),
            move |_, _, _| {
                if let Some(runtime) = runtime.upgrade() {
                    let event = runtime.parser.borrow_mut().flush_pending_escape();
                    runtime.handle_events(event.into_iter().collect());
                }
                TimeoutAction::Drop
            },
        );
        if let Err(error) = inserted {
            log::error!("failed to schedule the escape key timeout: {error}");
        }
    }
}

mod file_picker;
mod windows;

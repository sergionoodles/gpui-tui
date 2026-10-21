//! A GPUI window drawn into the terminal.

use std::cell::{Cell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::Arc;
use std::time::Instant;

use gpui::{
    AnyWindowHandle, DevicePixels, DispatchEventResult, FrameRequestSource, Modifiers, Pixels,
    PlatformAtlas, PlatformHeadlessRenderer, PlatformInput, PlatformInputHandler,
    RequestFrameOptions, Scene, Size, WindowVisibility,
};
use gpui_tui_term::Frame;
use gpui_tui_term::input::TermEvent;
use gpui_wgpu::WgpuHeadlessRenderer;

use crate::input::InputTranslator;
use crate::runtime::Runtime;

/// Takes a callback out of its slot, calls it, and puts it back unless it was replaced
/// meanwhile. Callbacks can re-enter the window, so no borrow is held while they run.
macro_rules! invoke_callback {
    ($window:expr, $slot:ident, |$callback:ident| $call:expr) => {{
        let taken = $window.callbacks.borrow_mut().$slot.take();
        taken.map(|mut $callback| {
            let result = $call;
            let mut callbacks = $window.callbacks.borrow_mut();
            if callbacks.$slot.is_none() {
                callbacks.$slot = Some($callback);
            }
            result
        })
    }};
}

pub(crate) struct TuiWindow(Rc<WindowShared>);

impl TuiWindow {
    pub(crate) fn new(
        handle: AnyWindowHandle,
        runtime: &Rc<Runtime>,
        title: Option<&str>,
    ) -> anyhow::Result<Self> {
        let renderer = WgpuHeadlessRenderer::new()?;
        let sprite_atlas = renderer.sprite_atlas();
        let shared = Rc::new(WindowShared {
            handle,
            runtime: Rc::downgrade(runtime),
            renderer: RefCell::new(renderer),
            sprite_atlas,
            state: RefCell::new(WindowState::default()),
            callbacks: RefCell::new(WindowCallbacks::default()),
            input: RefCell::new(InputTranslator::default()),
            frame_requested: Cell::new(true),
            needs_presentation: Cell::new(false),
        });
        runtime.push_window(shared.clone());
        if let Some(title) = title {
            shared.set_title(title);
        }
        Ok(Self(shared))
    }
}

impl Drop for TuiWindow {
    fn drop(&mut self) {
        if let Some(runtime) = self.0.runtime.upgrade() {
            runtime.remove_window(&self.0);
        }
    }
}

pub(crate) struct WindowShared {
    handle: AnyWindowHandle,
    runtime: Weak<Runtime>,
    renderer: RefCell<WgpuHeadlessRenderer>,
    sprite_atlas: Arc<dyn PlatformAtlas>,
    state: RefCell<WindowState>,
    callbacks: RefCell<WindowCallbacks>,
    input: RefCell<InputTranslator>,
    /// GPUI asked for a frame (its waker fired) and has not received one yet.
    frame_requested: Cell<bool>,
    /// The next frame must be presented even if nothing changed, e.g. after being uncovered.
    needs_presentation: Cell<bool>,
}

struct WindowState {
    title: String,
    input_handler: Option<PlatformInputHandler>,
    /// Whether the terminal has keyboard focus.
    is_focused: bool,
    is_hovered: bool,
}

impl Default for WindowState {
    fn default() -> Self {
        Self {
            title: String::new(),
            input_handler: None,
            is_focused: true,
            is_hovered: false,
        }
    }
}

type ResizeCallback = Box<dyn FnMut(Size<Pixels>, f32)>;

#[derive(Default)]
struct WindowCallbacks {
    request_frame: Option<Box<dyn FnMut(RequestFrameOptions)>>,
    input: Option<Box<dyn FnMut(PlatformInput) -> DispatchEventResult>>,
    active_status_change: Option<Box<dyn FnMut(bool)>>,
    visibility_change: Option<Box<dyn FnMut(WindowVisibility)>>,
    hover_status_change: Option<Box<dyn FnMut(bool)>>,
    resize: Option<ResizeCallback>,
    appearance_changed: Option<Box<dyn FnMut()>>,
}

impl WindowShared {
    pub(crate) fn handle(&self) -> AnyWindowHandle {
        self.handle
    }

    pub(crate) fn wants_frame(&self) -> bool {
        self.frame_requested.get()
    }

    pub(crate) fn request_frame(&self) {
        self.frame_requested.set(false);
        let options = RequestFrameOptions {
            require_presentation: self.needs_presentation.take(),
            signal_at: Some(Instant::now()),
            signal_source: FrameRequestSource::LocalSchedule,
            ..RequestFrameOptions::default()
        };
        invoke_callback!(self, request_frame, |callback| callback(options));
    }

    fn mark_frame_requested(&self) {
        self.frame_requested.set(true);
        if let Some(runtime) = self.runtime.upgrade() {
            runtime.wake_frame_loop();
        }
    }

    /// Draws and shows a frame even if nothing changed, e.g. after something covered the window.
    pub(crate) fn redraw(&self) {
        self.needs_presentation.set(true);
        self.mark_frame_requested();
    }

    /// This window is now the top of the stack and must be redrawn.
    pub(crate) fn became_visible(&self) {
        self.redraw();
        let title = self.state.borrow().title.clone();
        if let Some(runtime) = self.runtime.upgrade()
            && !title.is_empty()
        {
            runtime.set_title(&title);
        }
        invoke_callback!(self, visibility_change, |callback| {
            callback(WindowVisibility::Visible)
        });
    }

    pub(crate) fn resized(&self, size: Size<Pixels>, scale_factor: f32) {
        invoke_callback!(self, resize, |callback| callback(size, scale_factor));
        self.mark_frame_requested();
    }

    pub(crate) fn appearance_changed(&self) {
        invoke_callback!(self, appearance_changed, |callback| callback());
    }

    fn set_title(&self, title: &str) {
        title.clone_into(&mut self.state.borrow_mut().title);
        if let Some(runtime) = self.runtime.upgrade()
            && runtime.is_active_window(self)
        {
            runtime.set_title(title);
        }
    }

    pub(crate) fn handle_terminal_input(&self, event: TermEvent) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        match event {
            TermEvent::Key(key) => {
                let inputs = self.input.borrow_mut().key(&key);
                self.dispatch_all(inputs, &runtime);
            }
            TermEvent::Mouse(mouse) => {
                self.set_hovered();
                let position = runtime.mouse_position(&mouse);
                let inputs = self
                    .input
                    .borrow_mut()
                    .mouse(&mouse, position, Instant::now());
                self.dispatch_all(inputs, &runtime);
            }
            TermEvent::Paste(text) => self.insert_text(&normalize_newlines(&text)),
            TermEvent::Focus(focused) => {
                self.state.borrow_mut().is_focused = focused;
                invoke_callback!(self, active_status_change, |callback| callback(focused));
            }
            TermEvent::Resize(_) | TermEvent::Reply(_) => {}
        }
    }

    fn set_hovered(&self) {
        if std::mem::replace(&mut self.state.borrow_mut().is_hovered, true) {
            return;
        }
        invoke_callback!(self, hover_status_change, |callback| callback(true));
    }

    fn dispatch_all(&self, inputs: Vec<PlatformInput>, runtime: &Runtime) {
        for input in inputs {
            self.dispatch(input, runtime);
        }
    }

    /// Sends input to GPUI. Unhandled key presses become typed text, like on GPUI's Linux
    /// backends; an unhandled Ctrl+C quits.
    fn dispatch(&self, input: PlatformInput, runtime: &Runtime) {
        let result = invoke_callback!(self, input, |callback| callback(input.clone()));
        let propagate = result.is_none_or(|result| result.propagate);
        if !propagate {
            return;
        }
        let PlatformInput::KeyDown(event) = input else {
            return;
        };
        let keystroke = &event.keystroke;
        if runtime.options().quit_on_unhandled_ctrl_c
            && keystroke.key == "c"
            && keystroke.modifiers == Modifiers::control()
        {
            runtime.quit();
            return;
        }
        if keystroke.modifiers.is_subset_of(&Modifiers::shift())
            && let Some(text) = &keystroke.key_char
        {
            self.insert_text(text);
        }
    }

    fn insert_text(&self, text: &str) {
        let handler = self.state.borrow_mut().input_handler.take();
        let Some(mut handler) = handler else {
            return;
        };
        handler.replace_text_in_range(None, text);
        let mut state = self.state.borrow_mut();
        if state.input_handler.is_none() {
            state.input_handler = Some(handler);
        }
    }

    fn render(&self, scene: &Scene) {
        let Some(runtime) = self.runtime.upgrade() else {
            return;
        };
        if !runtime.is_active_window(self) {
            return;
        }
        let geometry = runtime.geometry();
        let device_size = Size {
            width: DevicePixels(i32::try_from(geometry.pixel_width()).unwrap_or(i32::MAX)),
            height: DevicePixels(i32::try_from(geometry.pixel_height()).unwrap_or(i32::MAX)),
        };
        let image = self
            .renderer
            .borrow_mut()
            .render_scene_to_image(scene, device_size);
        match image {
            Ok(image) => runtime.present(Frame {
                pixels: image.as_raw(),
                width: image.width(),
                height: image.height(),
            }),
            Err(error) => log::error!("failed to render frame: {error:#}"),
        }
    }
}

/// Terminals send line breaks in pastes as `\r`.
fn normalize_newlines(text: &str) -> String {
    text.replace("\r\n", "\n").replace('\r', "\n")
}

mod platform_window;

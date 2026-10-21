//! [`PlatformWindow`] for [`TuiWindow`]: what GPUI asks of and registers with a window.

use std::rc::Rc;
use std::sync::Arc;

use gpui::{
    Bounds, Capslock, DispatchEventResult, GpuSpecs, Modifiers, Pixels, PlatformAtlas,
    PlatformDisplay, PlatformInput, PlatformInputHandler, PlatformWindow, Point, PromptButton,
    PromptLevel, RequestFrameOptions, Scene, Size, WindowAppearance, WindowBackgroundAppearance,
    WindowBounds, WindowControlArea, WindowControls, WindowVisibility,
};

use super::TuiWindow;
use crate::display::TuiDisplay;

impl raw_window_handle::HasWindowHandle for TuiWindow {
    fn window_handle(
        &self,
    ) -> Result<raw_window_handle::WindowHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl raw_window_handle::HasDisplayHandle for TuiWindow {
    fn display_handle(
        &self,
    ) -> Result<raw_window_handle::DisplayHandle<'_>, raw_window_handle::HandleError> {
        Err(raw_window_handle::HandleError::NotSupported)
    }
}

impl PlatformWindow for TuiWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds::new(Point::default(), self.content_size())
    }

    fn is_maximized(&self) -> bool {
        false
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.0
            .runtime
            .upgrade()
            .map(|runtime| runtime.content_size())
            .unwrap_or_default()
    }

    /// The terminal decides the size.
    fn resize(&mut self, _size: Size<Pixels>) {}

    fn scale_factor(&self) -> f32 {
        self.0
            .runtime
            .upgrade()
            .map_or(1.0, |runtime| runtime.scale_factor())
    }

    fn appearance(&self) -> WindowAppearance {
        self.0
            .runtime
            .upgrade()
            .map_or(WindowAppearance::Dark, |runtime| runtime.appearance())
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        let runtime = self.0.runtime.upgrade()?;
        Some(Rc::new(TuiDisplay::new(
            runtime.content_size(),
            runtime.frame_interval(),
        )))
    }

    fn mouse_position(&self) -> Point<Pixels> {
        self.0.input.borrow().mouse_position()
    }

    fn modifiers(&self) -> Modifiers {
        self.0.input.borrow().modifiers()
    }

    fn capslock(&self) -> Capslock {
        Capslock {
            on: self.0.input.borrow().caps_lock(),
        }
    }

    fn set_input_handler(&mut self, input_handler: PlatformInputHandler) {
        self.0.state.borrow_mut().input_handler = Some(input_handler);
    }

    fn take_input_handler(&mut self) -> Option<PlatformInputHandler> {
        self.0.state.borrow_mut().input_handler.take()
    }

    /// GPUI then renders its own in-window prompt.
    fn prompt(
        &self,
        _level: PromptLevel,
        _msg: &str,
        _detail: Option<&str>,
        _answers: &[PromptButton],
    ) -> Option<futures::channel::oneshot::Receiver<usize>> {
        None
    }

    fn activate(&self) {
        if let Some(runtime) = self.0.runtime.upgrade() {
            runtime.activate_window(&self.0);
        }
    }

    fn is_active(&self) -> bool {
        self.0.state.borrow().is_focused
            && self
                .0
                .runtime
                .upgrade()
                .is_some_and(|runtime| runtime.is_active_window(&self.0))
    }

    fn visibility(&self) -> WindowVisibility {
        WindowVisibility::Visible
    }

    fn is_hovered(&self) -> bool {
        self.0.state.borrow().is_hovered
    }

    fn background_appearance(&self) -> WindowBackgroundAppearance {
        WindowBackgroundAppearance::Opaque
    }

    fn set_title(&mut self, title: &str) {
        self.0.set_title(title);
    }

    fn get_title(&self) -> String {
        self.0.state.borrow().title.clone()
    }

    fn set_background_appearance(&self, _background_appearance: WindowBackgroundAppearance) {}

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        false
    }

    fn frame_waker(&self) -> Option<Rc<dyn Fn()>> {
        // Weak: the waker lives in GPUI's window, which this platform window belongs to.
        let window = Rc::downgrade(&self.0);
        Some(Rc::new(move || {
            if let Some(window) = window.upgrade() {
                window.mark_frame_requested();
            }
        }))
    }

    fn schedule_frame(&self) {
        self.0.mark_frame_requested();
    }

    fn on_request_frame(&self, callback: Box<dyn FnMut(RequestFrameOptions)>) {
        self.0.callbacks.borrow_mut().request_frame = Some(callback);
    }

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput) -> DispatchEventResult>) {
        self.0.callbacks.borrow_mut().input = Some(callback);
    }

    fn on_active_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().active_status_change = Some(callback);
    }

    fn on_visibility_change(&self, callback: Box<dyn FnMut(WindowVisibility)>) {
        self.0.callbacks.borrow_mut().visibility_change = Some(callback);
    }

    fn on_hover_status_change(&self, callback: Box<dyn FnMut(bool)>) {
        self.0.callbacks.borrow_mut().hover_status_change = Some(callback);
    }

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.0.callbacks.borrow_mut().resize = Some(callback);
    }

    /// Terminal windows never move.
    fn on_moved(&self, _callback: Box<dyn FnMut()>) {}

    fn on_display_changed(&self, _callback: Box<dyn FnMut()>) {}

    /// The user cannot close a terminal window except by quitting.
    fn on_should_close(&self, _callback: Box<dyn FnMut() -> bool>) {}

    fn on_hit_test_window_control(&self, _callback: Box<dyn FnMut() -> Option<WindowControlArea>>) {
    }

    fn on_close(&self, _callback: Box<dyn FnOnce()>) {}

    fn on_appearance_changed(&self, callback: Box<dyn FnMut()>) {
        self.0.callbacks.borrow_mut().appearance_changed = Some(callback);
    }

    fn draw(&self, scene: &Scene) {
        self.0.render(scene);
    }

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.0.sprite_atlas.clone()
    }

    fn is_subpixel_rendering_supported(&self) -> bool {
        false
    }

    /// No minimize, maximize, fullscreen, or window menu buttons make sense in a terminal.
    fn window_controls(&self) -> WindowControls {
        WindowControls {
            fullscreen: false,
            maximize: false,
            minimize: false,
            window_menu: false,
        }
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }

    fn update_ime_position(&self, _bounds: Bounds<Pixels>) {}
}

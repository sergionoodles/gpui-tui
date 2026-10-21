//! [`Platform`] for [`TuiPlatform`].

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;

use anyhow::{Context as _, Result, anyhow};
use futures::channel::oneshot;
use gpui::{
    Action, ActivityGuard, AnyWindowHandle, BackgroundExecutor, ClipboardItem, CursorStyle,
    DummyKeyboardMapper, ForegroundExecutor, Keymap, Menu, MenuItem, PathPromptOptions, Platform,
    PlatformDisplay, PlatformKeyboardLayout, PlatformKeyboardMapper, PlatformTextSystem,
    PlatformWindow, Task, ThermalState, WindowAppearance, WindowParams,
};

use super::{TuiKeyboardLayout, TuiPlatform};
use crate::runtime::{self, PickerReply};
use crate::window::TuiWindow;

impl Platform for TuiPlatform {
    fn background_executor(&self) -> BackgroundExecutor {
        self.background_executor.clone()
    }

    fn foreground_executor(&self) -> ForegroundExecutor {
        self.foreground_executor.clone()
    }

    fn text_system(&self) -> Arc<dyn PlatformTextSystem> {
        self.text_system.clone()
    }

    fn run(&self, on_finish_launching: Box<dyn 'static + FnOnce()>) {
        let Some(startup) = self.startup.borrow_mut().take() else {
            log::error!("the terminal platform is already running");
            return;
        };
        let mut event_loop = match self.prepare_event_loop(startup) {
            Ok(event_loop) => event_loop,
            Err(error) => {
                self.runtime.finish();
                eprintln!("gpui-tui: {error:#}");
                return;
            }
        };

        self.runtime
            .handle_events(self.runtime.take_pending_events());
        on_finish_launching();

        if let Err(error) = event_loop.run(None, &mut (), |_| {}) {
            log::error!("the event loop failed: {error}");
        }

        let quit_callback = self.state.borrow_mut().quit_callback.take();
        if let Some(mut quit_callback) = quit_callback {
            quit_callback();
        }
        self.runtime.finish();
    }

    fn quit(&self) {
        self.runtime.quit();
    }

    fn restart(&self, _binary_path: Option<PathBuf>, _arguments: Vec<OsString>) {
        log::warn!("restarting is not supported in the terminal");
    }

    fn activate(&self, _ignoring_other_apps: bool) {}

    fn hide(&self) {}

    fn hide_other_apps(&self) {}

    fn unhide_other_apps(&self) {}

    fn displays(&self) -> Vec<Rc<dyn PlatformDisplay>> {
        vec![self.display()]
    }

    fn primary_display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        Some(self.display())
    }

    fn on_displays_changed(&self, _callback: Box<dyn FnMut()>) {}

    fn active_window(&self) -> Option<AnyWindowHandle> {
        self.runtime.active_window().map(|window| window.handle())
    }

    fn window_stack(&self) -> Option<Vec<AnyWindowHandle>> {
        let mut handles = self.runtime.window_handles();
        handles.reverse();
        Some(handles)
    }

    fn open_window(
        &self,
        handle: AnyWindowHandle,
        options: WindowParams,
    ) -> Result<Box<dyn PlatformWindow>> {
        let title = options
            .titlebar
            .as_ref()
            .and_then(|titlebar| titlebar.title.as_ref())
            .map(|title| title.to_string());
        let window = TuiWindow::new(handle, &self.runtime, title.as_deref())
            .context("failed to create a terminal window")?;
        Ok(Box::new(window))
    }

    fn window_appearance(&self) -> WindowAppearance {
        self.runtime.appearance()
    }

    fn open_url(&self, url: &str) {
        self.open_with_desktop(url.as_ref());
    }

    fn on_open_urls(&self, _callback: Box<dyn FnMut(Vec<String>)>) {}

    fn register_url_scheme(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "URL schemes cannot be registered from the terminal"
        )))
    }

    fn prompt_for_paths(
        &self,
        options: PathPromptOptions,
    ) -> oneshot::Receiver<Result<Option<Vec<PathBuf>>>> {
        let (sender, receiver) = oneshot::channel();
        let picker = runtime::open_picker(&options);
        self.runtime
            .open_file_picker(picker, PickerReply::Paths(sender));
        receiver
    }

    fn prompt_for_new_path(
        &self,
        directory: &Path,
        suggested_name: Option<&str>,
    ) -> oneshot::Receiver<Result<Option<PathBuf>>> {
        let (sender, receiver) = oneshot::channel();
        let picker = runtime::save_picker(directory, suggested_name);
        self.runtime
            .open_file_picker(picker, PickerReply::NewPath(sender));
        receiver
    }

    fn can_select_mixed_files_and_dirs(&self) -> bool {
        true
    }

    fn reveal_path(&self, path: &Path) {
        let directory = if path.is_dir() {
            path
        } else {
            path.parent().unwrap_or(path)
        };
        self.open_with_desktop(directory.as_os_str());
    }

    fn open_with_system(&self, path: &Path) {
        self.open_with_desktop(path.as_os_str());
    }

    fn on_quit(&self, callback: Box<dyn FnMut() -> bool>) {
        self.state.borrow_mut().quit_callback = Some(callback);
    }

    fn on_reopen(&self, _callback: Box<dyn FnMut()>) {}

    fn on_system_sleep(&self, _callback: Box<dyn FnMut()>) {}

    fn on_system_wake(&self, _callback: Box<dyn FnMut()>) {}

    fn set_menus(&self, _menus: Vec<Menu>, _keymap: &Keymap) {}

    fn set_dock_menu(&self, _menu: Vec<MenuItem>, _keymap: &Keymap) {}

    fn on_app_menu_action(&self, _callback: Box<dyn FnMut(&dyn Action)>) {}

    fn on_will_open_app_menu(&self, _callback: Box<dyn FnMut()>) {}

    fn on_validate_app_menu_command(&self, _callback: Box<dyn FnMut(&dyn Action) -> bool>) {}

    fn thermal_state(&self) -> ThermalState {
        ThermalState::Nominal
    }

    fn on_thermal_state_change(&self, _callback: Box<dyn FnMut()>) {}

    fn prevent_idle_sleep(&self, _reason: &str) -> Task<Result<ActivityGuard>> {
        Task::ready(Ok(ActivityGuard::noop()))
    }

    fn app_path(&self) -> Result<PathBuf> {
        Ok(std::env::current_exe()?)
    }

    fn path_for_auxiliary_executable(&self, name: &str) -> Result<PathBuf> {
        Err(anyhow!("auxiliary executable {name:?} is not available"))
    }

    fn set_cursor_style(&self, style: CursorStyle) {
        self.runtime.set_cursor_style(style);
    }

    fn hide_cursor_until_mouse_moves(&self) {}

    fn is_cursor_visible(&self) -> bool {
        true
    }

    fn should_auto_hide_scrollbars(&self) -> bool {
        false
    }

    /// Terminals rarely allow reading the clipboard, so this returns what the app last copied.
    /// Pasting goes through the terminal's own paste instead.
    fn read_from_clipboard(&self) -> Option<ClipboardItem> {
        self.state.borrow().clipboard.clone()
    }

    fn write_to_clipboard(&self, item: ClipboardItem) {
        if let Some(text) = item.text() {
            self.runtime.copy_to_clipboard(&text);
        }
        self.state.borrow_mut().clipboard = Some(item);
    }

    fn read_from_primary(&self) -> Option<ClipboardItem> {
        self.state.borrow().primary.clone()
    }

    fn write_to_primary(&self, item: ClipboardItem) {
        self.state.borrow_mut().primary = Some(item);
    }

    fn write_credentials(&self, _url: &str, _username: &str, _password: &[u8]) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "credentials are not available in the terminal"
        )))
    }

    fn read_credentials(&self, _url: &str) -> Task<Result<Option<(String, Vec<u8>)>>> {
        Task::ready(Err(anyhow!(
            "credentials are not available in the terminal"
        )))
    }

    fn delete_credentials(&self, _url: &str) -> Task<Result<()>> {
        Task::ready(Err(anyhow!(
            "credentials are not available in the terminal"
        )))
    }

    fn keyboard_layout(&self) -> Box<dyn PlatformKeyboardLayout> {
        Box::new(TuiKeyboardLayout)
    }

    fn keyboard_mapper(&self) -> Rc<dyn PlatformKeyboardMapper> {
        Rc::new(DummyKeyboardMapper)
    }

    fn on_keyboard_layout_change(&self, _callback: Box<dyn FnMut()>) {}
}

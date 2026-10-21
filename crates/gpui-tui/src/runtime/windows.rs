//! The window stack: only the top window is shown and receives input.

use std::rc::Rc;

use super::Runtime;
use crate::window::WindowShared;

impl Runtime {
    pub(crate) fn push_window(&self, window: Rc<WindowShared>) {
        self.windows.borrow_mut().push(window);
        self.presenter.borrow_mut().invalidate();
    }

    pub(crate) fn remove_window(&self, window: &WindowShared) {
        let was_top = self.is_active_window(window);
        self.windows
            .borrow_mut()
            .retain(|open| !std::ptr::eq(open.as_ref(), window));
        if self.windows.borrow().is_empty() {
            self.quit();
            return;
        }
        if was_top && let Some(top) = self.active_window() {
            self.presenter.borrow_mut().invalidate();
            top.became_visible();
        }
    }

    /// Moves a window to the top of the stack.
    pub(crate) fn activate_window(&self, window: &WindowShared) {
        let mut windows = self.windows.borrow_mut();
        let Some(index) = windows
            .iter()
            .position(|open| std::ptr::eq(open.as_ref(), window))
        else {
            return;
        };
        if index + 1 == windows.len() {
            return;
        }
        let window = windows.remove(index);
        windows.push(window.clone());
        drop(windows);
        self.presenter.borrow_mut().invalidate();
        window.became_visible();
    }

    pub(crate) fn active_window(&self) -> Option<Rc<WindowShared>> {
        self.windows.borrow().last().cloned()
    }

    pub(crate) fn window_handles(&self) -> Vec<gpui::AnyWindowHandle> {
        self.windows
            .borrow()
            .iter()
            .map(|window| window.handle())
            .collect()
    }

    pub(crate) fn is_active_window(&self, window: &WindowShared) -> bool {
        self.windows
            .borrow()
            .last()
            .is_some_and(|top| std::ptr::eq(top.as_ref(), window))
    }
}

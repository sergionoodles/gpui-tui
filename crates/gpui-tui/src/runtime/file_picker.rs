//! The text file picker, shown in place of the app while it asks for paths.

use std::path::{Path, PathBuf};

use anyhow::{Result, anyhow};
use futures::channel::oneshot;
use gpui::{CursorStyle, PathPromptOptions};
use gpui_tui_term::input::{MouseButton, MouseEvent, MouseEventKind, ScrollDirection, TermEvent};
use gpui_tui_term::picker::{self, PathPicker, PickerMode, PickerStatus};
use gpui_tui_term::{graphics, sequences};

use super::Runtime;

/// Entries moved per mouse wheel notch.
const SCROLL_ENTRIES: isize = 3;
const CLEAR_SCREEN: &[u8] = b"\x1b[0m\x1b[2J";

/// Where the picked paths go: the app awaits the receiving end.
pub(crate) enum PickerReply {
    Paths(oneshot::Sender<Result<Option<Vec<PathBuf>>>>),
    NewPath(oneshot::Sender<Result<Option<PathBuf>>>),
}

impl PickerReply {
    /// `None` means the user cancelled.
    fn send(self, result: Result<Option<Vec<PathBuf>>>) {
        let delivered = match self {
            Self::Paths(sender) => sender.send(result).is_ok(),
            Self::NewPath(sender) => {
                let path = result.map(|paths| paths.and_then(|paths| paths.into_iter().next()));
                sender.send(path).is_ok()
            }
        };
        if !delivered {
            log::debug!("the app stopped waiting for the file picker");
        }
    }
}

pub(super) struct ActivePicker {
    picker: PathPicker,
    reply: PickerReply,
}

/// A picker for `cx.prompt_for_paths`, starting in the working directory.
pub(crate) fn open_picker(options: &PathPromptOptions) -> PathPicker {
    let mode = PickerMode::Open {
        files: options.files,
        directories: options.directories,
        multiple: options.multiple,
    };
    let title = match &options.prompt {
        Some(prompt) => prompt.to_string(),
        None => default_open_title(options).to_owned(),
    };
    PathPicker::new(mode, title, &start_directory())
}

/// A picker for `cx.prompt_for_new_path`.
pub(crate) fn save_picker(directory: &Path, suggested_name: Option<&str>) -> PathPicker {
    let mode = PickerMode::Save {
        suggested_name: suggested_name.unwrap_or_default().to_owned(),
    };
    PathPicker::new(mode, "Save as".to_owned(), directory)
}

fn default_open_title(options: &PathPromptOptions) -> &'static str {
    match (options.files, options.directories, options.multiple) {
        (true, true, _) => "Open",
        (false, true, false) => "Choose a folder",
        (false, true, true) => "Choose folders",
        (_, false, true) => "Open files",
        (_, false, false) => "Open a file",
    }
}

/// The working directory, or the home folder if it is gone.
fn start_directory() -> PathBuf {
    std::env::current_dir()
        .ok()
        .or_else(std::env::home_dir)
        .unwrap_or_else(|| PathBuf::from("/"))
}

impl Runtime {
    /// Hides the app and shows `picker` until a path is picked or it is cancelled.
    pub(crate) fn open_file_picker(&self, mut picker: PathPicker, reply: PickerReply) {
        if self.is_showing_file_picker() {
            reply.send(Err(anyhow!("a file picker is already open")));
            return;
        }
        picker.set_screen_rows(self.geometry().rows);
        let mut out = Vec::new();
        graphics::delete_all(&mut out);
        out.extend_from_slice(CLEAR_SCREEN);
        self.send(out);
        self.presenter.borrow_mut().invalidate();
        // The app's pointer shape (a text beam, a hand) means nothing over the picker.
        self.set_cursor_style(CursorStyle::Arrow);
        self.file_picker
            .replace(Some(ActivePicker { picker, reply }));
        self.draw_file_picker();
    }

    pub(super) fn is_showing_file_picker(&self) -> bool {
        self.file_picker.borrow().is_some()
    }

    pub(super) fn handle_file_picker_input(&self, event: TermEvent) {
        let status = {
            let mut active = self.file_picker.borrow_mut();
            let Some(active) = active.as_mut() else {
                return;
            };
            match event {
                TermEvent::Key(key) => active.picker.handle_key(&key),
                TermEvent::Paste(text) => active.picker.insert_text(&text),
                TermEvent::Mouse(mouse) => self.handle_file_picker_mouse(&mut active.picker, mouse),
                _ => {}
            }
            active.picker.status().clone()
        };
        match status {
            PickerStatus::Active => self.draw_file_picker(),
            PickerStatus::Cancelled => self.close_file_picker(None),
            PickerStatus::Chosen(paths) => self.close_file_picker(Some(paths)),
        }
    }

    /// The wheel moves the selection; a click selects, and a click on the selection opens it.
    fn handle_file_picker_mouse(&self, picker: &mut PathPicker, mouse: MouseEvent) {
        match mouse.kind {
            MouseEventKind::Scroll(ScrollDirection::Up) => picker.move_selection(-SCROLL_ENTRIES),
            MouseEventKind::Scroll(ScrollDirection::Down) => picker.move_selection(SCROLL_ENTRIES),
            MouseEventKind::Press(MouseButton::Left) => {
                let Some(index) = picker::entry_at_row(picker, self.mouse_row(&mouse)) else {
                    return;
                };
                if index == picker.selected() {
                    picker.activate_selection();
                } else {
                    picker.select(index);
                }
            }
            _ => {}
        }
    }

    /// The 0-based cell row of a mouse event, which may be reported in pixels.
    fn mouse_row(&self, mouse: &MouseEvent) -> u16 {
        let row = if self.session.borrow().capabilities().sgr_pixels {
            mouse.row / u32::from(self.geometry().cell_height.max(1))
        } else {
            mouse.row
        };
        u16::try_from(row).unwrap_or(u16::MAX)
    }

    pub(super) fn resize_file_picker(&self) {
        let rows = self.geometry().rows;
        if let Some(active) = self.file_picker.borrow_mut().as_mut() {
            active.picker.set_screen_rows(rows);
        }
        self.draw_file_picker();
    }

    fn draw_file_picker(&self) {
        let columns = self.geometry().columns;
        let out = match self.file_picker.borrow().as_ref() {
            Some(active) => picker::render(&active.picker, columns),
            None => return,
        };
        self.send(out);
    }

    /// Answers the app and brings its window back.
    fn close_file_picker(&self, paths: Option<Vec<PathBuf>>) {
        let Some(active) = self.file_picker.take() else {
            return;
        };
        let mut out = Vec::new();
        sequences::autowrap(&mut out, true);
        out.extend_from_slice(CLEAR_SCREEN);
        self.send(out);
        active.reply.send(Ok(paths));
        if let Some(window) = self.active_window() {
            window.redraw();
        }
    }
}

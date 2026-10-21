//! A keyboard-driven file picker drawn with terminal text. Terminals have no file dialogs, so
//! this stands in for them when an app asks for paths.

mod listing;
mod view;

#[cfg(test)]
mod tests;

use std::path::{Path, PathBuf};

use listing::{Entry, EntryKind};

use crate::input::{Key, KeyEvent, KeyEventKind};

pub use view::{entry_at_row, render};

/// Screen height assumed until [`PathPicker::set_screen_rows`] is called.
const DEFAULT_SCREEN_ROWS: u16 = 24;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerMode {
    /// Pick existing paths.
    Open {
        files: bool,
        directories: bool,
        multiple: bool,
    },
    /// Pick a path to save to, which may not exist yet.
    Save { suggested_name: String },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PickerStatus {
    Active,
    Cancelled,
    Chosen(Vec<PathBuf>),
}

pub struct PathPicker {
    mode: PickerMode,
    title: String,
    directory: PathBuf,
    /// Everything in `directory`, including the `..` and `.` entries.
    entries: Vec<Entry>,
    /// Filters the list when opening; the file name when saving.
    query: String,
    /// Index into [`PathPicker::visible_entries`].
    selected: usize,
    /// Index of the first visible entry shown at the top of the list.
    scroll: usize,
    screen_rows: u16,
    marked: Vec<PathBuf>,
    show_hidden: bool,
    message: Option<String>,
    /// A path that already exists; pressing Enter again replaces it.
    pending_overwrite: Option<PathBuf>,
    status: PickerStatus,
}

impl PathPicker {
    /// Starts in `directory`, or the closest ancestor that can be read.
    pub fn new(mode: PickerMode, title: String, directory: &Path) -> Self {
        let query = match &mode {
            PickerMode::Save { suggested_name } => suggested_name.clone(),
            PickerMode::Open { .. } => String::new(),
        };
        let mut picker = Self {
            mode,
            title,
            directory: directory.to_path_buf(),
            entries: Vec::new(),
            query,
            selected: 0,
            scroll: 0,
            screen_rows: DEFAULT_SCREEN_ROWS,
            marked: Vec::new(),
            show_hidden: false,
            message: None,
            pending_overwrite: None,
            status: PickerStatus::Active,
        };
        for ancestor in directory.ancestors() {
            if picker.navigate(ancestor).is_ok() {
                return picker;
            }
        }
        picker.message = Some(format!("Cannot read {}", directory.display()));
        picker
    }

    pub fn status(&self) -> &PickerStatus {
        &self.status
    }

    /// Index of the selected entry among the visible ones.
    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn set_screen_rows(&mut self, rows: u16) {
        self.screen_rows = rows;
        self.keep_selection_visible();
    }

    pub fn handle_key(&mut self, key: &KeyEvent) {
        if key.kind == KeyEventKind::Release || matches!(key.key, Key::Modifier(_)) {
            return;
        }
        let pending_overwrite = self.pending_overwrite.take();
        self.message = None;
        let modifiers = key.modifiers;
        let control = modifiers.control && !modifiers.alt && !modifiers.super_key;
        match key.key {
            Key::Escape => self.status = PickerStatus::Cancelled,
            Key::Char('c' | 'g') if control => self.status = PickerStatus::Cancelled,
            Key::Char('h') if control => self.toggle_hidden(),
            Key::Char('u') if control => self.set_query(String::new()),
            Key::Char('p') if control => self.move_selection(-1),
            Key::Char('n') if control => self.move_selection(1),
            Key::Up => self.move_selection(-1),
            Key::Down => self.move_selection(1),
            Key::PageUp => self.move_selection(-self.page_step()),
            Key::PageDown => self.move_selection(self.page_step()),
            Key::Home => self.selected = 0,
            Key::End => self.selected = self.visible_entries().len().saturating_sub(1),
            Key::Left => self.open_parent(),
            Key::Right => self.open_selected_directory(),
            Key::Tab if self.is_multiple() => self.toggle_mark(),
            Key::Tab => self.open_selected_directory(),
            Key::Backspace if self.query.is_empty() => self.open_parent(),
            Key::Backspace => {
                let mut query = self.query.clone();
                query.pop();
                self.set_query(query);
            }
            Key::Enter => self.activate(pending_overwrite),
            _ if !control && !modifiers.alt && !modifiers.super_key => {
                if let Some(text) = &key.text {
                    self.insert_text(text);
                }
            }
            _ => {}
        }
        self.keep_selection_visible();
    }

    /// Typed or pasted text.
    pub fn insert_text(&mut self, text: &str) {
        let printable = text.chars().filter(|character| !character.is_control());
        let query = self.query.chars().chain(printable).collect();
        self.set_query(query);
        self.keep_selection_visible();
    }

    pub fn move_selection(&mut self, delta: isize) {
        let last = self.visible_entries().len().saturating_sub(1);
        let selected = self.selected.saturating_add_signed(delta);
        self.selected = selected.min(last);
        self.keep_selection_visible();
    }

    pub fn select(&mut self, index: usize) {
        self.message = None;
        self.pending_overwrite = None;
        if index < self.visible_entries().len() {
            self.selected = index;
        }
        self.keep_selection_visible();
    }

    /// What Enter does: open the selected folder, or pick the selection.
    pub fn activate_selection(&mut self) {
        self.activate(None);
        self.keep_selection_visible();
    }

    /// Scrolls the list just enough to show the selected entry.
    fn keep_selection_visible(&mut self) {
        let list_rows = view::list_rows(self.screen_rows);
        if self.selected < self.scroll {
            self.scroll = self.selected;
        }
        if self.selected >= self.scroll + list_rows {
            self.scroll = self.selected + 1 - list_rows;
        }
    }

    fn page_step(&self) -> isize {
        let list_rows = view::list_rows(self.screen_rows);
        isize::try_from(list_rows.saturating_sub(1).max(1)).unwrap_or(isize::MAX)
    }

    fn activate(&mut self, pending_overwrite: Option<PathBuf>) {
        let query = self.query.trim().to_owned();
        if is_typed_path(&query) {
            self.activate_typed_path(&query, pending_overwrite);
            return;
        }
        if matches!(self.mode, PickerMode::Save { .. }) && !query.is_empty() {
            let path = self.directory.join(&query);
            self.save_or_open(path, pending_overwrite);
            return;
        }
        if self.is_multiple() && !self.marked.is_empty() {
            self.status = PickerStatus::Chosen(self.marked.clone());
            return;
        }
        let Some(entry) = self.selected_entry().cloned() else {
            self.message = Some("Nothing matches".to_owned());
            return;
        };
        let path = self.directory.join(&entry.name);
        match entry.kind {
            EntryKind::Parent => self.open_parent(),
            EntryKind::Current => self.status = PickerStatus::Chosen(vec![self.directory.clone()]),
            EntryKind::Directory => self.open_directory(&path),
            EntryKind::File => match self.mode {
                PickerMode::Save { .. } => self.query = entry.display_name(),
                PickerMode::Open { .. } => self.status = PickerStatus::Chosen(vec![path]),
            },
        }
    }

    /// A path typed into the query box: a folder is opened, anything else is picked.
    fn activate_typed_path(&mut self, query: &str, pending_overwrite: Option<PathBuf>) {
        let path = self.resolve(query);
        if matches!(self.mode, PickerMode::Save { .. }) {
            self.save_or_open(path, pending_overwrite);
            return;
        }
        if path.is_dir() {
            self.open_directory(&path);
            return;
        }
        if path.is_file() && self.allows(EntryKind::File) {
            self.status = PickerStatus::Chosen(vec![path]);
            return;
        }
        self.message = Some(format!("{} was not found", path.display()));
    }

    /// Saving to a folder opens it; an existing file needs a second Enter to be replaced.
    fn save_or_open(&mut self, path: PathBuf, pending_overwrite: Option<PathBuf>) {
        if path.is_dir() {
            self.open_directory(&path);
            return;
        }
        let parent_exists = path.parent().is_some_and(Path::is_dir);
        if !parent_exists {
            self.message = Some(format!("The folder for {} does not exist", path.display()));
            return;
        }
        if path.exists() && pending_overwrite.as_ref() != Some(&path) {
            self.message = Some(format!(
                "{} already exists. Press Enter again to replace it.",
                path.display()
            ));
            self.pending_overwrite = Some(path);
            return;
        }
        self.status = PickerStatus::Chosen(vec![path]);
    }

    fn resolve(&self, typed: &str) -> PathBuf {
        let home = std::env::home_dir();
        let expanded = match (typed.strip_prefix('~'), home) {
            (Some(rest), Some(home)) => home.join(rest.trim_start_matches('/')),
            _ => PathBuf::from(typed),
        };
        self.directory.join(expanded)
    }

    fn open_selected_directory(&mut self) {
        let Some(entry) = self.selected_entry().cloned() else {
            return;
        };
        match entry.kind {
            EntryKind::Parent => self.open_parent(),
            EntryKind::Directory => self.open_directory(&self.directory.join(&entry.name)),
            EntryKind::Current | EntryKind::File => {}
        }
    }

    fn open_directory(&mut self, path: &Path) {
        // Resolves `..` in typed paths. If this fails, so does `navigate`, which reports it.
        let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
        if let Err(error) = self.navigate(&path) {
            self.message = Some(format!("Cannot open {}: {error}", path.display()));
        }
    }

    /// Goes up a folder and selects the one we came from.
    fn open_parent(&mut self) {
        let Some(parent) = self.directory.parent().map(Path::to_path_buf) else {
            return;
        };
        let child = self.directory.file_name().map(ToOwned::to_owned);
        if let Err(error) = self.navigate(&parent) {
            self.message = Some(format!("Cannot open {}: {error}", parent.display()));
            return;
        }
        let position = self
            .visible_entries()
            .iter()
            .position(|entry| Some(&entry.name) == child.as_ref());
        self.selected = position.unwrap_or(0);
    }

    fn navigate(&mut self, directory: &Path) -> std::io::Result<()> {
        let listed = listing::read_directory(directory, self.show_hidden)?;
        let mut entries = Vec::with_capacity(listed.len() + 2);
        if directory.parent().is_some() {
            entries.push(special_entry("..", EntryKind::Parent));
        }
        if self.allows(EntryKind::Current) {
            entries.push(special_entry(".", EntryKind::Current));
        }
        entries.extend(listed);
        self.directory = directory.to_path_buf();
        self.entries = entries;
        self.selected = 0;
        self.scroll = 0;
        if matches!(self.mode, PickerMode::Open { .. }) {
            self.query.clear();
        }
        Ok(())
    }

    fn toggle_hidden(&mut self) {
        self.show_hidden = !self.show_hidden;
        let directory = self.directory.clone();
        if let Err(error) = self.navigate(&directory) {
            self.message = Some(format!("Cannot open {}: {error}", directory.display()));
        }
    }

    fn toggle_mark(&mut self) {
        let Some(entry) = self.selected_entry() else {
            return;
        };
        if !matches!(entry.kind, EntryKind::Directory | EntryKind::File) || !self.allows(entry.kind)
        {
            return;
        }
        let path = self.directory.join(&entry.name);
        match self.marked.iter().position(|marked| *marked == path) {
            Some(index) => {
                self.marked.remove(index);
            }
            None => self.marked.push(path),
        }
        self.move_selection(1);
    }

    fn set_query(&mut self, query: String) {
        self.query = query;
        if matches!(self.mode, PickerMode::Open { .. }) {
            self.selected = 0;
        }
    }

    fn is_multiple(&self) -> bool {
        matches!(self.mode, PickerMode::Open { multiple: true, .. })
    }

    /// Whether entries of this kind can be picked (folders can always be browsed).
    fn allows(&self, kind: EntryKind) -> bool {
        match (&self.mode, kind) {
            (PickerMode::Open { files, .. }, EntryKind::File) => *files,
            (PickerMode::Open { directories, .. }, EntryKind::Directory | EntryKind::Current) => {
                *directories
            }
            (PickerMode::Save { .. }, EntryKind::Current) => false,
            _ => true,
        }
    }

    fn is_marked(&self, entry: &Entry) -> bool {
        let path = self.directory.join(&entry.name);
        self.marked.contains(&path)
    }

    fn selected_entry(&self) -> Option<&Entry> {
        self.visible_entries().get(self.selected).copied()
    }

    /// The entries shown: files are hidden when only folders can be picked, and when opening,
    /// the query keeps the entries whose name contains it.
    fn visible_entries(&self) -> Vec<&Entry> {
        let filter = match self.mode {
            PickerMode::Open { .. } if !is_typed_path(&self.query) => self.query.to_lowercase(),
            _ => String::new(),
        };
        self.entries
            .iter()
            .filter(|entry| entry.kind != EntryKind::File || self.allows(EntryKind::File))
            .filter(|entry| {
                if filter.is_empty() {
                    return true;
                }
                let is_special = matches!(entry.kind, EntryKind::Parent | EntryKind::Current);
                !is_special && entry.display_name().to_lowercase().contains(&filter)
            })
            .collect()
    }
}

fn special_entry(name: &str, kind: EntryKind) -> Entry {
    Entry {
        name: name.into(),
        kind,
    }
}

/// Text that names a path rather than filtering the list: `/tmp`, `~/notes`, `src/main.rs`.
fn is_typed_path(query: &str) -> bool {
    query.contains('/') || query.starts_with('~')
}

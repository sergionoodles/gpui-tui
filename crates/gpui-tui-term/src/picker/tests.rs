use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use super::view::lines;
use super::*;
use crate::input::Modifiers;

static NEXT_TREE: AtomicUsize = AtomicUsize::new(0);

/// A folder of empty files under the system temp folder, removed on drop. Names ending in `/`
/// are folders.
struct TempTree(PathBuf);

impl TempTree {
    fn new(paths: &[&str]) -> Self {
        let id = NEXT_TREE.fetch_add(1, Ordering::Relaxed);
        let name = format!("gpui-tui-picker-{}-{id}", std::process::id());
        let root = std::env::temp_dir().join(name);
        std::fs::create_dir_all(&root).expect("create temp tree");
        let root = std::fs::canonicalize(root).expect("canonicalize temp tree");
        for path in paths {
            let full = root.join(path);
            if path.ends_with('/') {
                std::fs::create_dir_all(full).expect("create folder");
                continue;
            }
            if let Some(parent) = full.parent() {
                std::fs::create_dir_all(parent).expect("create parent folder");
            }
            std::fs::write(full, b"").expect("create file");
        }
        Self(root)
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.0.join(relative)
    }
}

impl Drop for TempTree {
    fn drop(&mut self) {
        if let Err(error) = std::fs::remove_dir_all(&self.0) {
            eprintln!("failed to remove {}: {error}", self.0.display());
        }
    }
}

fn open_files(directory: &Path) -> PathPicker {
    let mode = PickerMode::Open {
        files: true,
        directories: false,
        multiple: false,
    };
    PathPicker::new(mode, "Open".to_owned(), directory)
}

fn save_as(directory: &Path, name: &str) -> PathPicker {
    let mode = PickerMode::Save {
        suggested_name: name.to_owned(),
    };
    PathPicker::new(mode, "Save".to_owned(), directory)
}

fn press(picker: &mut PathPicker, key: Key) {
    picker.handle_key(&KeyEvent::press(key, Modifiers::default()));
}

fn press_control(picker: &mut PathPicker, character: char) {
    let control = Modifiers {
        control: true,
        ..Modifiers::default()
    };
    picker.handle_key(&KeyEvent::press(Key::Char(character), control));
}

fn type_text(picker: &mut PathPicker, text: &str) {
    for character in text.chars() {
        picker.handle_key(&KeyEvent {
            text: Some(character.to_string()),
            ..KeyEvent::press(Key::Char(character), Modifiers::default())
        });
    }
}

fn entry_names(picker: &PathPicker) -> Vec<String> {
    picker
        .visible_entries()
        .iter()
        .map(|entry| entry.display_name())
        .collect()
}

fn chosen(paths: &[PathBuf]) -> PickerStatus {
    PickerStatus::Chosen(paths.to_vec())
}

#[test]
fn lists_folders_first_and_hides_dot_files_until_asked() {
    let tree = TempTree::new(&["b.txt", "A.txt", "zeta/", "Alpha/", ".hidden"]);
    let mut picker = open_files(&tree.0);
    assert_eq!(
        entry_names(&picker),
        ["..", "Alpha", "zeta", "A.txt", "b.txt"]
    );

    press_control(&mut picker, 'h');
    assert!(entry_names(&picker).contains(&".hidden".to_owned()));
}

#[test]
fn typing_filters_and_enter_picks_the_file() {
    let tree = TempTree::new(&["notes.md", "data.sqlite", "more-data.csv"]);
    let mut picker = open_files(&tree.0);

    type_text(&mut picker, "DATA");
    assert_eq!(entry_names(&picker), ["data.sqlite", "more-data.csv"]);
    press(&mut picker, Key::Down);
    press(&mut picker, Key::Enter);

    assert_eq!(picker.status(), &chosen(&[tree.path("more-data.csv")]));
}

#[test]
fn enter_opens_folders_and_backspace_returns_to_the_parent() {
    let tree = TempTree::new(&["first/", "second/inner.txt"]);
    let mut picker = open_files(&tree.0);

    type_text(&mut picker, "sec");
    press(&mut picker, Key::Enter);
    assert_eq!(picker.directory, tree.path("second"));
    assert_eq!(entry_names(&picker), ["..", "inner.txt"]);

    press(&mut picker, Key::Backspace);
    assert_eq!(picker.directory, tree.0);
    assert_eq!(
        picker.selected_entry().map(Entry::display_name),
        Some("second".to_owned())
    );
    assert_eq!(picker.status(), &PickerStatus::Active);
}

#[test]
fn choosing_a_folder_uses_the_current_folder_entry() {
    let tree = TempTree::new(&["project/", "file.txt"]);
    let mode = PickerMode::Open {
        files: false,
        directories: true,
        multiple: false,
    };
    let mut picker = PathPicker::new(mode, "Choose".to_owned(), &tree.0);
    assert_eq!(entry_names(&picker), ["..", ".", "project"]);

    press(&mut picker, Key::End);
    press(&mut picker, Key::Right);
    press(&mut picker, Key::Down);
    press(&mut picker, Key::Enter);

    assert_eq!(picker.status(), &chosen(&[tree.path("project")]));
}

#[test]
fn tab_marks_several_files_when_multiple_are_allowed() {
    let tree = TempTree::new(&["a.txt", "b.txt", "c.txt"]);
    let mode = PickerMode::Open {
        files: true,
        directories: false,
        multiple: true,
    };
    let mut picker = PathPicker::new(mode, "Open".to_owned(), &tree.0);

    press(&mut picker, Key::Down);
    press(&mut picker, Key::Tab);
    press(&mut picker, Key::Down);
    press(&mut picker, Key::Tab);
    press(&mut picker, Key::Enter);

    assert_eq!(
        picker.status(),
        &chosen(&[tree.path("a.txt"), tree.path("c.txt")])
    );
}

#[test]
fn saving_uses_the_suggested_name_in_the_current_folder() {
    let tree = TempTree::new(&["exports/"]);
    let mut picker = save_as(&tree.0, "report.csv");
    assert_eq!(entry_names(&picker), ["..", "exports"]);

    press(&mut picker, Key::Down);
    press(&mut picker, Key::Right);
    press(&mut picker, Key::Enter);

    assert_eq!(picker.status(), &chosen(&[tree.path("exports/report.csv")]));
}

#[test]
fn replacing_an_existing_file_needs_a_second_enter() {
    let tree = TempTree::new(&["report.csv"]);
    let mut picker = save_as(&tree.0, "report.csv");

    press(&mut picker, Key::Enter);
    assert_eq!(picker.status(), &PickerStatus::Active);
    assert!(
        picker
            .message
            .as_deref()
            .is_some_and(|m| m.contains("already exists"))
    );

    // Any other key withdraws the confirmation.
    press(&mut picker, Key::Down);
    press(&mut picker, Key::Enter);
    assert_eq!(picker.status(), &PickerStatus::Active);

    press(&mut picker, Key::Enter);
    assert_eq!(picker.status(), &chosen(&[tree.path("report.csv")]));
}

#[test]
fn saving_into_a_missing_folder_is_refused() {
    let tree = TempTree::new(&[]);
    let mut picker = save_as(&tree.0, "");

    type_text(&mut picker, "missing/report.csv");
    press(&mut picker, Key::Enter);

    assert_eq!(picker.status(), &PickerStatus::Active);
    assert!(picker.message.is_some());
}

#[test]
fn a_typed_folder_path_is_opened() {
    let tree = TempTree::new(&["deep/er/file.txt"]);
    let mut picker = open_files(&tree.0);

    type_text(&mut picker, &format!("{}/", tree.path("deep/er").display()));
    press(&mut picker, Key::Enter);

    assert_eq!(picker.directory, tree.path("deep/er"));
    assert_eq!(picker.query, "");
}

#[test]
fn escape_and_control_c_cancel() {
    let tree = TempTree::new(&[]);
    let mut picker = open_files(&tree.0);
    press(&mut picker, Key::Escape);
    assert_eq!(picker.status(), &PickerStatus::Cancelled);

    let mut picker = open_files(&tree.0);
    press_control(&mut picker, 'c');
    assert_eq!(picker.status(), &PickerStatus::Cancelled);
}

#[test]
fn a_missing_start_folder_falls_back_to_an_existing_ancestor() {
    let tree = TempTree::new(&["here.txt"]);
    let picker = open_files(&tree.path("not/created/yet"));
    assert_eq!(picker.directory, tree.0);
}

#[test]
fn control_characters_in_file_names_never_reach_the_terminal() {
    let tree = TempTree::new(&["evil\x1b]2;pwned\x07.txt"]);
    let picker = open_files(&tree.0);

    let output = render(&picker, 80);

    assert!(!output.windows(3).any(|bytes| bytes == b"\x1b]2"));
    assert!(String::from_utf8_lossy(&output).contains("evil\u{fffd}]2;pwned\u{fffd}.txt"));
}

#[test]
fn the_list_scrolls_to_keep_the_selection_visible() {
    let names: Vec<String> = (0..20).map(|index| format!("file{index:02}.txt")).collect();
    let paths: Vec<&str> = names.iter().map(String::as_str).collect();
    let tree = TempTree::new(&paths);
    let mut picker = open_files(&tree.0);
    // Ten rows leave five for the list.
    picker.set_screen_rows(10);

    for _ in 0..7 {
        press(&mut picker, Key::Down);
    }

    let lines = lines(&picker, 40);
    assert_eq!(lines.len(), 10);
    let selected: Vec<String> = lines
        .iter()
        .filter(|line| line.is_selected)
        .map(|line| line.text().trim().to_owned())
        .collect();
    assert_eq!(selected, ["file06.txt"]);
    assert_eq!(entry_at_row(&picker, 8), Some(7));
    assert_eq!(entry_at_row(&picker, 9), None);
}

#[test]
fn long_names_are_cut_to_the_screen_width() {
    let tree = TempTree::new(&["a-file-name-that-is-far-too-long-for-the-screen.txt"]);
    let picker = open_files(&tree.0);

    let lines = lines(&picker, 20);

    let entry = lines[5].text();
    assert_eq!(entry.chars().count(), 20);
    assert!(entry.ends_with('…'));
}

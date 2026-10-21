//! Reading a folder's contents for the picker.

use std::cmp::Ordering;
use std::ffi::OsString;
use std::io;
use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Entry {
    pub(crate) name: OsString,
    pub(crate) kind: EntryKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EntryKind {
    /// `..`: the parent folder.
    Parent,
    /// `.`: picks the folder being shown.
    Current,
    Directory,
    File,
}

impl Entry {
    pub(crate) fn display_name(&self) -> String {
        self.name.to_string_lossy().into_owned()
    }
}

/// Lists the folders and files in `directory`, folders first, each sorted by name ignoring
/// case. Hidden (dot) entries are left out unless `show_hidden` is set.
pub(crate) fn read_directory(directory: &Path, show_hidden: bool) -> io::Result<Vec<Entry>> {
    let mut entries = Vec::new();
    for item in std::fs::read_dir(directory)? {
        let item = item?;
        let name = item.file_name();
        let is_hidden = name.as_encoded_bytes().starts_with(b".");
        if is_hidden && !show_hidden {
            continue;
        }
        let kind = if is_directory(&item.path()) {
            EntryKind::Directory
        } else {
            EntryKind::File
        };
        entries.push(Entry { name, kind });
    }
    entries.sort_by(compare_entries);
    Ok(entries)
}

/// Follows symbolic links, so a link to a folder can be opened. A broken link is listed as a
/// file; picking it reports the problem to the app, which is where it can be handled.
fn is_directory(path: &Path) -> bool {
    std::fs::metadata(path).is_ok_and(|metadata| metadata.is_dir())
}

fn compare_entries(left: &Entry, right: &Entry) -> Ordering {
    let is_file = |entry: &Entry| entry.kind == EntryKind::File;
    is_file(left).cmp(&is_file(right)).then_with(|| {
        let left_name = left.display_name().to_lowercase();
        let right_name = right.display_name().to_lowercase();
        left_name.cmp(&right_name)
    })
}

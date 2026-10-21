//! Drawing the picker as full-screen terminal text.

use super::listing::{Entry, EntryKind};
use super::{PathPicker, PickerMode};
use crate::sequences;

/// Title, folder, query, and separator rows above the list.
const HEADER_ROWS: u16 = 4;
/// The hint or message row below the list.
const FOOTER_ROWS: u16 = 1;
const ELLIPSIS: char = '…';
/// Shown instead of control characters in file names, which would otherwise reach the terminal
/// as escape sequences.
const REPLACEMENT: char = '\u{fffd}';
const SEPARATOR: &str = "─";
const MARK: &str = "● ";
const NO_MARK: &str = "  ";
const CURSOR: &str = " ";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Style {
    Plain,
    Title,
    Muted,
    Directory,
    Special,
    Message,
    Cursor,
}

impl Style {
    fn sgr(self) -> &'static str {
        match self {
            Self::Plain => "0",
            Self::Title => "1",
            Self::Muted => "2",
            Self::Directory => "1;34",
            Self::Special => "36",
            Self::Message => "33",
            Self::Cursor => "7",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Span {
    pub(crate) text: String,
    pub(crate) style: Style,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct Line {
    pub(crate) spans: Vec<Span>,
    /// Drawn in reverse video across the whole width.
    pub(crate) is_selected: bool,
}

impl Line {
    fn new(style: Style, text: String) -> Self {
        Self {
            spans: vec![Span { text, style }],
            is_selected: false,
        }
    }

    #[cfg(test)]
    pub(crate) fn text(&self) -> String {
        self.spans.iter().map(|span| span.text.as_str()).collect()
    }
}

/// Entries that fit on a screen this tall.
pub(crate) fn list_rows(screen_rows: u16) -> usize {
    usize::from(screen_rows.saturating_sub(HEADER_ROWS + FOOTER_ROWS)).max(1)
}

/// The bytes that draw the picker over the whole screen.
pub fn render(picker: &PathPicker, columns: u16) -> Vec<u8> {
    let mut out = sequences::SYNCHRONIZED_UPDATE_BEGIN.to_vec();
    // Long lines are clipped at the right edge instead of wrapping.
    sequences::autowrap(&mut out, false);
    for (row, line) in lines(picker, columns).iter().enumerate() {
        out.extend_from_slice(format!("\x1b[{};1H", row + 1).as_bytes());
        for span in &line.spans {
            let reverse = if line.is_selected { "7;" } else { "" };
            out.extend_from_slice(format!("\x1b[0;{reverse}{}m", span.style.sgr()).as_bytes());
            out.extend_from_slice(span.text.as_bytes());
        }
        out.extend_from_slice(b"\x1b[0m\x1b[K");
    }
    out.extend_from_slice(sequences::SYNCHRONIZED_UPDATE_END);
    out
}

/// The visible entry at a 0-based screen row, as an index for [`PathPicker::select`].
pub fn entry_at_row(picker: &PathPicker, row: u16) -> Option<usize> {
    let list_row = usize::from(row.checked_sub(HEADER_ROWS)?);
    if list_row >= list_rows(picker.screen_rows) {
        return None;
    }
    let index = picker.scroll + list_row;
    (index < picker.visible_entries().len()).then_some(index)
}

pub(crate) fn lines(picker: &PathPicker, columns: u16) -> Vec<Line> {
    let width = usize::from(columns);
    let screen_rows = usize::from(picker.screen_rows);
    let directory = picker.directory.display().to_string();
    let mut lines = vec![
        Line::new(Style::Title, fit(&picker.title, width)),
        Line::new(Style::Muted, fit_end(&directory, width)),
        query_line(picker, width),
        Line::new(Style::Muted, SEPARATOR.repeat(width)),
    ];

    let list_rows = list_rows(picker.screen_rows);
    let entries = picker.visible_entries();
    let shown = entries
        .iter()
        .enumerate()
        .skip(picker.scroll)
        .take(list_rows);
    for (index, entry) in shown {
        lines.push(entry_line(picker, entry, index == picker.selected, width));
    }

    let footer_row = screen_rows.saturating_sub(usize::from(FOOTER_ROWS));
    lines.resize(footer_row.max(lines.len()), Line::default());
    lines.push(footer_line(picker, width));
    lines.truncate(screen_rows);
    lines
}

fn query_line(picker: &PathPicker, width: usize) -> Line {
    let label = match picker.mode {
        PickerMode::Open { .. } => "Filter: ",
        PickerMode::Save { .. } => "Name: ",
    };
    let room = width.saturating_sub(label.chars().count() + CURSOR.len());
    let spans = vec![
        Span {
            text: label.to_owned(),
            style: Style::Muted,
        },
        Span {
            text: fit_end(&picker.query, room),
            style: Style::Plain,
        },
        Span {
            text: CURSOR.to_owned(),
            style: Style::Cursor,
        },
    ];
    Line {
        spans,
        is_selected: false,
    }
}

fn entry_line(picker: &PathPicker, entry: &Entry, is_selected: bool, width: usize) -> Line {
    let mark = if picker.is_marked(entry) {
        MARK
    } else {
        NO_MARK
    };
    let (name, style) = match entry.kind {
        EntryKind::Parent => ("../".to_owned(), Style::Special),
        EntryKind::Current => ("./  (choose this folder)".to_owned(), Style::Special),
        EntryKind::Directory => (format!("{}/", entry.display_name()), Style::Directory),
        EntryKind::File => (entry.display_name(), Style::Plain),
    };
    let text = fit(&format!("{mark}{name}"), width);
    // Pad so the reverse-video highlight spans the whole row.
    let padding = if is_selected {
        width.saturating_sub(text.chars().count())
    } else {
        0
    };
    Line {
        spans: vec![Span {
            text: format!("{text}{}", " ".repeat(padding)),
            style,
        }],
        is_selected,
    }
}

fn footer_line(picker: &PathPicker, width: usize) -> Line {
    if let Some(message) = &picker.message {
        return Line::new(Style::Message, fit(message, width));
    }
    let hints = match picker.mode {
        PickerMode::Open { multiple: true, .. } => {
            "Enter open · Tab mark · → into folder · ← up · Ctrl+H hidden files · Esc cancel"
        }
        PickerMode::Open { .. } => {
            "Enter open · → into folder · ← up · Ctrl+H hidden files · Esc cancel"
        }
        PickerMode::Save { .. } => {
            "Enter save · → into folder · ← up · Ctrl+H hidden files · Esc cancel"
        }
    };
    Line::new(Style::Muted, fit(hints, width))
}

/// Cuts `text` to `width` characters, ending with an ellipsis when cut.
fn fit(text: &str, width: usize) -> String {
    let text = sanitize(text);
    if width == 0 {
        return String::new();
    }
    if text.chars().count() <= width {
        return text;
    }
    let kept: String = text.chars().take(width.saturating_sub(1)).collect();
    format!("{kept}{ELLIPSIS}")
}

/// Like [`fit`], but keeps the end, which matters most for paths and typed text.
fn fit_end(text: &str, width: usize) -> String {
    let text = sanitize(text);
    if width == 0 {
        return String::new();
    }
    let count = text.chars().count();
    if count <= width {
        return text;
    }
    let kept: String = text.chars().skip(count + 1 - width).collect();
    format!("{ELLIPSIS}{kept}")
}

fn sanitize(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                REPLACEMENT
            } else {
                character
            }
        })
        .collect()
}

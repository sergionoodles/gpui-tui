//! Output control sequences: terminal modes, title, pointer shape, clipboard.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD as BASE64;

use crate::capabilities::{
    Capabilities, MODE_COLOR_SCHEME_UPDATES, MODE_IN_BAND_RESIZE, MODE_SGR_PIXELS,
};
use crate::graphics;

pub const SYNCHRONIZED_UPDATE_BEGIN: &[u8] = b"\x1b[?2026h";
pub const SYNCHRONIZED_UPDATE_END: &[u8] = b"\x1b[?2026l";
pub const CURSOR_HOME: &[u8] = b"\x1b[H";
/// String terminator. Sent first when restoring, to close any half-written escape sequence.
pub const STRING_TERMINATOR: &[u8] = b"\x1b\\";

/// Kitty keyboard flags: disambiguate (1), report event types (2), report alternate keys (4),
/// report all keys as escape codes (8), report associated text (16).
const KITTY_KEYBOARD_FLAGS: u8 = 31;

const MODE_ALTERNATE_SCREEN: u16 = 1049;
const MODE_CURSOR_VISIBLE: u16 = 25;
const MODE_ANY_MOTION_MOUSE: u16 = 1003;
const MODE_SGR_MOUSE: u16 = 1006;
const MODE_FOCUS_EVENTS: u16 = 1004;
const MODE_BRACKETED_PASTE: u16 = 2004;
const MODE_AUTOWRAP: u16 = 7;

/// Modes we enable, in order; disabled in reverse order on exit.
fn enabled_modes(capabilities: &Capabilities) -> Vec<u16> {
    let mut modes = vec![
        MODE_ALTERNATE_SCREEN,
        MODE_ANY_MOTION_MOUSE,
        MODE_SGR_MOUSE,
        MODE_FOCUS_EVENTS,
        MODE_BRACKETED_PASTE,
    ];
    let optional = [
        (capabilities.sgr_pixels, MODE_SGR_PIXELS),
        (capabilities.in_band_resize, MODE_IN_BAND_RESIZE),
        (capabilities.color_scheme_updates, MODE_COLOR_SCHEME_UPDATES),
    ];
    modes.extend(
        optional
            .into_iter()
            .filter_map(|(supported, mode)| supported.then_some(mode)),
    );
    modes
}

/// Switches the terminal into the state gpui-tui renders in.
pub fn enter(capabilities: &Capabilities) -> Vec<u8> {
    let mut out = Vec::new();
    // Save the window title so it can be restored.
    out.extend_from_slice(b"\x1b[22;0t");
    for mode in enabled_modes(capabilities) {
        set_mode(&mut out, mode, true);
    }
    set_mode(&mut out, MODE_CURSOR_VISIBLE, false);
    if capabilities.kitty_keyboard {
        out.extend_from_slice(format!("\x1b[>{KITTY_KEYBOARD_FLAGS}u").as_bytes());
    }
    out.extend_from_slice(b"\x1b[2J");
    out
}

/// Undoes [`enter`] and removes our images.
pub fn exit(capabilities: &Capabilities) -> Vec<u8> {
    let mut out = STRING_TERMINATOR.to_vec();
    if capabilities.kitty_keyboard {
        out.extend_from_slice(b"\x1b[<u");
    }
    graphics::delete_all(&mut out);
    pointer_shape(&mut out, "default");
    for mode in enabled_modes(capabilities).into_iter().rev() {
        set_mode(&mut out, mode, false);
    }
    set_mode(&mut out, MODE_CURSOR_VISIBLE, true);
    // Text screens such as the file picker turn line wrapping off.
    set_mode(&mut out, MODE_AUTOWRAP, true);
    // Restore the saved window title.
    out.extend_from_slice(b"\x1b[23;0t");
    out
}

fn set_mode(out: &mut Vec<u8>, mode: u16, enabled: bool) {
    let action = if enabled { 'h' } else { 'l' };
    out.extend_from_slice(format!("\x1b[?{mode}{action}").as_bytes());
}

/// DECAWM: whether text that reaches the right edge wraps to the next line.
pub fn autowrap(out: &mut Vec<u8>, enabled: bool) {
    set_mode(out, MODE_AUTOWRAP, enabled);
}

/// CUP: moves the cursor to a zero-based cell.
pub fn move_cursor(out: &mut Vec<u8>, row: u16, column: u16) {
    let row = u32::from(row) + 1;
    let column = u32::from(column) + 1;
    out.extend_from_slice(format!("\x1b[{row};{column}H").as_bytes());
}

/// OSC 2: window title.
pub fn title(out: &mut Vec<u8>, title: &str) {
    out.extend_from_slice(b"\x1b]2;");
    out.extend(title.bytes().filter(|byte| !byte.is_ascii_control()));
    out.extend_from_slice(STRING_TERMINATOR);
}

/// OSC 22: mouse pointer shape, using CSS cursor names.
pub fn pointer_shape(out: &mut Vec<u8>, name: &str) {
    out.extend_from_slice(b"\x1b]22;");
    out.extend_from_slice(name.as_bytes());
    out.extend_from_slice(STRING_TERMINATOR);
}

/// OSC 52: put text on the system clipboard.
pub fn copy_to_clipboard(out: &mut Vec<u8>, text: &str) {
    out.extend_from_slice(b"\x1b]52;c;");
    out.extend_from_slice(BASE64.encode(text).as_bytes());
    out.extend_from_slice(STRING_TERMINATOR);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exit_disables_every_mode_enter_enabled() {
        let capabilities = Capabilities {
            kitty_keyboard: true,
            sgr_pixels: true,
            in_band_resize: true,
            ..Capabilities::default()
        };
        let enter = String::from_utf8_lossy(&enter(&capabilities)).into_owned();
        let exit = String::from_utf8_lossy(&exit(&capabilities)).into_owned();
        for mode in enabled_modes(&capabilities) {
            assert!(enter.contains(&format!("\x1b[?{mode}h")));
            assert!(exit.contains(&format!("\x1b[?{mode}l")));
        }
        assert!(enter.contains("\x1b[>31u"));
        assert!(exit.contains("\x1b[<u"));
        assert!(exit.contains("\x1b[?7h"));
        assert!(!enter.contains("\x1b[?2031h"));
    }

    #[test]
    fn cursor_positions_are_one_based() {
        let mut out = Vec::new();
        move_cursor(&mut out, 0, 12);
        assert_eq!(out, b"\x1b[1;13H");
    }

    #[test]
    fn title_strips_control_characters() {
        let mut out = Vec::new();
        title(&mut out, "a\x1bb\x07c");
        assert_eq!(out, b"\x1b]2;abc\x1b\\");
    }

    #[test]
    fn clipboard_is_base64() {
        let mut out = Vec::new();
        copy_to_clipboard(&mut out, "hi");
        assert_eq!(out, b"\x1b]52;c;aGk=\x1b\\");
    }
}

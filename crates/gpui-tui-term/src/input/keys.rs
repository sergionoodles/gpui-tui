//! Key code tables for the kitty keyboard protocol and legacy xterm sequences.

use super::event::{Key, ModifierKey};

const KITTY_ENTER: u32 = 13;
const KITTY_TAB: u32 = 9;
const KITTY_ESCAPE: u32 = 27;
const KITTY_BACKSPACE: u32 = 127;
const KITTY_F13: u32 = 57376;
const KITTY_F35: u32 = 57398;
const KITTY_KEYPAD_0: u32 = 57399;
const KITTY_KEYPAD_9: u32 = 57408;
const PRIVATE_USE_START: u32 = 57344;
const PRIVATE_USE_END: u32 = 63743;

/// Maps the key code of a kitty `CSI … u` sequence to a key.
pub(super) fn kitty_key(code: u32) -> Key {
    match code {
        KITTY_ENTER => Key::Enter,
        KITTY_TAB => Key::Tab,
        KITTY_ESCAPE => Key::Escape,
        KITTY_BACKSPACE => Key::Backspace,
        57358 => Key::CapsLock,
        57359 => Key::ScrollLock,
        57360 => Key::NumLock,
        57361 => Key::PrintScreen,
        57362 => Key::Pause,
        57363 => Key::Menu,
        KITTY_F13..=KITTY_F35 => Key::Function(function_number(code - KITTY_F13 + 13)),
        KITTY_KEYPAD_0..=KITTY_KEYPAD_9 => keypad_digit(code - KITTY_KEYPAD_0),
        57409 => Key::Char('.'),
        57410 => Key::Char('/'),
        57411 => Key::Char('*'),
        57412 => Key::Char('-'),
        57413 => Key::Char('+'),
        57414 => Key::Enter,
        57415 => Key::Char('='),
        57416 => Key::Char(','),
        57417 => Key::Left,
        57418 => Key::Right,
        57419 => Key::Up,
        57420 => Key::Down,
        57421 => Key::PageUp,
        57422 => Key::PageDown,
        57423 => Key::Home,
        57424 => Key::End,
        57425 => Key::Insert,
        57426 => Key::Delete,
        57441 | 57447 => Key::Modifier(ModifierKey::Shift),
        57442 | 57448 => Key::Modifier(ModifierKey::Control),
        57443 | 57449 => Key::Modifier(ModifierKey::Alt),
        57444 | 57450 => Key::Modifier(ModifierKey::Super),
        57445 | 57451 => Key::Modifier(ModifierKey::Hyper),
        57446 | 57452 => Key::Modifier(ModifierKey::Meta),
        PRIVATE_USE_START..=PRIVATE_USE_END => Key::Unknown(code),
        _ => char::from_u32(code).map_or(Key::Unknown(code), Key::Char),
    }
}

/// Maps the number of a `CSI number ~` sequence to a key.
pub(super) fn tilde_key(number: u32) -> Key {
    match number {
        2 => Key::Insert,
        3 => Key::Delete,
        5 => Key::PageUp,
        6 => Key::PageDown,
        7 => Key::Home,
        8 => Key::End,
        11..=15 => Key::Function(function_number(number - 10)),
        17..=21 => Key::Function(function_number(number - 11)),
        23 | 24 => Key::Function(function_number(number - 12)),
        29 => Key::Menu,
        _ => Key::Unknown(number),
    }
}

/// Maps the final byte of `CSI 1 ; modifiers <final>` and `SS3 <final>` sequences to a key.
pub(super) fn letter_key(final_byte: u8) -> Option<Key> {
    let key = match final_byte {
        b'A' => Key::Up,
        b'B' => Key::Down,
        b'C' => Key::Right,
        b'D' => Key::Left,
        b'H' => Key::Home,
        b'F' => Key::End,
        b'M' => Key::Enter,
        b'P' => Key::Function(1),
        b'Q' => Key::Function(2),
        b'R' => Key::Function(3),
        b'S' => Key::Function(4),
        _ => return None,
    };
    Some(key)
}

fn function_number(number: u32) -> u8 {
    u8::try_from(number).unwrap_or(u8::MAX)
}

fn keypad_digit(offset: u32) -> Key {
    char::from_digit(offset, 10).map_or(Key::Unknown(KITTY_KEYPAD_0 + offset), Key::Char)
}

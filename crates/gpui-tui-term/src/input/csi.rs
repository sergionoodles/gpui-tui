//! Decoding of complete CSI sequences into events.

use super::event::{
    ColorScheme, GridSize, Key, KeyEvent, KeyEventKind, ModeState, Modifiers, MouseButton,
    MouseEvent, MouseEventKind, Reply, ScrollDirection, TermEvent,
};
use super::keys::{kitty_key, letter_key, tilde_key};

pub(super) const PASTE_START: u32 = 200;

/// What a complete CSI sequence turned out to be.
pub(super) enum CsiOutcome {
    Event(TermEvent),
    PasteStart,
    Ignored,
}

/// A CSI sequence split into its parts: `CSI <marker> <params> <intermediates> <final>`.
pub(super) struct CsiSequence {
    marker: Option<u8>,
    params: Vec<Vec<Option<u32>>>,
    intermediates: Vec<u8>,
    final_byte: u8,
}

impl CsiSequence {
    /// Splits the bytes between `CSI` and the final byte.
    pub(super) fn parse(body: &[u8], final_byte: u8) -> Self {
        let (marker, rest) = match body.first() {
            Some(&byte @ b'<'..=b'?') => (Some(byte), &body[1..]),
            _ => (None, body),
        };
        let parameter_len = rest
            .iter()
            .position(|byte| !matches!(byte, b'0'..=b'9' | b';' | b':'))
            .unwrap_or(rest.len());
        let (parameter_bytes, intermediates) = rest.split_at(parameter_len);

        Self {
            marker,
            params: parse_params(parameter_bytes),
            intermediates: intermediates.to_vec(),
            final_byte,
        }
    }

    fn param(&self, index: usize) -> Option<u32> {
        self.sub_param(index, 0)
    }

    fn sub_param(&self, index: usize, sub_index: usize) -> Option<u32> {
        self.params
            .get(index)
            .and_then(|sub_params| sub_params.get(sub_index).copied().flatten())
    }

    fn param_u16(&self, index: usize) -> u16 {
        self.param(index)
            .and_then(|value| u16::try_from(value).ok())
            .unwrap_or(0)
    }

    pub(super) fn decode(&self) -> CsiOutcome {
        let event = match (self.marker, self.intermediates.as_slice(), self.final_byte) {
            (None, [], b'u') => self.kitty_key_event(),
            (None, [], b'~') if self.param(0) == Some(PASTE_START) => {
                return CsiOutcome::PasteStart;
            }
            (None, [], b'~') => self.tilde_key_event(),
            (None, [], b'Z') => Some(self.legacy_back_tab()),
            (None, [], b'I') => Some(TermEvent::Focus(true)),
            (None, [], b'O') => Some(TermEvent::Focus(false)),
            (None, [], b't') => self.window_report(),
            (None, [], final_byte) => self.letter_key_event(final_byte),
            (Some(b'<'), [], b'M' | b'm') => self.mouse_event(),
            (Some(b'?'), [], b'u') => self
                .param(0)
                .map(|flags| TermEvent::Reply(Reply::KeyboardFlags(flags))),
            (Some(b'?'), [], b'c') => Some(TermEvent::Reply(Reply::PrimaryDeviceAttributes)),
            (Some(b'?'), [b'$'], b'y') => self.mode_report(),
            (Some(b'?'), [], b'n') => self.color_scheme_report(),
            _ => None,
        };
        event.map_or(CsiOutcome::Ignored, CsiOutcome::Event)
    }

    /// `CSI code[:shifted[:base]] ; modifiers[:event] ; text u`
    fn kitty_key_event(&self) -> Option<TermEvent> {
        let code = self.sub_param(0, 0)?;
        let key = kitty_key(code);
        let modifiers = Modifiers::from_parameter(self.sub_param(1, 0).unwrap_or(1));
        let kind = KeyEventKind::from_code(self.sub_param(1, 1).unwrap_or(1));
        let shifted_key = self.sub_param(0, 1).and_then(char::from_u32);
        let text = self
            .text_param()
            .or_else(|| infer_text(key, modifiers, kind, shifted_key));

        Some(TermEvent::Key(KeyEvent {
            key,
            modifiers,
            kind,
            shifted_key,
            text,
        }))
    }

    /// Text codepoints in the third field (kitty flag "report associated text").
    fn text_param(&self) -> Option<String> {
        let codepoints = self.params.get(2)?;
        let text: String = codepoints
            .iter()
            .flatten()
            .filter_map(|&codepoint| char::from_u32(codepoint))
            .collect();
        (!text.is_empty()).then_some(text)
    }

    fn tilde_key_event(&self) -> Option<TermEvent> {
        let key = tilde_key(self.param(0)?);
        Some(self.functional_key_event(key))
    }

    fn letter_key_event(&self, final_byte: u8) -> Option<TermEvent> {
        let key = letter_key(final_byte)?;
        Some(self.functional_key_event(key))
    }

    /// Keys with a `… ; modifiers[:event] <final>` shape.
    fn functional_key_event(&self, key: Key) -> TermEvent {
        let modifiers = Modifiers::from_parameter(self.sub_param(1, 0).unwrap_or(1));
        let kind = KeyEventKind::from_code(self.sub_param(1, 1).unwrap_or(1));
        TermEvent::Key(KeyEvent {
            kind,
            ..KeyEvent::press(key, modifiers)
        })
    }

    fn legacy_back_tab(&self) -> TermEvent {
        let modifiers = Modifiers {
            shift: true,
            ..Modifiers::default()
        };
        TermEvent::Key(KeyEvent::press(Key::Tab, modifiers))
    }

    /// `CSI < button ; column ; row M|m`
    fn mouse_event(&self) -> Option<TermEvent> {
        const MOTION: u32 = 32;
        const WHEEL: u32 = 64;
        const EXTRA_BUTTONS: u32 = 128;
        const BUTTON_MASK: u32 = 3;

        let bits = self.param(0)?;
        let column = self.param(1)?.saturating_sub(1);
        let row = self.param(2)?.saturating_sub(1);
        let modifiers = Modifiers::from_mouse_bits(bits);
        let button_code = bits & BUTTON_MASK;
        let is_release = self.final_byte == b'm';

        let kind = if bits & WHEEL != 0 {
            MouseEventKind::Scroll(scroll_direction(button_code))
        } else {
            let button = if bits & EXTRA_BUTTONS != 0 {
                extra_button(button_code)
            } else {
                primary_button(button_code)
            };
            match (bits & MOTION != 0, is_release, button) {
                (true, _, pressed) => MouseEventKind::Move { pressed },
                (false, false, Some(button)) => MouseEventKind::Press(button),
                (false, true, Some(button)) => MouseEventKind::Release(button),
                (false, _, None) => return None,
            }
        };

        Some(TermEvent::Mouse(MouseEvent {
            kind,
            column,
            row,
            modifiers,
        }))
    }

    /// `CSI 6 ; height ; width t` (cell size) and `CSI 48 ; rows ; cols ; height ; width t`.
    fn window_report(&self) -> Option<TermEvent> {
        const CELL_SIZE_REPORT: u32 = 6;
        const IN_BAND_RESIZE_REPORT: u32 = 48;

        match self.param(0)? {
            CELL_SIZE_REPORT => Some(TermEvent::Reply(Reply::CellSize {
                height: self.param_u16(1),
                width: self.param_u16(2),
            })),
            IN_BAND_RESIZE_REPORT => Some(TermEvent::Resize(GridSize {
                rows: self.param_u16(1),
                cols: self.param_u16(2),
                height_px: self.param_u16(3),
                width_px: self.param_u16(4),
            })),
            _ => None,
        }
    }

    /// `CSI ? mode ; state $ y`
    fn mode_report(&self) -> Option<TermEvent> {
        let mode = u16::try_from(self.param(0)?).ok()?;
        let state = ModeState::from_code(self.param(1).unwrap_or(0));
        Some(TermEvent::Reply(Reply::Mode { mode, state }))
    }

    /// `CSI ? 997 ; 1|2 n`
    fn color_scheme_report(&self) -> Option<TermEvent> {
        const COLOR_SCHEME_REPORT: u32 = 997;

        if self.param(0)? != COLOR_SCHEME_REPORT {
            return None;
        }
        let scheme = match self.param(1)? {
            1 => ColorScheme::Dark,
            2 => ColorScheme::Light,
            _ => return None,
        };
        Some(TermEvent::Reply(Reply::ColorScheme(scheme)))
    }
}

fn parse_params(bytes: &[u8]) -> Vec<Vec<Option<u32>>> {
    if bytes.is_empty() {
        return Vec::new();
    }
    bytes
        .split(|&byte| byte == b';')
        .map(|param| {
            param
                .split(|&byte| byte == b':')
                .map(parse_number)
                .collect()
        })
        .collect()
}

fn parse_number(digits: &[u8]) -> Option<u32> {
    if digits.is_empty() {
        return None;
    }
    digits.iter().try_fold(0u32, |value, &digit| {
        value
            .checked_mul(10)?
            .checked_add(u32::from(digit.wrapping_sub(b'0')))
    })
}

/// Text a key would type, for terminals that do not report associated text.
fn infer_text(
    key: Key,
    modifiers: Modifiers,
    kind: KeyEventKind,
    shifted_key: Option<char>,
) -> Option<String> {
    let Key::Char(character) = key else {
        return None;
    };
    if kind == KeyEventKind::Release || modifiers.control || modifiers.alt || modifiers.super_key {
        return None;
    }
    let typed = match (modifiers.shift, shifted_key) {
        (true, Some(shifted)) => shifted,
        (true, None) => character.to_uppercase().next().unwrap_or(character),
        (false, _) => character,
    };
    Some(typed.to_string())
}

fn scroll_direction(button_code: u32) -> ScrollDirection {
    match button_code {
        0 => ScrollDirection::Up,
        1 => ScrollDirection::Down,
        2 => ScrollDirection::Left,
        _ => ScrollDirection::Right,
    }
}

fn primary_button(button_code: u32) -> Option<MouseButton> {
    match button_code {
        0 => Some(MouseButton::Left),
        1 => Some(MouseButton::Middle),
        2 => Some(MouseButton::Right),
        _ => None,
    }
}

fn extra_button(button_code: u32) -> Option<MouseButton> {
    match button_code {
        0 => Some(MouseButton::Back),
        1 => Some(MouseButton::Forward),
        _ => None,
    }
}

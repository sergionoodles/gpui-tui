//! Byte-level state machine that turns terminal input into [`TermEvent`]s.

use super::csi::{CsiOutcome, CsiSequence};
use super::event::{GraphicsReply, Key, KeyEvent, Modifiers, Reply, Rgb, TermEvent};
use super::keys::letter_key;

const ESCAPE: u8 = 0x1b;
const BELL: u8 = 0x07;
const STRING_TERMINATOR: u8 = b'\\';
const DELETE: u8 = 0x7f;
const PASTE_END: &[u8] = b"\x1b[201~";
/// Longest CSI sequence we accept before treating the input as garbage.
const MAX_SEQUENCE_LEN: usize = 4096;
/// Longest OSC/APC payload or paste we buffer.
const MAX_STRING_LEN: usize = 16 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    Ss3,
    Osc {
        saw_escape: bool,
    },
    Apc {
        saw_escape: bool,
    },
    /// DCS, SOS and PM strings: consumed and dropped.
    IgnoredString {
        saw_escape: bool,
    },
    Paste,
}

/// Incremental parser for bytes read from the terminal.
///
/// Input may be split at any byte; state carries over between [`Parser::feed`] calls.
pub struct Parser {
    state: State,
    buffer: Vec<u8>,
    utf8: Vec<u8>,
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

impl Parser {
    pub fn new() -> Self {
        Self {
            state: State::Ground,
            buffer: Vec::new(),
            utf8: Vec::new(),
        }
    }

    /// Parses `bytes` and returns the events they complete.
    pub fn feed(&mut self, bytes: &[u8]) -> Vec<TermEvent> {
        let mut events = Vec::new();
        for &byte in bytes {
            self.advance(byte, &mut events);
        }
        events
    }

    /// Whether the input ended right after an ESC byte. Callers should wait briefly for more
    /// input and then call [`Parser::flush_pending_escape`]: a lone ESC is the Escape key.
    pub fn has_pending_escape(&self) -> bool {
        self.state == State::Escape
    }

    /// Resolves a pending lone ESC into an Escape key press.
    pub fn flush_pending_escape(&mut self) -> Option<TermEvent> {
        if !self.has_pending_escape() {
            return None;
        }
        self.state = State::Ground;
        Some(key_event(Key::Escape, Modifiers::default(), None))
    }

    fn advance(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        match self.state {
            State::Ground => self.advance_ground(byte, events),
            State::Escape => self.advance_escape(byte, events),
            State::Csi => self.advance_csi(byte, events),
            State::Ss3 => self.advance_ss3(byte, events),
            State::Osc { saw_escape } => {
                self.advance_string(byte, saw_escape, events, |saw_escape| State::Osc {
                    saw_escape,
                });
            }
            State::Apc { saw_escape } => {
                self.advance_string(byte, saw_escape, events, |saw_escape| State::Apc {
                    saw_escape,
                });
            }
            State::IgnoredString { saw_escape } => {
                self.advance_string(byte, saw_escape, events, |saw_escape| {
                    State::IgnoredString { saw_escape }
                });
            }
            State::Paste => self.advance_paste(byte, events),
        }
    }

    fn advance_ground(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        if byte == ESCAPE {
            self.utf8.clear();
            self.state = State::Escape;
            return;
        }
        if let Some(event) = self.text_byte_event(byte, Modifiers::default()) {
            events.push(event);
        }
    }

    fn advance_escape(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        self.buffer.clear();
        self.state = match byte {
            b'[' => State::Csi,
            b'O' => State::Ss3,
            b']' => State::Osc { saw_escape: false },
            b'_' => State::Apc { saw_escape: false },
            b'P' | b'X' | b'^' => State::IgnoredString { saw_escape: false },
            ESCAPE => {
                events.push(key_event(Key::Escape, Modifiers::default(), None));
                State::Escape
            }
            _ => {
                let alt = Modifiers {
                    alt: true,
                    ..Modifiers::default()
                };
                if let Some(event) = self.text_byte_event(byte, alt) {
                    events.push(event);
                }
                State::Ground
            }
        };
    }

    fn advance_csi(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        match byte {
            0x40..=0x7e => {
                let sequence = CsiSequence::parse(&self.buffer, byte);
                self.buffer.clear();
                self.state = State::Ground;
                match sequence.decode() {
                    CsiOutcome::Event(event) => events.push(event),
                    CsiOutcome::PasteStart => self.state = State::Paste,
                    CsiOutcome::Ignored => {}
                }
            }
            ESCAPE => self.state = State::Escape,
            _ if self.buffer.len() >= MAX_SEQUENCE_LEN => {
                self.buffer.clear();
                self.state = State::Ground;
            }
            _ => self.buffer.push(byte),
        }
    }

    fn advance_ss3(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        self.state = State::Ground;
        if let Some(key) = letter_key(byte) {
            events.push(key_event(key, Modifiers::default(), None));
        }
    }

    /// OSC, APC and ignored strings end with BEL or `ESC \`.
    fn advance_string(
        &mut self,
        byte: u8,
        saw_escape: bool,
        events: &mut Vec<TermEvent>,
        next_state: impl Fn(bool) -> State,
    ) {
        match (saw_escape, byte) {
            (false, ESCAPE) => {
                self.state = next_state(true);
                return;
            }
            (false, BELL) | (true, _) => {}
            (false, _) => {
                if self.buffer.len() < MAX_STRING_LEN {
                    self.buffer.push(byte);
                }
                return;
            }
        }

        let payload = std::mem::take(&mut self.buffer);
        let event = match self.state {
            State::Osc { .. } => osc_event(&payload),
            State::Apc { .. } => apc_event(&payload),
            _ => None,
        };
        events.extend(event);
        self.state = State::Ground;

        // `ESC` followed by anything other than `\` starts a new sequence.
        if saw_escape && byte != STRING_TERMINATOR {
            self.state = State::Escape;
            self.advance(byte, events);
        }
    }

    fn advance_paste(&mut self, byte: u8, events: &mut Vec<TermEvent>) {
        if self.buffer.len() < MAX_STRING_LEN {
            self.buffer.push(byte);
        }
        if !self.buffer.ends_with(PASTE_END) {
            return;
        }
        let text_len = self.buffer.len() - PASTE_END.len();
        let text = String::from_utf8_lossy(&self.buffer[..text_len]).into_owned();
        self.buffer.clear();
        self.state = State::Ground;
        events.push(TermEvent::Paste(text));
    }

    /// Legacy (non-kitty) encoding of a single typed byte: text, or a control character.
    fn text_byte_event(&mut self, byte: u8, modifiers: Modifiers) -> Option<TermEvent> {
        if byte >= 0x80 || !self.utf8.is_empty() {
            return self.utf8_byte_event(byte, modifiers);
        }
        let control = Modifiers {
            control: true,
            ..modifiers
        };
        let event = match byte {
            b'\r' => key_event(Key::Enter, modifiers, None),
            b'\t' => key_event(Key::Tab, modifiers, None),
            DELETE | 0x08 => key_event(Key::Backspace, modifiers, None),
            0x00 => key_event(Key::Char(' '), control, None),
            0x01..=0x1a => key_event(Key::Char(char::from(byte + 0x60)), control, None),
            0x1c..=0x1f => key_event(Key::Char(char::from(byte + 0x40)), control, None),
            _ => character_event(char::from(byte), modifiers),
        };
        Some(event)
    }

    fn utf8_byte_event(&mut self, byte: u8, modifiers: Modifiers) -> Option<TermEvent> {
        self.utf8.push(byte);
        match std::str::from_utf8(&self.utf8) {
            Ok(text) => {
                let character = text.chars().next()?;
                self.utf8.clear();
                Some(character_event(character, modifiers))
            }
            Err(error) if error.error_len().is_some() => {
                self.utf8.clear();
                None
            }
            Err(_) => None,
        }
    }
}

fn key_event(key: Key, modifiers: Modifiers, text: Option<String>) -> TermEvent {
    TermEvent::Key(KeyEvent {
        text,
        ..KeyEvent::press(key, modifiers)
    })
}

/// A typed character in legacy mode, normalized to the kitty shape (unshifted key + text).
fn character_event(character: char, modifiers: Modifiers) -> TermEvent {
    let is_upper = character.is_uppercase();
    let key = character.to_lowercase().next().unwrap_or(character);
    let modifiers = Modifiers {
        shift: modifiers.shift || is_upper,
        ..modifiers
    };
    let text = (!modifiers.alt).then(|| character.to_string());
    TermEvent::Key(KeyEvent {
        shifted_key: is_upper.then_some(character),
        text,
        ..KeyEvent::press(Key::Char(key), modifiers)
    })
}

/// `OSC 11 ; rgb:RRRR/GGGG/BBBB`
fn osc_event(payload: &[u8]) -> Option<TermEvent> {
    let payload = std::str::from_utf8(payload).ok()?;
    let color = payload.strip_prefix("11;")?;
    parse_x11_color(color).map(|rgb| TermEvent::Reply(Reply::Background(rgb)))
}

fn parse_x11_color(color: &str) -> Option<Rgb> {
    let mut components = color
        .strip_prefix("rgb:")?
        .split('/')
        .map(parse_hex_component);
    let red = components.next()??;
    let green = components.next()??;
    let blue = components.next()??;
    Some(Rgb { red, green, blue })
}

/// Scales a 1–4 digit hex color component to 8 bits.
fn parse_hex_component(component: &str) -> Option<u8> {
    if component.is_empty() || component.len() > 4 {
        return None;
    }
    let value = u32::from_str_radix(component, 16).ok()?;
    let digits = u32::try_from(component.len()).ok()?;
    let max = 16u32.pow(digits) - 1;
    u8::try_from(value * 255 / max).ok()
}

/// `APC G key=value,… ; message`
fn apc_event(payload: &[u8]) -> Option<TermEvent> {
    let payload = std::str::from_utf8(payload).ok()?;
    let body = payload.strip_prefix('G')?;
    let (keys, message) = body.split_once(';')?;
    let image_id = keys
        .split(',')
        .find_map(|pair| pair.strip_prefix("i="))
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    Some(TermEvent::Reply(Reply::Graphics(GraphicsReply {
        image_id,
        message: message.to_owned(),
    })))
}

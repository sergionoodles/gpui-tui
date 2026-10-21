//! Splitting the child's output into the control sequences a terminal would act on.

const ESCAPE: u8 = 0x1b;
const BELL: u8 = 0x07;
const DELETE: u8 = 0x7f;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum Sequence {
    /// `CSI <body> <final>`; `body` holds parameters and intermediates.
    Csi {
        body: Vec<u8>,
        final_byte: u8,
    },
    Osc(Vec<u8>),
    Apc(Vec<u8>),
    /// A byte of printed text.
    Print(u8),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum State {
    Ground,
    Escape,
    Csi,
    String { kind: StringKind, saw_escape: bool },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StringKind {
    Osc,
    Apc,
    Ignored,
}

/// Incremental parser; control characters and unknown sequences are dropped.
pub(crate) struct OutputParser {
    state: State,
    buffer: Vec<u8>,
}

impl OutputParser {
    pub(crate) fn new() -> Self {
        Self {
            state: State::Ground,
            buffer: Vec::new(),
        }
    }

    pub(crate) fn feed(&mut self, bytes: &[u8]) -> Vec<Sequence> {
        let mut sequences = Vec::new();
        for &byte in bytes {
            if let Some(sequence) = self.advance(byte) {
                sequences.push(sequence);
            }
        }
        sequences
    }

    fn advance(&mut self, byte: u8) -> Option<Sequence> {
        match self.state {
            State::Ground => match byte {
                ESCAPE => {
                    self.state = State::Escape;
                    None
                }
                DELETE | 0x00..=0x1f => None,
                _ => Some(Sequence::Print(byte)),
            },
            State::Escape => {
                self.buffer.clear();
                self.state = match byte {
                    b'[' => State::Csi,
                    b']' => self.string(StringKind::Osc),
                    b'_' => self.string(StringKind::Apc),
                    b'P' | b'X' | b'^' => self.string(StringKind::Ignored),
                    ESCAPE => State::Escape,
                    _ => State::Ground,
                };
                None
            }
            State::Csi => {
                if (0x40..=0x7e).contains(&byte) {
                    self.state = State::Ground;
                    return Some(Sequence::Csi {
                        body: std::mem::take(&mut self.buffer),
                        final_byte: byte,
                    });
                }
                self.buffer.push(byte);
                None
            }
            State::String { kind, saw_escape } => self.advance_string(kind, saw_escape, byte),
        }
    }

    fn string(&self, kind: StringKind) -> State {
        State::String {
            kind,
            saw_escape: false,
        }
    }

    fn advance_string(&mut self, kind: StringKind, saw_escape: bool, byte: u8) -> Option<Sequence> {
        match (saw_escape, byte) {
            (false, ESCAPE) => {
                self.state = State::String {
                    kind,
                    saw_escape: true,
                };
                None
            }
            (false, BELL) | (true, _) => {
                self.state = State::Ground;
                let payload = std::mem::take(&mut self.buffer);
                match kind {
                    StringKind::Osc => Some(Sequence::Osc(payload)),
                    StringKind::Apc => Some(Sequence::Apc(payload)),
                    StringKind::Ignored => None,
                }
            }
            (false, _) => {
                self.buffer.push(byte);
                None
            }
        }
    }
}

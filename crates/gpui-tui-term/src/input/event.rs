/// Something the terminal sent us: user input or a reply to one of our queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TermEvent {
    Key(KeyEvent),
    Mouse(MouseEvent),
    Paste(String),
    Focus(bool),
    /// In-band resize report (mode 2048).
    Resize(GridSize),
    Reply(Reply),
}

/// A reply to a query we sent to the terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reply {
    /// Primary device attributes (`CSI ? … c`). Every terminal answers this, so it terminates
    /// capability probes.
    PrimaryDeviceAttributes,
    /// DECRPM reply to a DECRQM mode query.
    Mode { mode: u16, state: ModeState },
    /// Current kitty keyboard protocol flags (`CSI ? flags u`).
    KeyboardFlags(u32),
    /// Cell size in pixels (`CSI 6 ; height ; width t`).
    CellSize { width: u16, height: u16 },
    /// Default background color (OSC 11).
    Background(Rgb),
    /// Color scheme report (`CSI ? 997 ; 1|2 n`).
    ColorScheme(ColorScheme),
    /// Kitty graphics protocol response (`ESC _ G i=… ; message ESC \`).
    Graphics(GraphicsReply),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModeState {
    NotRecognized,
    Set,
    Reset,
    PermanentlySet,
    PermanentlyReset,
}

impl ModeState {
    pub fn from_code(code: u32) -> Self {
        match code {
            1 => Self::Set,
            2 => Self::Reset,
            3 => Self::PermanentlySet,
            4 => Self::PermanentlyReset,
            _ => Self::NotRecognized,
        }
    }

    /// Whether the terminal knows the mode and lets us change it.
    pub fn is_supported(self) -> bool {
        matches!(self, Self::Set | Self::Reset | Self::PermanentlySet)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColorScheme {
    Dark,
    Light,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rgb {
    pub red: u8,
    pub green: u8,
    pub blue: u8,
}

impl Rgb {
    /// Relative luminance in `0.0..=1.0` (sRGB coefficients, no gamma correction).
    pub fn luminance(self) -> f32 {
        let red = f32::from(self.red) / 255.0;
        let green = f32::from(self.green) / 255.0;
        let blue = f32::from(self.blue) / 255.0;
        0.2126 * red + 0.7152 * green + 0.0722 * blue
    }

    pub fn color_scheme(self) -> ColorScheme {
        const DARK_THRESHOLD: f32 = 0.5;

        if self.luminance() < DARK_THRESHOLD {
            ColorScheme::Dark
        } else {
            ColorScheme::Light
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GraphicsReply {
    pub image_id: u32,
    /// `"OK"` on success, otherwise an error such as `"ENOENT:…"`.
    pub message: String,
}

impl GraphicsReply {
    pub fn is_ok(&self) -> bool {
        self.message == "OK"
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub rows: u16,
    pub cols: u16,
    pub height_px: u16,
    pub width_px: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub control: bool,
    pub super_key: bool,
    pub hyper: bool,
    pub meta: bool,
    pub caps_lock: bool,
    pub num_lock: bool,
}

impl Modifiers {
    /// Decodes the `1 + bitmask` modifier parameter used by xterm and the kitty protocol.
    pub fn from_parameter(parameter: u32) -> Self {
        let bits = parameter.saturating_sub(1);
        Self {
            shift: bits & 1 != 0,
            alt: bits & 2 != 0,
            control: bits & 4 != 0,
            super_key: bits & 8 != 0,
            hyper: bits & 16 != 0,
            meta: bits & 32 != 0,
            caps_lock: bits & 64 != 0,
            num_lock: bits & 128 != 0,
        }
    }

    /// Decodes the modifier bits of an SGR mouse button parameter.
    pub fn from_mouse_bits(bits: u32) -> Self {
        Self {
            shift: bits & 4 != 0,
            alt: bits & 8 != 0,
            control: bits & 16 != 0,
            ..Self::default()
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub modifiers: Modifiers,
    pub kind: KeyEventKind,
    /// The key with shift applied, when the terminal reports it (kitty "alternate keys").
    pub shifted_key: Option<char>,
    /// The text this key press produces, if any.
    pub text: Option<String>,
}

impl KeyEvent {
    pub fn press(key: Key, modifiers: Modifiers) -> Self {
        Self {
            key,
            modifiers,
            kind: KeyEventKind::Press,
            shifted_key: None,
            text: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyEventKind {
    Press,
    Repeat,
    Release,
}

impl KeyEventKind {
    pub fn from_code(code: u32) -> Self {
        match code {
            2 => Self::Repeat,
            3 => Self::Release,
            _ => Self::Press,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    /// A text key, reported unshifted (`a`, not `A`; `1`, not `!`).
    Char(char),
    Enter,
    Tab,
    Backspace,
    Escape,
    Insert,
    Delete,
    Left,
    Right,
    Up,
    Down,
    Home,
    End,
    PageUp,
    PageDown,
    Function(u8),
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    Menu,
    Modifier(ModifierKey),
    /// A key we do not map, with its kitty key code.
    Unknown(u32),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierKey {
    Shift,
    Control,
    Alt,
    Super,
    Hyper,
    Meta,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub kind: MouseEventKind,
    /// Zero-based position: pixels when SGR-pixel mode (1016) is on, cells otherwise.
    pub column: u32,
    pub row: u32,
    pub modifiers: Modifiers,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEventKind {
    Press(MouseButton),
    Release(MouseButton),
    Move { pressed: Option<MouseButton> },
    Scroll(ScrollDirection),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Middle,
    Right,
    Back,
    Forward,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScrollDirection {
    Up,
    Down,
    Left,
    Right,
}

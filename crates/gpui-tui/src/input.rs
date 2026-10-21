//! Translation of terminal input events into GPUI platform input.

use std::time::{Duration, Instant};

use gpui::{
    KeyDownEvent, KeyUpEvent, Keystroke, Modifiers, ModifiersChangedEvent, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, NavigationDirection, Pixels, PlatformInput,
    Point, ScrollDelta, ScrollWheelEvent, TouchPhase, point, px,
};
use gpui_tui_term::input::{self as term, Key, KeyEventKind, MouseEventKind, ScrollDirection};

/// Matches GTK's defaults, like GPUI's Linux backends.
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);
const DOUBLE_CLICK_DISTANCE: Pixels = px(5.0);
/// Lines scrolled per wheel notch, like GPUI's Linux backends.
const SCROLL_LINES: f32 = 3.0;

/// Converts terminal events into GPUI events, tracking modifier, button, and click state.
#[derive(Default)]
pub(crate) struct InputTranslator {
    modifiers: Modifiers,
    caps_lock: bool,
    pressed_button: Option<MouseButton>,
    mouse_position: Option<Point<Pixels>>,
    last_click: Option<Click>,
}

#[derive(Clone, Copy)]
struct Click {
    button: MouseButton,
    position: Point<Pixels>,
    at: Instant,
    count: usize,
}

impl InputTranslator {
    pub(crate) fn modifiers(&self) -> Modifiers {
        self.modifiers
    }

    pub(crate) fn caps_lock(&self) -> bool {
        self.caps_lock
    }

    pub(crate) fn mouse_position(&self) -> Point<Pixels> {
        self.mouse_position.unwrap_or_default()
    }

    pub(crate) fn key(&mut self, event: &term::KeyEvent) -> Vec<PlatformInput> {
        let mut inputs = Vec::new();
        if let Key::Modifier(modifier) = event.key {
            let mut modifiers = gpui_modifiers(event.modifiers);
            set_modifier(
                &mut modifiers,
                modifier,
                event.kind != KeyEventKind::Release,
            );
            self.update_modifiers(modifiers, event.modifiers.caps_lock, &mut inputs);
            return inputs;
        }

        self.update_modifiers(
            gpui_modifiers(event.modifiers),
            event.modifiers.caps_lock,
            &mut inputs,
        );
        let Some(keystroke) = keystroke(event) else {
            return inputs;
        };
        inputs.push(match event.kind {
            KeyEventKind::Release => PlatformInput::KeyUp(KeyUpEvent { keystroke }),
            KeyEventKind::Press | KeyEventKind::Repeat => PlatformInput::KeyDown(KeyDownEvent {
                keystroke,
                is_held: event.kind == KeyEventKind::Repeat,
                prefer_character_input: false,
            }),
        });
        inputs
    }

    /// `position` is the event position converted to window coordinates.
    pub(crate) fn mouse(
        &mut self,
        event: &term::MouseEvent,
        position: Point<Pixels>,
        now: Instant,
    ) -> Vec<PlatformInput> {
        let mut inputs = Vec::new();
        self.update_modifiers(gpui_modifiers(event.modifiers), self.caps_lock, &mut inputs);
        let modifiers = self.modifiers;
        let moved = self.mouse_position != Some(position);
        self.mouse_position = Some(position);

        match event.kind {
            MouseEventKind::Press(button) => {
                let button = gpui_button(button);
                // Hover state must be current before the press, or GPUI hit-tests stale state.
                if moved {
                    inputs.push(self.move_input(position, modifiers));
                }
                let click_count = self.register_click(button, position, now);
                self.pressed_button = Some(button);
                inputs.push(PlatformInput::MouseDown(MouseDownEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                    first_mouse: false,
                }));
            }
            MouseEventKind::Release(button) => {
                let button = gpui_button(button);
                self.pressed_button = None;
                let click_count = self.last_click.map_or(1, |click| click.count);
                inputs.push(PlatformInput::MouseUp(MouseUpEvent {
                    button,
                    position,
                    modifiers,
                    click_count,
                }));
            }
            MouseEventKind::Move { pressed } => {
                self.pressed_button = pressed.map(gpui_button);
                inputs.push(self.move_input(position, modifiers));
            }
            MouseEventKind::Scroll(direction) => {
                inputs.push(PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position,
                    delta: ScrollDelta::Lines(scroll_lines(direction, modifiers.shift)),
                    modifiers,
                    touch_phase: TouchPhase::Moved,
                }));
            }
        }
        inputs
    }

    fn move_input(&self, position: Point<Pixels>, modifiers: Modifiers) -> PlatformInput {
        PlatformInput::MouseMove(MouseMoveEvent {
            position,
            pressed_button: self.pressed_button,
            modifiers,
        })
    }

    fn update_modifiers(
        &mut self,
        modifiers: Modifiers,
        caps_lock: bool,
        inputs: &mut Vec<PlatformInput>,
    ) {
        if modifiers == self.modifiers && caps_lock == self.caps_lock {
            return;
        }
        self.modifiers = modifiers;
        self.caps_lock = caps_lock;
        inputs.push(PlatformInput::ModifiersChanged(ModifiersChangedEvent {
            modifiers,
            capslock: gpui::Capslock { on: caps_lock },
        }));
    }

    fn register_click(
        &mut self,
        button: MouseButton,
        position: Point<Pixels>,
        now: Instant,
    ) -> usize {
        let count = match self.last_click {
            Some(click)
                if click.button == button
                    && now.duration_since(click.at) <= DOUBLE_CLICK_INTERVAL
                    && (click.position.x - position.x).abs() <= DOUBLE_CLICK_DISTANCE
                    && (click.position.y - position.y).abs() <= DOUBLE_CLICK_DISTANCE =>
            {
                click.count + 1
            }
            _ => 1,
        };
        self.last_click = Some(Click {
            button,
            position,
            at: now,
            count,
        });
        count
    }
}

/// Builds the GPUI keystroke for a key event, following the conventions of GPUI's Linux
/// backends: letters keep `shift` and stay lowercase, shifted symbols drop it.
pub(crate) fn keystroke(event: &term::KeyEvent) -> Option<Keystroke> {
    let mut modifiers = gpui_modifiers(event.modifiers);
    let key = match event.key {
        Key::Char(' ') => "space".to_owned(),
        Key::Char(character) => character_key(character, event.shifted_key, &mut modifiers),
        Key::Enter => "enter".to_owned(),
        Key::Tab => "tab".to_owned(),
        Key::Backspace => "backspace".to_owned(),
        Key::Escape => "escape".to_owned(),
        Key::Insert => "insert".to_owned(),
        Key::Delete => "delete".to_owned(),
        Key::Left => "left".to_owned(),
        Key::Right => "right".to_owned(),
        Key::Up => "up".to_owned(),
        Key::Down => "down".to_owned(),
        Key::Home => "home".to_owned(),
        Key::End => "end".to_owned(),
        Key::PageUp => "pageup".to_owned(),
        Key::PageDown => "pagedown".to_owned(),
        Key::Function(number) => format!("f{number}"),
        Key::Menu => "menu".to_owned(),
        Key::CapsLock
        | Key::ScrollLock
        | Key::NumLock
        | Key::PrintScreen
        | Key::Pause
        | Key::Modifier(_)
        | Key::Unknown(_) => return None,
    };
    let key_char = event
        .text
        .clone()
        .filter(|text| event.kind != KeyEventKind::Release && !text.chars().any(char::is_control));

    Some(Keystroke {
        modifiers,
        key,
        key_char,
    })
}

fn character_key(character: char, shifted: Option<char>, modifiers: &mut Modifiers) -> String {
    if modifiers.shift
        && !character.is_alphabetic()
        && let Some(shifted) = shifted
    {
        modifiers.shift = false;
        return shifted.to_string();
    }
    character.to_lowercase().collect()
}

fn gpui_modifiers(modifiers: term::Modifiers) -> Modifiers {
    Modifiers {
        control: modifiers.control,
        alt: modifiers.alt || modifiers.meta,
        shift: modifiers.shift,
        platform: modifiers.super_key,
        function: false,
    }
}

fn set_modifier(modifiers: &mut Modifiers, modifier: term::ModifierKey, pressed: bool) {
    match modifier {
        term::ModifierKey::Shift => modifiers.shift = pressed,
        term::ModifierKey::Control => modifiers.control = pressed,
        term::ModifierKey::Alt | term::ModifierKey::Meta => modifiers.alt = pressed,
        term::ModifierKey::Super => modifiers.platform = pressed,
        term::ModifierKey::Hyper => {}
    }
}

fn gpui_button(button: term::MouseButton) -> MouseButton {
    match button {
        term::MouseButton::Left => MouseButton::Left,
        term::MouseButton::Middle => MouseButton::Middle,
        term::MouseButton::Right => MouseButton::Right,
        term::MouseButton::Back => MouseButton::Navigate(NavigationDirection::Back),
        term::MouseButton::Forward => MouseButton::Navigate(NavigationDirection::Forward),
    }
}

/// Positive `y` scrolls up and positive `x` scrolls left, as on GPUI's Linux backends. Shift
/// turns vertical scrolling horizontal.
fn scroll_lines(direction: ScrollDirection, shift: bool) -> Point<f32> {
    let (x, y) = match direction {
        ScrollDirection::Up => (0.0, SCROLL_LINES),
        ScrollDirection::Down => (0.0, -SCROLL_LINES),
        ScrollDirection::Left => (SCROLL_LINES, 0.0),
        ScrollDirection::Right => (-SCROLL_LINES, 0.0),
    };
    if shift { point(y, x) } else { point(x, y) }
}

#[cfg(test)]
mod tests;

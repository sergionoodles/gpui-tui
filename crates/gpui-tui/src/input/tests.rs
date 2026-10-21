use super::*;
use gpui_tui_term::input::{KeyEvent, ModifierKey, MouseEvent, Parser, TermEvent};

fn parse_key(bytes: &[u8]) -> KeyEvent {
    match Parser::new().feed(bytes).pop() {
        Some(TermEvent::Key(event)) => event,
        other => panic!("expected a key event, got {other:?}"),
    }
}

fn keystroke_for(bytes: &[u8]) -> Keystroke {
    keystroke(&parse_key(bytes)).unwrap_or_default()
}

fn mouse(kind: MouseEventKind) -> MouseEvent {
    MouseEvent {
        kind,
        column: 0,
        row: 0,
        modifiers: term::Modifiers::default(),
    }
}

#[test]
fn letters_keep_shift_and_symbols_drop_it() {
    let shifted_letter = keystroke_for(b"\x1b[97:65;2;65u");
    assert_eq!(shifted_letter.key, "a");
    assert!(shifted_letter.modifiers.shift);
    assert_eq!(shifted_letter.key_char.as_deref(), Some("A"));

    let shifted_symbol = keystroke_for(b"\x1b[49:33;2;33u");
    assert_eq!(shifted_symbol.key, "!");
    assert!(!shifted_symbol.modifiers.shift);
}

#[test]
fn control_combinations_and_named_keys() {
    let copy = keystroke_for(b"\x1b[99;5u");
    assert_eq!(copy.key, "c");
    assert!(copy.modifiers.control);
    assert_eq!(copy.key_char, None);

    assert_eq!(keystroke_for(b"\x1b[32;1;32u").key, "space");
    assert_eq!(keystroke_for(b"\x1b[13u").key, "enter");
    assert_eq!(keystroke_for(b"\x1b[5~").key, "pageup");
    assert_eq!(keystroke_for(b"\x1b[15~").key, "f5");
    let super_s = keystroke_for(b"\x1b[115;9u");
    assert!(super_s.modifiers.platform);
}

#[test]
fn release_becomes_key_up_and_repeat_is_held() {
    let mut translator = InputTranslator::default();
    let release = translator.key(&parse_key(b"\x1b[97;1:3u"));
    assert!(matches!(release.as_slice(), [PlatformInput::KeyUp(_)]));

    let repeat = translator.key(&parse_key(b"\x1b[97;1:2u"));
    assert!(matches!(
        repeat.as_slice(),
        [PlatformInput::KeyDown(KeyDownEvent { is_held: true, .. })]
    ));
}

#[test]
fn modifier_keys_emit_modifiers_changed_only() {
    let mut translator = InputTranslator::default();
    let press = translator.key(&KeyEvent::press(
        Key::Modifier(ModifierKey::Control),
        term::Modifiers::default(),
    ));
    let [PlatformInput::ModifiersChanged(changed)] = press.as_slice() else {
        panic!("expected a modifiers change, got {press:?}");
    };
    assert!(changed.modifiers.control);
    assert!(translator.modifiers().control);

    let release = translator.key(&KeyEvent {
        kind: KeyEventKind::Release,
        ..KeyEvent::press(
            Key::Modifier(ModifierKey::Control),
            term::Modifiers {
                control: true,
                ..term::Modifiers::default()
            },
        )
    });
    assert!(matches!(
        release.as_slice(),
        [PlatformInput::ModifiersChanged(_)]
    ));
    assert!(!translator.modifiers().control);
}

#[test]
fn press_after_movement_is_preceded_by_a_move() {
    let mut translator = InputTranslator::default();
    let now = Instant::now();
    let inputs = translator.mouse(
        &mouse(MouseEventKind::Press(term::MouseButton::Left)),
        point(px(10.0), px(10.0)),
        now,
    );
    assert!(matches!(
        inputs.as_slice(),
        [PlatformInput::MouseMove(_), PlatformInput::MouseDown(_)]
    ));
}

#[test]
fn quick_presses_at_the_same_spot_count_as_double_click() {
    let mut translator = InputTranslator::default();
    let position = point(px(10.0), px(10.0));
    let start = Instant::now();
    let click_count = |inputs: Vec<PlatformInput>| {
        inputs.iter().find_map(|input| match input {
            PlatformInput::MouseDown(down) => Some(down.click_count),
            _ => None,
        })
    };

    let press = mouse(MouseEventKind::Press(term::MouseButton::Left));
    let release = mouse(MouseEventKind::Release(term::MouseButton::Left));
    assert_eq!(
        click_count(translator.mouse(&press, position, start)),
        Some(1)
    );
    translator.mouse(&release, position, start);
    let second = start + Duration::from_millis(100);
    assert_eq!(
        click_count(translator.mouse(&press, position, second)),
        Some(2)
    );

    let much_later = second + Duration::from_secs(2);
    assert_eq!(
        click_count(translator.mouse(&press, position, much_later)),
        Some(1)
    );
}

#[test]
fn drag_reports_the_pressed_button() {
    let mut translator = InputTranslator::default();
    let inputs = translator.mouse(
        &mouse(MouseEventKind::Move {
            pressed: Some(term::MouseButton::Left),
        }),
        point(px(3.0), px(4.0)),
        Instant::now(),
    );
    assert!(matches!(
        inputs.as_slice(),
        [PlatformInput::MouseMove(MouseMoveEvent {
            pressed_button: Some(MouseButton::Left),
            ..
        })]
    ));
}

#[test]
fn wheel_scrolls_lines_and_shift_scrolls_horizontally() {
    let mut translator = InputTranslator::default();
    let position = point(px(0.0), px(0.0));
    let scroll_delta = |inputs: Vec<PlatformInput>| {
        inputs.into_iter().find_map(|input| match input {
            PlatformInput::ScrollWheel(ScrollWheelEvent {
                delta: ScrollDelta::Lines(lines),
                ..
            }) => Some(lines),
            _ => None,
        })
    };

    let up = mouse(MouseEventKind::Scroll(ScrollDirection::Up));
    assert_eq!(
        scroll_delta(translator.mouse(&up, position, Instant::now())),
        Some(point(0.0, SCROLL_LINES))
    );

    let shift_down = MouseEvent {
        modifiers: term::Modifiers {
            shift: true,
            ..term::Modifiers::default()
        },
        ..mouse(MouseEventKind::Scroll(ScrollDirection::Down))
    };
    assert_eq!(
        scroll_delta(translator.mouse(&shift_down, position, Instant::now())),
        Some(point(-SCROLL_LINES, 0.0))
    );
}

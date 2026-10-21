use super::*;

fn parse(bytes: &[u8]) -> Vec<TermEvent> {
    Parser::new().feed(bytes)
}

fn single(bytes: &[u8]) -> TermEvent {
    let events = parse(bytes);
    assert_eq!(
        events.len(),
        1,
        "expected one event for {bytes:?}, got {events:?}"
    );
    events.into_iter().next().unwrap_or(TermEvent::Focus(false))
}

fn key(bytes: &[u8]) -> KeyEvent {
    match single(bytes) {
        TermEvent::Key(event) => event,
        other => panic!("expected a key event, got {other:?}"),
    }
}

fn mouse(bytes: &[u8]) -> MouseEvent {
    match single(bytes) {
        TermEvent::Mouse(event) => event,
        other => panic!("expected a mouse event, got {other:?}"),
    }
}

fn reply(bytes: &[u8]) -> Reply {
    match single(bytes) {
        TermEvent::Reply(reply) => reply,
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn shift() -> Modifiers {
    Modifiers {
        shift: true,
        ..Modifiers::default()
    }
}

fn control() -> Modifiers {
    Modifiers {
        control: true,
        ..Modifiers::default()
    }
}

#[test]
fn kitty_text_key_with_associated_text() {
    let event = key(b"\x1b[97;1;97u");
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.modifiers, Modifiers::default());
    assert_eq!(event.kind, KeyEventKind::Press);
    assert_eq!(event.text.as_deref(), Some("a"));
}

#[test]
fn kitty_shifted_key_reports_alternate_and_text() {
    let event = key(b"\x1b[49:33;2;33u");
    assert_eq!(event.key, Key::Char('1'));
    assert_eq!(event.shifted_key, Some('!'));
    assert_eq!(event.modifiers, shift());
    assert_eq!(event.text.as_deref(), Some("!"));
}

#[test]
fn kitty_text_is_inferred_without_associated_text_flag() {
    assert_eq!(key(b"\x1b[97:65;2u").text.as_deref(), Some("A"));
    assert_eq!(key(b"\x1b[97u").text.as_deref(), Some("a"));
}

#[test]
fn kitty_control_key_has_no_text() {
    let event = key(b"\x1b[99;5u");
    assert_eq!(event.key, Key::Char('c'));
    assert_eq!(event.modifiers, control());
    assert_eq!(event.text, None);
}

#[test]
fn kitty_repeat_and_release_events() {
    assert_eq!(key(b"\x1b[97;1:2u").kind, KeyEventKind::Repeat);
    let release = key(b"\x1b[97;1:3u");
    assert_eq!(release.kind, KeyEventKind::Release);
    assert_eq!(release.text, None);
}

#[test]
fn kitty_functional_keys() {
    assert_eq!(key(b"\x1b[13u").key, Key::Enter);
    assert_eq!(key(b"\x1b[27u").key, Key::Escape);
    assert_eq!(key(b"\x1b[127u").key, Key::Backspace);
    assert_eq!(key(b"\x1b[9;2u").modifiers, shift());
    assert_eq!(key(b"\x1b[57441;2u").key, Key::Modifier(ModifierKey::Shift));
    assert_eq!(key(b"\x1b[57414u").key, Key::Enter);
}

#[test]
fn letter_and_tilde_keys_with_modifiers() {
    let up = key(b"\x1b[1;5A");
    assert_eq!(up.key, Key::Up);
    assert_eq!(up.modifiers, control());
    assert_eq!(key(b"\x1b[A").key, Key::Up);
    assert_eq!(key(b"\x1b[15~").key, Key::Function(5));
    assert_eq!(key(b"\x1b[24;1:3~").kind, KeyEventKind::Release);
    assert_eq!(key(b"\x1b[3~").key, Key::Delete);
    assert_eq!(key(b"\x1bOP").key, Key::Function(1));
    assert_eq!(key(b"\x1b[Z").modifiers, shift());
}

#[test]
fn legacy_bytes() {
    let event = key(b"A");
    assert_eq!(event.key, Key::Char('a'));
    assert_eq!(event.modifiers, shift());
    assert_eq!(event.text.as_deref(), Some("A"));

    assert_eq!(key(b"\r").key, Key::Enter);
    assert_eq!(key(b"\x7f").key, Key::Backspace);

    let control_c = key(b"\x03");
    assert_eq!(control_c.key, Key::Char('c'));
    assert_eq!(control_c.modifiers, control());

    let alt_x = key(b"\x1bx");
    assert_eq!(alt_x.key, Key::Char('x'));
    assert!(alt_x.modifiers.alt);
    assert_eq!(alt_x.text, None);
}

#[test]
fn legacy_utf8_text_split_across_reads() {
    let mut parser = Parser::new();
    assert!(parser.feed(&[0xc3]).is_empty());
    let events = parser.feed(&[0xa9]);
    let [TermEvent::Key(event)] = events.as_slice() else {
        panic!("expected one key event, got {events:?}");
    };
    assert_eq!(event.text.as_deref(), Some("é"));
}

#[test]
fn lone_escape_waits_for_flush() {
    let mut parser = Parser::new();
    assert!(parser.feed(b"\x1b").is_empty());
    assert!(parser.has_pending_escape());
    let Some(TermEvent::Key(event)) = parser.flush_pending_escape() else {
        panic!("expected the escape key");
    };
    assert_eq!(event.key, Key::Escape);
    assert!(!parser.has_pending_escape());
}

#[test]
fn sequences_split_across_reads() {
    let mut parser = Parser::new();
    assert!(parser.feed(b"\x1b[1;").is_empty());
    assert!(!parser.has_pending_escape());
    let events = parser.feed(b"5A");
    assert!(matches!(
        events.as_slice(),
        [TermEvent::Key(KeyEvent { key: Key::Up, .. })]
    ));
}

#[test]
fn sgr_mouse_press_release_and_motion() {
    let press = mouse(b"\x1b[<0;10;5M");
    assert_eq!(press.kind, MouseEventKind::Press(MouseButton::Left));
    assert_eq!((press.column, press.row), (9, 4));

    let release = mouse(b"\x1b[<2;10;5m");
    assert_eq!(release.kind, MouseEventKind::Release(MouseButton::Right));

    let drag = mouse(b"\x1b[<32;11;5M");
    assert_eq!(
        drag.kind,
        MouseEventKind::Move {
            pressed: Some(MouseButton::Left)
        }
    );

    let hover = mouse(b"\x1b[<35;11;5M");
    assert_eq!(hover.kind, MouseEventKind::Move { pressed: None });

    let control_click = mouse(b"\x1b[<16;1;1M");
    assert_eq!(control_click.modifiers, control());
}

#[test]
fn sgr_mouse_wheel_and_extra_buttons() {
    assert_eq!(
        mouse(b"\x1b[<64;1;1M").kind,
        MouseEventKind::Scroll(ScrollDirection::Up)
    );
    assert_eq!(
        mouse(b"\x1b[<65;1;1M").kind,
        MouseEventKind::Scroll(ScrollDirection::Down)
    );
    assert_eq!(
        mouse(b"\x1b[<128;1;1M").kind,
        MouseEventKind::Press(MouseButton::Back)
    );
}

#[test]
fn bracketed_paste_keeps_escape_bytes_verbatim() {
    let event = single(b"\x1b[200~hello\x1b[Aworld\x1b[201~");
    assert_eq!(event, TermEvent::Paste("hello\x1b[Aworld".to_owned()));
}

#[test]
fn focus_and_in_band_resize() {
    assert_eq!(single(b"\x1b[I"), TermEvent::Focus(true));
    assert_eq!(single(b"\x1b[O"), TermEvent::Focus(false));
    assert_eq!(
        single(b"\x1b[48;40;120;800;1200t"),
        TermEvent::Resize(GridSize {
            rows: 40,
            cols: 120,
            height_px: 800,
            width_px: 1200,
        })
    );
}

#[test]
fn query_replies() {
    assert_eq!(reply(b"\x1b[?62;22;52c"), Reply::PrimaryDeviceAttributes);
    assert_eq!(
        reply(b"\x1b[?2026;2$y"),
        Reply::Mode {
            mode: 2026,
            state: ModeState::Reset
        }
    );
    assert_eq!(reply(b"\x1b[?31u"), Reply::KeyboardFlags(31));
    assert_eq!(
        reply(b"\x1b[6;20;10t"),
        Reply::CellSize {
            width: 10,
            height: 20
        }
    );
    assert_eq!(
        reply(b"\x1b[?997;1n"),
        Reply::ColorScheme(ColorScheme::Dark)
    );
}

#[test]
fn background_color_reply_with_either_terminator() {
    let expected = Reply::Background(Rgb {
        red: 0x28,
        green: 0x2c,
        blue: 0x34,
    });
    assert_eq!(reply(b"\x1b]11;rgb:2828/2c2c/3434\x1b\\"), expected);
    assert_eq!(reply(b"\x1b]11;rgb:28/2c/34\x07"), expected);
}

#[test]
fn graphics_replies() {
    let ok = reply(b"\x1b_Gi=31;OK\x1b\\");
    assert_eq!(
        ok,
        Reply::Graphics(GraphicsReply {
            image_id: 31,
            message: "OK".to_owned()
        })
    );

    let Reply::Graphics(error) = reply(b"\x1b_Gi=32;ENOENT:no such file\x1b\\") else {
        panic!("expected a graphics reply");
    };
    assert_eq!(error.image_id, 32);
    assert!(!error.is_ok());
}

#[test]
fn replies_and_keys_interleaved() {
    let events = parse(b"\x1b_Gi=1;OK\x1b\\x\x1b[?62c");
    assert_eq!(events.len(), 3);
    assert!(matches!(
        events[1],
        TermEvent::Key(KeyEvent {
            key: Key::Char('x'),
            ..
        })
    ));
}

#[test]
fn background_luminance_picks_color_scheme() {
    let dark = Rgb {
        red: 30,
        green: 30,
        blue: 30,
    };
    let light = Rgb {
        red: 250,
        green: 250,
        blue: 240,
    };
    assert_eq!(dark.color_scheme(), ColorScheme::Dark);
    assert_eq!(light.color_scheme(), ColorScheme::Light);
}

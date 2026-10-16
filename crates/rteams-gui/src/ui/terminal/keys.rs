//! Maps egui input events to the byte sequences a terminal program expects (xterm style).

use eframe::egui::{Event, Key, Modifiers};

/// Result of translating one frame of events.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Translated {
    /// Bytes for the pty.
    pub bytes: Vec<u8>,
    /// A copy shortcut was pressed (Cmd+C, or Ctrl+C where egui reports it as a copy command).
    pub copy: bool,
}

fn modifier_param(m: Modifiers) -> u8 {
    1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8
}

fn csi(final_byte: char, m: Modifiers) -> Vec<u8> {
    if modifier_param(m) > 1 {
        format!("\x1b[1;{}{final_byte}", modifier_param(m)).into_bytes()
    } else {
        format!("\x1b[{final_byte}").into_bytes()
    }
}

fn tilde(n: u8, m: Modifiers) -> Vec<u8> {
    if modifier_param(m) > 1 {
        format!("\x1b[{n};{}~", modifier_param(m)).into_bytes()
    } else {
        format!("\x1b[{n}~").into_bytes()
    }
}

fn cursor_key(final_byte: char, m: Modifiers, app: bool) -> Vec<u8> {
    if app && modifier_param(m) == 1 {
        format!("\x1bO{final_byte}").into_bytes()
    } else {
        csi(final_byte, m)
    }
}

fn function_key(n: u8, m: Modifiers) -> Option<Vec<u8>> {
    let p = modifier_param(m);
    Some(match n {
        1..=4 => {
            let c = ['P', 'Q', 'R', 'S'][n as usize - 1];
            if p > 1 {
                format!("\x1b[1;{p}{c}").into_bytes()
            } else {
                format!("\x1bO{c}").into_bytes()
            }
        }
        5 => tilde(15, m),
        6 => tilde(17, m),
        7 => tilde(18, m),
        8 => tilde(19, m),
        9 => tilde(20, m),
        10 => tilde(21, m),
        11 => tilde(23, m),
        12 => tilde(24, m),
        _ => return None,
    })
}

fn alt_prefix(m: Modifiers, mut b: Vec<u8>) -> Vec<u8> {
    if m.alt {
        b.insert(0, 0x1b);
    }
    b
}

/// Bytes for a key press, or `None` when the key is covered by a `Text` event (plain printable
/// characters) or has no terminal meaning.
pub fn encode_key(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    Some(match key {
        Key::Enter => alt_prefix(m, b"\r".to_vec()),
        Key::Tab => {
            if m.shift {
                b"\x1b[Z".to_vec()
            } else {
                alt_prefix(m, b"\t".to_vec())
            }
        }
        Key::Escape => b"\x1b".to_vec(),
        Key::Backspace => {
            if m.ctrl {
                alt_prefix(m, vec![0x08])
            } else {
                alt_prefix(m, vec![0x7f])
            }
        }
        Key::ArrowUp => cursor_key('A', m, app_cursor),
        Key::ArrowDown => cursor_key('B', m, app_cursor),
        Key::ArrowRight => cursor_key('C', m, app_cursor),
        Key::ArrowLeft => cursor_key('D', m, app_cursor),
        Key::Home => cursor_key('H', m, app_cursor),
        Key::End => cursor_key('F', m, app_cursor),
        Key::Insert => tilde(2, m),
        Key::Delete => tilde(3, m),
        Key::PageUp => tilde(5, m),
        Key::PageDown => tilde(6, m),
        Key::F1 => function_key(1, m)?,
        Key::F2 => function_key(2, m)?,
        Key::F3 => function_key(3, m)?,
        Key::F4 => function_key(4, m)?,
        Key::F5 => function_key(5, m)?,
        Key::F6 => function_key(6, m)?,
        Key::F7 => function_key(7, m)?,
        Key::F8 => function_key(8, m)?,
        Key::F9 => function_key(9, m)?,
        Key::F10 => function_key(10, m)?,
        Key::F11 => function_key(11, m)?,
        Key::F12 => function_key(12, m)?,
        Key::Space if m.ctrl => alt_prefix(m, vec![0]),
        _ if m.ctrl && !m.mac_cmd => {
            let b = ctrl_byte(key)?;
            alt_prefix(m, vec![b])
        }
        _ if m.alt && !m.ctrl => {
            let name = key.name();
            let mut chars = name.chars();
            let c = chars.next()?;
            if chars.next().is_some() {
                return None;
            }
            let c = if m.shift { c } else { c.to_ascii_lowercase() };
            let mut b = vec![0x1b];
            let mut buf = [0u8; 4];
            b.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            b
        }
        _ => return None,
    })
}

fn ctrl_byte(key: Key) -> Option<u8> {
    let name = key.name();
    let mut it = name.chars();
    let c = it.next()?;
    if it.next().is_none() && c.is_ascii_alphabetic() {
        return Some(c.to_ascii_uppercase() as u8 - b'A' + 1);
    }
    Some(match key {
        Key::OpenBracket => 0x1b,
        Key::Backslash => 0x1c,
        Key::CloseBracket => 0x1d,
        Key::Slash => 0x1f,
        Key::Minus => 0x1f,
        Key::Num2 => 0,
        _ => return None,
    })
}

/// Wrap pasted text for the pty. `bracketed` follows the program's mode 2004.
pub fn encode_paste(text: &str, bracketed: bool) -> Vec<u8> {
    let text = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        let mut v = b"\x1b[200~".to_vec();
        v.extend_from_slice(text.as_bytes());
        v.extend_from_slice(b"\x1b[201~");
        v
    } else {
        text.into_bytes()
    }
}

/// Translate a frame's events.
pub fn translate(events: &[Event], app_cursor: bool, bracketed: bool) -> Translated {
    let mut out = Translated::default();
    let mut suppress_text = false;
    for ev in events {
        match ev {
            Event::Key {
                key,
                pressed: true,
                modifiers,
                ..
            } => {
                if modifiers.ctrl && modifiers.shift && *key == Key::C {
                    out.copy = true;
                    continue;
                }
                if let Some(b) = encode_key(*key, *modifiers, app_cursor) {
                    // Alt/Ctrl combos also arrive as Text on some platforms; the key wins.
                    suppress_text = modifiers.alt || modifiers.ctrl;
                    out.bytes.extend(b);
                }
            }
            Event::Text(t) => {
                if suppress_text {
                    suppress_text = false;
                    continue;
                }
                out.bytes.extend_from_slice(t.as_bytes());
            }
            Event::Paste(t) => out.bytes.extend(encode_paste(t, bracketed)),
            Event::Copy => out.copy = true,
            _ => {}
        }
    }
    out
}

/// Cursor-key sequences for the mouse wheel in alt-screen programs.
pub fn wheel_arrows(up: bool, lines: usize, app_cursor: bool) -> Vec<u8> {
    let one = cursor_key(if up { 'A' } else { 'B' }, Modifiers::NONE, app_cursor);
    one.repeat(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NONE: Modifiers = Modifiers::NONE;
    const CTRL: Modifiers = Modifiers {
        ctrl: true,
        ..Modifiers::NONE
    };
    const ALT: Modifiers = Modifiers {
        alt: true,
        ..Modifiers::NONE
    };
    const SHIFT: Modifiers = Modifiers {
        shift: true,
        ..Modifiers::NONE
    };

    fn k(key: Key, m: Modifiers) -> Option<Vec<u8>> {
        encode_key(key, m, false)
    }

    #[test]
    fn mapping_table() {
        let table: Vec<(Key, Modifiers, bool, Option<&[u8]>)> = vec![
            (Key::Enter, NONE, false, Some(b"\r")),
            (Key::Tab, NONE, false, Some(b"\t")),
            (Key::Tab, SHIFT, false, Some(b"\x1b[Z")),
            (Key::Escape, NONE, false, Some(b"\x1b")),
            (Key::Backspace, NONE, false, Some(b"\x7f")),
            (Key::ArrowUp, NONE, false, Some(b"\x1b[A")),
            (Key::ArrowDown, NONE, false, Some(b"\x1b[B")),
            (Key::ArrowRight, NONE, false, Some(b"\x1b[C")),
            (Key::ArrowLeft, NONE, false, Some(b"\x1b[D")),
            (Key::ArrowUp, NONE, true, Some(b"\x1bOA")),
            (Key::ArrowLeft, CTRL, false, Some(b"\x1b[1;5D")),
            (Key::ArrowRight, ALT, false, Some(b"\x1b[1;3C")),
            (Key::Home, NONE, false, Some(b"\x1b[H")),
            (Key::End, NONE, false, Some(b"\x1b[F")),
            (Key::Delete, NONE, false, Some(b"\x1b[3~")),
            (Key::PageUp, NONE, false, Some(b"\x1b[5~")),
            (Key::PageDown, NONE, false, Some(b"\x1b[6~")),
            (Key::F1, NONE, false, Some(b"\x1bOP")),
            (Key::F4, NONE, false, Some(b"\x1bOS")),
            (Key::F5, NONE, false, Some(b"\x1b[15~")),
            (Key::F12, NONE, false, Some(b"\x1b[24~")),
            (Key::F5, SHIFT, false, Some(b"\x1b[15;2~")),
            (Key::C, CTRL, false, Some(&[3])),
            (Key::A, CTRL, false, Some(&[1])),
            (Key::Z, CTRL, false, Some(&[26])),
            (Key::D, CTRL, false, Some(&[4])),
            (Key::Space, CTRL, false, Some(&[0])),
            (Key::OpenBracket, CTRL, false, Some(&[27])),
            (Key::F, ALT, false, Some(b"\x1bf")),
            (Key::Enter, ALT, false, Some(b"\x1b\r")),
            (Key::Backspace, ALT, false, Some(b"\x1b\x7f")),
            (Key::A, NONE, false, None),
            (Key::Num1, NONE, false, None),
            (Key::Space, NONE, false, None),
        ];
        for (key, m, app, want) in table {
            assert_eq!(encode_key(key, m, app).as_deref(), want, "{key:?} {m:?}");
        }
        assert!(k(Key::F5, NONE).is_some());
    }

    #[test]
    fn paste_modes() {
        assert_eq!(encode_paste("a\nb", false), b"a\rb");
        assert_eq!(encode_paste("x", true), b"\x1b[200~x\x1b[201~");
    }

    #[test]
    fn translate_events() {
        let evs = vec![
            Event::Text("ls".into()),
            Event::Key {
                key: Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: NONE,
            },
            Event::Key {
                key: Key::C,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: CTRL,
            },
            Event::Text("c".into()),
            Event::Paste("p".into()),
            Event::Copy,
        ];
        let t = translate(&evs, false, true);
        assert_eq!(t.bytes, b"ls\r\x03\x1b[200~p\x1b[201~");
        assert!(t.copy);
    }

    #[test]
    fn wheel() {
        assert_eq!(wheel_arrows(true, 2, false), b"\x1b[A\x1b[A");
    }
}

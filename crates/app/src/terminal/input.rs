//! Keyboard → bytes for the PTY (same sequences as xterm.js: `Keyboard.ts`).

use winit::keyboard::{Key, KeyCode, NamedKey, PhysicalKey};

/// Modifier state relevant for encoding.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    /// xterm modifier parameter (`1 + shift + 2·alt + 4·ctrl + 8·meta`); 0 if none.
    fn param(self) -> u8 {
        let m = (self.shift as u8) | ((self.alt as u8) << 1) | ((self.ctrl as u8) << 2) | ((self.meta as u8) << 3);
        if m == 0 {
            0
        } else {
            m + 1
        }
    }
}

/// Encodes a pressed key. `text` is the text produced by the layout (already with Shift/AltGr).
/// `app_cursor` = DECCKM mode (`ESC O x` arrows). Returns `None` when the key generates no bytes.
/// (The app uses [`encode_key_physical`]; this form is kept for tests.)
#[cfg(test)]
pub fn encode_key(key: &Key, text: Option<&str>, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    encode_key_physical(key, None, text, mods, app_cursor)
}

/// Like [`encode_key`], with the physical key (`KeyEvent::physical_key`): Ctrl+letter in a non-Latin
/// layout (e.g. Ctrl+C in Cyrillic, where the logical key is "с") uses the letter's physical position.
pub fn encode_key_physical(key: &Key, physical: Option<&PhysicalKey>, text: Option<&str>, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
    let m = mods.param();
    let csi_letter = |letter: char, ss3_when_plain: bool| -> Vec<u8> {
        if m > 0 {
            format!("\x1b[1;{m}{letter}").into_bytes()
        } else if ss3_when_plain {
            format!("\x1bO{letter}").into_bytes()
        } else {
            format!("\x1b[{letter}").into_bytes()
        }
    };
    let csi_tilde = |n: u8| -> Vec<u8> {
        if m > 0 {
            format!("\x1b[{n};{m}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let alt_prefix = |mut v: Vec<u8>| -> Vec<u8> {
        if mods.alt {
            v.insert(0, 0x1b);
        }
        v
    };

    if let Key::Named(named) = key {
        let bytes = match named {
            NamedKey::Enter => alt_prefix(vec![b'\r']),
            NamedKey::Backspace => {
                if mods.ctrl {
                    vec![0x08]
                } else {
                    alt_prefix(vec![0x7f])
                }
            }
            NamedKey::Tab => {
                if mods.shift {
                    b"\x1b[Z".to_vec()
                } else {
                    alt_prefix(vec![b'\t'])
                }
            }
            NamedKey::Escape => alt_prefix(vec![0x1b]),
            NamedKey::Space => {
                if mods.ctrl {
                    vec![0]
                } else {
                    alt_prefix(vec![b' '])
                }
            }
            NamedKey::ArrowUp => csi_letter('A', app_cursor),
            NamedKey::ArrowDown => csi_letter('B', app_cursor),
            NamedKey::ArrowRight => csi_letter('C', app_cursor),
            NamedKey::ArrowLeft => csi_letter('D', app_cursor),
            NamedKey::Home => csi_letter('H', app_cursor),
            NamedKey::End => csi_letter('F', app_cursor),
            NamedKey::Insert => csi_tilde(2),
            NamedKey::Delete => csi_tilde(3),
            NamedKey::PageUp => csi_tilde(5),
            NamedKey::PageDown => csi_tilde(6),
            NamedKey::F1 => csi_letter('P', true),
            NamedKey::F2 => csi_letter('Q', true),
            NamedKey::F3 => csi_letter('R', true),
            NamedKey::F4 => csi_letter('S', true),
            NamedKey::F5 => csi_tilde(15),
            NamedKey::F6 => csi_tilde(17),
            NamedKey::F7 => csi_tilde(18),
            NamedKey::F8 => csi_tilde(19),
            NamedKey::F9 => csi_tilde(20),
            NamedKey::F10 => csi_tilde(21),
            NamedKey::F11 => csi_tilde(23),
            NamedKey::F12 => csi_tilde(24),
            _ => return None,
        };
        return Some(bytes);
    }

    // Character keys.
    let ch_str = match key {
        Key::Character(s) => s.as_str(),
        _ => text.unwrap_or(""),
    };
    let produced = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));

    // AltGr (Ctrl+Alt on Windows) producing a symbol: sends text as is.
    if mods.ctrl && mods.alt {
        if let Some(t) = produced {
            if !t.chars().all(|c| c.is_ascii_alphabetic()) {
                return Some(t.as_bytes().to_vec());
            }
        }
    }

    if mods.ctrl {
        let mut chars = ch_str.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            if let Some(code) = ctrl_code(c) {
                return Some(alt_prefix(vec![code]));
            }
            // Letter from another alphabet: physical position takes precedence (letters only, as in xterm).
            if !c.is_ascii() {
                if let Some(code) = physical.and_then(physical_ctrl_code) {
                    return Some(alt_prefix(vec![code]));
                }
            }
        }
        // Ctrl + key without control code: sends the text (if any).
        return produced.map(|t| t.as_bytes().to_vec());
    }

    let t = produced.or(if ch_str.is_empty() { None } else { Some(ch_str) })?;
    if t.chars().any(char::is_control) {
        return None;
    }
    Some(alt_prefix(t.as_bytes().to_vec()))
}

/// Control code for Ctrl+key (like xterm).
fn ctrl_code(c: char) -> Option<u8> {
    let c = c.to_ascii_lowercase();
    match c {
        'a'..='z' => Some(c as u8 - b'a' + 1),
        '@' | '2' | ' ' => Some(0),
        '[' | '3' => Some(0x1b),
        '\\' | '4' => Some(0x1c),
        ']' | '5' => Some(0x1d),
        '^' | '6' => Some(0x1e),
        '_' | '-' | '7' | '/' => Some(0x1f),
        '8' | '?' => Some(0x7f),
        _ => None,
    }
}

/// Control code for Ctrl+letter via physical key (`KeyA`…`KeyZ`).
fn physical_ctrl_code(physical: &PhysicalKey) -> Option<u8> {
    use KeyCode::*;
    const LETTERS: [KeyCode; 26] = [
        KeyA, KeyB, KeyC, KeyD, KeyE, KeyF, KeyG, KeyH, KeyI, KeyJ, KeyK, KeyL, KeyM, KeyN, KeyO, KeyP, KeyQ, KeyR, KeyS, KeyT, KeyU, KeyV,
        KeyW, KeyX, KeyY, KeyZ,
    ];
    let PhysicalKey::Code(code) = physical else { return None };
    LETTERS.iter().position(|k| k == code).map(|i| i as u8 + 1)
}

/// Prepares pasted text: `\r\n`/`\n` → `\r` (like xterm.js) and wraps in
/// `ESC[200~ … ESC[201~` when the program activated *bracketed paste*.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Without ESC and C1 controls (U+009B = CSI) no marker survives, not even nested
        // (`ESC[20ESC[201~1~`): text does not escape bracketed paste mode.
        let safe: String = normalized.chars().filter(|&c| c != '\x1b' && !('\u{80}'..='\u{9f}').contains(&c)).collect();
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

/// Text delivered to a tab via bridge (message from another agent): always as bracketed paste.
/// ESC and other control characters are stripped so the text cannot close the paste
/// (`ESC[201~`) and turn into keystrokes in the tab's program.
pub fn submit_paste(text: &str) -> Vec<u8> {
    let safe = holt_core::util::terminal_text(text);
    format!("\x1b[200~{safe}\x1b[201~").into_bytes()
}

/// Base button code for mouse reporting (xterm): 0 left, 1 middle, 2 right.
pub const MOUSE_LEFT: u8 = 0;
pub const MOUSE_MIDDLE: u8 = 1;
pub const MOUSE_RIGHT: u8 = 2;
/// Motion bit (drag or move) added to button code.
pub const MOUSE_MOTION: u8 = 32;
/// Motion without any pressed button (`3 + 32`).
pub const MOUSE_NO_BUTTON: u8 = 3;
/// Mouse wheel up and down (adds modifiers like buttons).
pub const MOUSE_WHEEL_UP: u8 = 64;
pub const MOUSE_WHEEL_DOWN: u8 = 65;

/// Mouse report (click, release, motion, wheel) for the PTY.
///
/// `code` is the xterm button code already added to modifiers (`alt` 8, `ctrl` 16) and motion
/// bit (32), or 64/65 for wheel. `col`/`row` are 1-based. In SGR mode (1006) release is
/// `m`; in legacy format (X10) release becomes button 3 and coordinates only go up to 223.
pub fn mouse_report(sgr: bool, code: u8, col: usize, row: usize, release: bool) -> Option<Vec<u8>> {
    if sgr {
        let end = if release { 'm' } else { 'M' };
        return Some(format!("\x1b[<{code};{col};{row}{end}").into_bytes());
    }
    if col >= 224 || row >= 224 {
        return None;
    }
    let code = if release { (code & !0b11) | 3 } else { code };
    Some(vec![0x1b, b'[', b'M', 32 + code, 32 + col as u8, 32 + row as u8])
}

/// Adds mouse report modifiers (`shift` is excluded: it returns mouse handling to the app).
pub fn mouse_mods(alt: bool, ctrl: bool) -> u8 {
    (if alt { 8 } else { 0 }) | (if ctrl { 16 } else { 0 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use winit::keyboard::SmolStr;

    fn ch(s: &str) -> Key {
        Key::Character(SmolStr::new(s))
    }
    const NONE: Mods = Mods { shift: false, ctrl: false, alt: false, meta: false };
    const CTRL: Mods = Mods { shift: false, ctrl: true, alt: false, meta: false };
    const ALT: Mods = Mods { shift: false, ctrl: false, alt: true, meta: false };
    const SHIFT: Mods = Mods { shift: true, ctrl: false, alt: false, meta: false };

    fn enc(key: Key, text: Option<&str>, mods: Mods, app: bool) -> Vec<u8> {
        encode_key(&key, text, mods, app).unwrap_or_default()
    }

    #[test]
    fn plain_text_and_enter() {
        assert_eq!(enc(ch("a"), Some("a"), NONE, false), b"a");
        assert_eq!(enc(ch("A"), Some("A"), SHIFT, false), b"A");
        assert_eq!(enc(ch("ç"), Some("ç"), NONE, false), "ç".as_bytes());
        assert_eq!(enc(Key::Named(NamedKey::Enter), Some("\r"), NONE, false), b"\r");
        assert_eq!(enc(Key::Named(NamedKey::Backspace), None, NONE, false), b"\x7f");
        assert_eq!(enc(Key::Named(NamedKey::Tab), None, NONE, false), b"\t");
        assert_eq!(enc(Key::Named(NamedKey::Tab), None, SHIFT, false), b"\x1b[Z");
        assert_eq!(enc(Key::Named(NamedKey::Escape), None, NONE, false), b"\x1b");
        assert_eq!(enc(Key::Named(NamedKey::Space), Some(" "), NONE, false), b" ");
    }

    #[test]
    fn arrows_honor_app_cursor_and_modifiers() {
        assert_eq!(enc(Key::Named(NamedKey::ArrowUp), None, NONE, false), b"\x1b[A");
        assert_eq!(enc(Key::Named(NamedKey::ArrowUp), None, NONE, true), b"\x1bOA");
        assert_eq!(enc(Key::Named(NamedKey::ArrowLeft), None, CTRL, true), b"\x1b[1;5D");
        assert_eq!(enc(Key::Named(NamedKey::Home), None, NONE, false), b"\x1b[H");
        assert_eq!(enc(Key::Named(NamedKey::End), None, NONE, true), b"\x1bOF");
        assert_eq!(enc(Key::Named(NamedKey::Delete), None, NONE, false), b"\x1b[3~");
        assert_eq!(enc(Key::Named(NamedKey::PageUp), None, SHIFT, false), b"\x1b[5;2~");
        assert_eq!(enc(Key::Named(NamedKey::Insert), None, NONE, false), b"\x1b[2~");
    }

    #[test]
    fn function_keys() {
        assert_eq!(enc(Key::Named(NamedKey::F1), None, NONE, false), b"\x1bOP");
        assert_eq!(enc(Key::Named(NamedKey::F4), None, NONE, false), b"\x1bOS");
        assert_eq!(enc(Key::Named(NamedKey::F5), None, NONE, false), b"\x1b[15~");
        assert_eq!(enc(Key::Named(NamedKey::F12), None, CTRL, false), b"\x1b[24;5~");
        assert_eq!(enc(Key::Named(NamedKey::F2), None, SHIFT, false), b"\x1b[1;2Q");
    }

    #[test]
    fn control_codes_and_alt() {
        assert_eq!(enc(ch("c"), None, CTRL, false), vec![3]);
        assert_eq!(enc(ch("C"), None, Mods { shift: true, ..CTRL }, false), vec![3]);
        assert_eq!(enc(ch("a"), None, CTRL, false), vec![1]);
        assert_eq!(enc(ch("z"), None, CTRL, false), vec![26]);
        assert_eq!(enc(ch("["), None, CTRL, false), vec![0x1b]);
        assert_eq!(enc(Key::Named(NamedKey::Space), None, CTRL, false), vec![0]);
        assert_eq!(enc(Key::Named(NamedKey::Backspace), None, CTRL, false), vec![8]);
        assert_eq!(enc(ch("b"), Some("b"), ALT, false), b"\x1bb");
        assert_eq!(enc(Key::Named(NamedKey::Enter), None, ALT, false), b"\x1b\r");
        assert_eq!(enc(ch("c"), None, Mods { alt: true, ..CTRL }, false), vec![0x1b, 3]);
    }

    #[test]
    fn ctrl_letters_fall_back_to_the_physical_key() {
        let c = PhysicalKey::Code(KeyCode::KeyC);
        // Cyrillic layout: logical key is "с", but Ctrl+C is still ^C.
        assert_eq!(encode_key_physical(&ch("с"), Some(&c), Some("с"), CTRL, false), Some(vec![3]));
        assert_eq!(encode_key_physical(&ch("с"), Some(&c), None, Mods { alt: true, ..CTRL }, false), Some(vec![0x1b, 3]));
        // Logical key takes precedence first (Dvorak: physical KeyI is "c").
        assert_eq!(encode_key_physical(&ch("c"), Some(&PhysicalKey::Code(KeyCode::KeyI)), None, CTRL, false), Some(vec![3]));
        // Outside letters nothing changes; without Ctrl, layout text.
        let semicolon = PhysicalKey::Code(KeyCode::Semicolon);
        assert_eq!(encode_key_physical(&ch("ж"), Some(&semicolon), Some("ж"), CTRL, false), Some("ж".as_bytes().to_vec()));
        assert_eq!(encode_key_physical(&ch("с"), Some(&c), Some("с"), NONE, false), Some("с".as_bytes().to_vec()));
    }

    #[test]
    fn altgr_symbols_pass_through() {
        let altgr = Mods { ctrl: true, alt: true, ..NONE };
        assert_eq!(enc(ch("q"), Some("/"), altgr, false), b"/");
        assert_eq!(enc(ch("2"), Some("@"), altgr, false), b"@");
    }

    #[test]
    fn unknown_named_keys_produce_nothing() {
        assert!(encode_key(&Key::Named(NamedKey::Shift), None, SHIFT, false).is_none());
        assert!(encode_key(&Key::Named(NamedKey::CapsLock), None, NONE, false).is_none());
    }

    #[test]
    fn mouse_reports_sgr_and_legacy() {
        assert_eq!(mouse_report(true, MOUSE_LEFT, 5, 3, false).unwrap(), b"\x1b[<0;5;3M");
        assert_eq!(mouse_report(true, MOUSE_LEFT, 5, 3, true).unwrap(), b"\x1b[<0;5;3m");
        assert_eq!(mouse_report(true, MOUSE_RIGHT + mouse_mods(false, true), 1, 1, false).unwrap(), b"\x1b[<18;1;1M");
        assert_eq!(mouse_report(true, MOUSE_LEFT + MOUSE_MOTION, 10, 2, false).unwrap(), b"\x1b[<32;10;2M");
        assert_eq!(mouse_report(true, 64, 7, 7, false).unwrap(), b"\x1b[<64;7;7M");
        // Legacy: release becomes button 3, coordinates add 32.
        assert_eq!(mouse_report(false, MOUSE_LEFT, 5, 3, false).unwrap(), vec![0x1b, b'[', b'M', 32, 37, 35]);
        assert_eq!(mouse_report(false, MOUSE_RIGHT, 5, 3, true).unwrap(), vec![0x1b, b'[', b'M', 35, 37, 35]);
        assert_eq!(mouse_report(false, MOUSE_NO_BUTTON + MOUSE_MOTION, 1, 1, false).unwrap(), vec![0x1b, b'[', b'M', 67, 33, 33]);
        assert!(mouse_report(false, MOUSE_LEFT, 224, 1, false).is_none());
        assert!(mouse_report(true, MOUSE_LEFT, 300, 400, false).is_some());
        assert_eq!(mouse_mods(true, true), 24);
    }

    #[test]
    fn paste_modes() {
        assert_eq!(paste_bytes("a\nb\r\nc", false), b"a\rb\rc");
        assert_eq!(paste_bytes("x\ny", true), b"\x1b[200~x\ry\x1b[201~");
        assert_eq!(paste_bytes("\x1b[201~rm", true), b"\x1b[200~[201~rm\x1b[201~");
        // Nested marker: removing only markers would form another `ESC[201~`.
        assert_eq!(paste_bytes("\x1b[20\x1b[201~1~curl x|sh\r", true), b"\x1b[200~[20[201~1~curl x|sh\r\x1b[201~");
        // 8-bit CSI (U+009B) is also stripped; tab and newlines remain.
        assert_eq!(paste_bytes("a\u{9b}201~b\tc\n", true), b"\x1b[200~a201~b\tc\r\x1b[201~");
        // Without bracketed paste, text goes as is.
        assert_eq!(paste_bytes("\x1b[A", false), b"\x1b[A");
        assert_eq!(submit_paste("hello @a.md"), b"\x1b[200~hello @a.md\x1b[201~");
        // A message from another agent does not close bracketed paste to turn into keystrokes (e.g. `!cmd` + Enter).
        assert_eq!(submit_paste("x\x1b[201~!rm -rf ~\r\x1b[200~"), b"\x1b[200~x[201~!rm -rf ~\n[200~\x1b[201~");
        assert_eq!(submit_paste("a\u{9b}2J\x03b"), b"\x1b[200~a2Jb\x1b[201~");
    }
}

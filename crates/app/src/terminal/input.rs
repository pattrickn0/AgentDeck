//! Teclado → bytes para o PTY (mesmas sequências do xterm.js: `Keyboard.ts`).

use winit::keyboard::{Key, NamedKey};

/// Estado dos modificadores relevante para a codificação.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Mods {
    pub shift: bool,
    pub ctrl: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Mods {
    /// Parâmetro de modificador do xterm (`1 + shift + 2·alt + 4·ctrl + 8·meta`); 0 se nenhum.
    fn param(self) -> u8 {
        let m = (self.shift as u8) | ((self.alt as u8) << 1) | ((self.ctrl as u8) << 2) | ((self.meta as u8) << 3);
        if m == 0 {
            0
        } else {
            m + 1
        }
    }
}

/// Codifica uma tecla pressionada. `text` é o texto produzido pelo layout (já com Shift/AltGr).
/// `app_cursor` = modo DECCKM (setas `ESC O x`). Retorna `None` quando a tecla não gera bytes.
pub fn encode_key(key: &Key, text: Option<&str>, mods: Mods, app_cursor: bool) -> Option<Vec<u8>> {
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

    // Teclas de caractere.
    let ch_str = match key {
        Key::Character(s) => s.as_str(),
        _ => text.unwrap_or(""),
    };
    let produced = text.filter(|t| !t.is_empty() && !t.chars().any(char::is_control));

    // AltGr (Ctrl+Alt no Windows) produzindo um símbolo: envia o texto como está.
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
        }
        // Ctrl + tecla sem código de controle: envia o texto (se houver).
        return produced.map(|t| t.as_bytes().to_vec());
    }

    let t = produced.or(if ch_str.is_empty() { None } else { Some(ch_str) })?;
    if t.chars().any(char::is_control) {
        return None;
    }
    Some(alt_prefix(t.as_bytes().to_vec()))
}

/// Código de controle de Ctrl+tecla (como o xterm).
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

/// Prepara texto colado: `\r\n`/`\n` → `\r` (como o xterm.js) e envolve em
/// `ESC[200~ … ESC[201~` quando o programa ativou o *bracketed paste*.
pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    let normalized = text.replace("\r\n", "\r").replace('\n', "\r");
    if bracketed {
        // Remove marcadores embutidos para que o texto não escape do modo colagem.
        let safe = normalized.replace("\x1b[200~", "").replace("\x1b[201~", "");
        format!("\x1b[200~{safe}\x1b[201~").into_bytes()
    } else {
        normalized.into_bytes()
    }
}

/// Texto enviado pelo compositor (`pty:submit`) ou por outro agente: sempre como colagem entre
/// colchetes. ESC e demais caracteres de controle são removidos, para o texto não poder fechar a
/// colagem (`ESC[201~`) e virar teclas no programa da aba.
pub fn submit_paste(text: &str) -> Vec<u8> {
    let safe = commanddeck_core::util::terminal_text(text);
    format!("\x1b[200~{safe}\x1b[201~").into_bytes()
}

/// Código base do botão para relatório de mouse (xterm): 0 esquerdo, 1 meio, 2 direito.
pub const MOUSE_LEFT: u8 = 0;
pub const MOUSE_MIDDLE: u8 = 1;
pub const MOUSE_RIGHT: u8 = 2;
/// Bit de movimento (arrastar ou mover) somado ao código do botão.
pub const MOUSE_MOTION: u8 = 32;
/// Movimento sem nenhum botão pressionado (`3 + 32`).
pub const MOUSE_NO_BUTTON: u8 = 3;

/// Relatório de mouse (clique, soltar, movimento, roda) para o PTY.
///
/// `code` é o código do botão do xterm já somado aos modificadores (`alt` 8, `ctrl` 16) e ao bit
/// de movimento (32), ou 64/65 para a roda. `col`/`row` começam em 1. No modo SGR (1006) soltar é
/// `m`; no formato legado (X10) soltar vira o botão 3 e as coordenadas só vão até 223.
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

/// Soma os modificadores do relatório de mouse (`shift` fica de fora: ele devolve o mouse ao app).
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
        // Legado: soltar vira botão 3, coordenadas somam 32.
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
        assert_eq!(paste_bytes("\x1b[201~rm", true), b"\x1b[200~rm\x1b[201~");
        assert_eq!(submit_paste("oi @a.md"), b"\x1b[200~oi @a.md\x1b[201~");
        // Uma mensagem de outro agente não fecha a colagem para virar teclas (ex.: `!cmd` + Enter).
        assert_eq!(submit_paste("x\x1b[201~!rm -rf ~\r\x1b[200~"), b"\x1b[200~x[201~!rm -rf ~\n[200~\x1b[201~");
        assert_eq!(submit_paste("a\u{9b}2J\x03b"), b"\x1b[200~a2Jb\x1b[201~");
    }
}

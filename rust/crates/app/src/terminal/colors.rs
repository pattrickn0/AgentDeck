//! Paleta do terminal: tema do xterm.js usado na versão Electron (fundo/fonte/cursor/seleção
//! customizados) + paleta ANSI padrão do xterm.js (Tango) e cubo 256 cores.

use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::vte::ansi::{Color, NamedColor, Rgb};
use egui::Color32;

use crate::theme;

/// As 16 cores ANSI padrão do xterm.js.
pub const ANSI16: [u32; 16] = [
    0x2e3436, 0xcc0000, 0x4e9a06, 0xc4a000, 0x3465a4, 0x75507b, 0x06989a, 0xd3d7cf, // normais
    0x555753, 0xef2929, 0x8ae234, 0xfce94f, 0x729fcf, 0xad7fa8, 0x34e2e2, 0xeeeeec, // brilhantes
];

/// Cor RGB do índice 0..=255 (16 ANSI + cubo 6×6×6 + rampa de cinza).
pub fn indexed_rgb(i: u8) -> Rgb {
    let hex = |h: u32| Rgb { r: (h >> 16) as u8, g: (h >> 8) as u8, b: h as u8 };
    match i {
        0..=15 => hex(ANSI16[i as usize]),
        16..=231 => {
            let i = i - 16;
            let lvl = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Rgb { r: lvl(i / 36), g: lvl((i / 6) % 6), b: lvl(i % 6) }
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Rgb { r: v, g: v, b: v }
        }
    }
}

fn c32(rgb: Rgb) -> Color32 {
    Color32::from_rgb(rgb.r, rgb.g, rgb.b)
}

fn rgb_of(c: Color32) -> Rgb {
    Rgb { r: c.r(), g: c.g(), b: c.b() }
}

/// Cor padrão (sem override OSC) do índice usado por `ColorRequest`.
pub fn default_for_index(index: usize) -> Rgb {
    match index {
        0..=255 => indexed_rgb(index as u8),
        i if i == NamedColor::Background as usize => rgb_of(theme::TERM_BG),
        i if i == NamedColor::Cursor as usize => rgb_of(theme::TERM_CURSOR),
        _ => rgb_of(theme::TERM_FG),
    }
}

/// Resolve uma cor de célula considerando overrides OSC 4/10/11 feitos pelo programa.
pub fn resolve(color: Color, overrides: &Colors, bold: bool, is_fg: bool) -> Color32 {
    match color {
        Color::Spec(rgb) => c32(rgb),
        Color::Indexed(i) => {
            // xterm.js: texto em negrito usa a variante brilhante das 8 cores básicas.
            let i = if bold && is_fg && i < 8 { i + 8 } else { i };
            c32(overrides[i as usize].unwrap_or_else(|| indexed_rgb(i)))
        }
        Color::Named(n) => named(n, overrides, bold && is_fg),
    }
}

fn named(n: NamedColor, overrides: &Colors, bright_bold: bool) -> Color32 {
    use NamedColor::*;
    let idx = n as usize;
    if let Some(rgb) = overrides[idx] {
        return c32(rgb);
    }
    match n {
        Foreground | BrightForeground => theme::TERM_FG,
        Background => theme::TERM_BG,
        Cursor => theme::TERM_CURSOR,
        DimForeground => theme::with_opacity(theme::TERM_FG, 0.5),
        DimBlack | DimRed | DimGreen | DimYellow | DimBlue | DimMagenta | DimCyan | DimWhite => {
            let base = idx - DimBlack as usize;
            theme::with_opacity(c32(indexed_rgb(base as u8)), 0.5)
        }
        _ if idx < 8 && bright_bold => c32(overrides[idx + 8].unwrap_or_else(|| indexed_rgb(idx as u8 + 8))),
        _ if idx < 16 => c32(indexed_rgb(idx as u8)),
        _ => theme::TERM_FG,
    }
}

/// Cores finais (frente, fundo) de uma célula, aplicando inverse/dim/hidden.
/// O fundo `None` significa "fundo padrão" (não precisa pintar).
pub fn cell_colors(fg: Color, bg: Color, flags: Flags, overrides: &Colors) -> (Color32, Option<Color32>) {
    let bold = flags.contains(Flags::BOLD);
    let mut f = resolve(fg, overrides, bold, true);
    let default_bg = matches!(bg, Color::Named(NamedColor::Background)) && overrides[NamedColor::Background as usize].is_none();
    let mut b = if default_bg { None } else { Some(resolve(bg, overrides, false, false)) };
    if flags.contains(Flags::INVERSE) {
        let new_bg = f;
        f = b.unwrap_or(theme::TERM_BG);
        b = Some(new_bg);
    }
    if flags.contains(Flags::DIM) {
        f = theme::with_opacity(f, 0.5);
    }
    if flags.contains(Flags::HIDDEN) {
        f = Color32::TRANSPARENT;
    }
    (f, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cube_and_gray() {
        assert_eq!(indexed_rgb(16), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(indexed_rgb(231), Rgb { r: 255, g: 255, b: 255 });
        assert_eq!(indexed_rgb(196), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(indexed_rgb(232), Rgb { r: 8, g: 8, b: 8 });
        assert_eq!(indexed_rgb(255), Rgb { r: 238, g: 238, b: 238 });
        assert_eq!(indexed_rgb(1), Rgb { r: 0xcc, g: 0, b: 0 });
    }

    #[test]
    fn inverse_and_bold_bright() {
        let o = Colors::default();
        let (f, b) = cell_colors(Color::Named(NamedColor::Foreground), Color::Named(NamedColor::Background), Flags::INVERSE, &o);
        assert_eq!(f, theme::TERM_BG);
        assert_eq!(b, Some(theme::TERM_FG));
        let (f, b) = cell_colors(Color::Named(NamedColor::Red), Color::Named(NamedColor::Background), Flags::BOLD, &o);
        assert_eq!(f, Color32::from_rgb(0xef, 0x29, 0x29));
        assert_eq!(b, None);
        let (f, _) = cell_colors(Color::Spec(Rgb { r: 1, g: 2, b: 3 }), Color::Named(NamedColor::Background), Flags::empty(), &o);
        assert_eq!(f, Color32::from_rgb(1, 2, 3));
    }
}

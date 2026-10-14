//! Tokens visuais (espelho do `:root` de `renderer/styles.css`), fontes e estilo do egui.

use std::path::PathBuf;
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};

// ---------- Cores (:root) ----------

pub const BG: Color32 = rgb(0x0b0b0d);
pub const PANEL: Color32 = rgb(0x131317);
pub const PANEL_2: Color32 = rgb(0x1b1b21);
pub const BORDER: Color32 = rgb(0x2a2a33);
pub const ACCENT: Color32 = rgb(0xb3263a);
pub const TEXT: Color32 = rgb(0xe6e6ea);
pub const MUTED: Color32 = rgb(0x8b8b96);
pub const CLAUDE: Color32 = rgb(0xd97757);
pub const ANTIGRAVITY: Color32 = rgb(0x4f8cf7);
pub const SHELL: Color32 = rgb(0x9aa0a6);
pub const OLLAMA: Color32 = rgb(0xf59e0b);

// ---------- Cores avulsas usadas no CSS ----------

pub const WHITE: Color32 = Color32::WHITE;
pub const RED: Color32 = rgb(0xe5484d);
pub const RED_TEXT: Color32 = rgb(0xff8b8f);
pub const TREE_TEXT: Color32 = rgb(0xcccccc);
pub const TREE_ICON: Color32 = rgb(0x858585);
pub const SELECT_TEXT: Color32 = rgb(0xa4a4af);
pub const CHIP_TEXT: Color32 = rgb(0xb8b8c2);
pub const INPUT_TEXT: Color32 = rgb(0xe2e2e8);
/// Cor padrão do placeholder de `<textarea>` no Chromium (tema escuro).
pub const PLACEHOLDER: Color32 = rgb(0x75757e);
/// `--send-color` padrão (azul-claro) quando não há aba específica.
pub const SEND_DEFAULT: Color32 = rgb(0x7cc4fa);
pub const SEND_IDLE_TEXT: Color32 = rgb(0x55555f);
pub const STOP_BG: Color32 = rgb(0x444444);
/// `whoColor()` para remetentes sem cor conhecida (ex.: "você").
pub const WHO_DEFAULT: Color32 = rgb(0xc9c9d1);
pub const DROP_BLUE: Color32 = rgb(0x4f8cf7);

// Badges de extensão da árvore (estilo VS Code)
pub const BADGE_JS: Color32 = rgb(0xf1e05a);
pub const BADGE_JSON: Color32 = rgb(0x4ade80);
pub const BADGE_MD: Color32 = rgb(0x519aba);
pub const BADGE_TS: Color32 = rgb(0x3178c6);
pub const BADGE_HTML: Color32 = rgb(0xe34f26);
pub const BADGE_CSS: Color32 = rgb(0x42a5f5);
pub const BADGE_PY: Color32 = rgb(0x3572a5);

// ---------- Terminal (tema do xterm) ----------

pub const TERM_BG: Color32 = rgb(0x0b0b0d);
pub const TERM_FG: Color32 = rgb(0xe6e6ea);
pub const TERM_CURSOR: Color32 = rgb(0xe6e6ea);
pub const TERM_SELECTION: Color32 = rgb(0x3a3a48);
pub const TERM_FONT_SIZE: f32 = 14.0;

// ---------- Métricas ----------

pub const UI_FONT_SIZE: f32 = 13.0;
pub const LINE_HEIGHT: f32 = 1.45;
pub const TOPBAR_H: f32 = 38.0;
pub const SIDEBAR_DEFAULT_W: f32 = 220.0;
pub const SIDEBAR_MIN_W: f32 = 150.0;
pub const SIDEBAR_MAX_W: f32 = 480.0;
pub const BOTTOM_DEFAULT_H: f32 = 310.0;

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// Cor CSS com alfa (`rgba(r, g, b, a)`).
pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Converte `#rgb`, `#rrggbb` ou `#rrggbbaa` em [`Color32`].
pub fn parse_hex_color(s: &str) -> Option<Color32> {
    let h = s.trim().strip_prefix('#')?;
    if !h.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let byte = |i: usize| u8::from_str_radix(&h[i..i + 2], 16).ok();
    match h.len() {
        3 => {
            let v: Vec<u8> = h.chars().map(|c| c.to_digit(16).unwrap_or(0) as u8 * 17).collect();
            Some(Color32::from_rgb(v[0], v[1], v[2]))
        }
        6 => Some(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Some(Color32::from_rgba_unmultiplied(byte(0)?, byte(2)?, byte(4)?, byte(6)?)),
        _ => None,
    }
}

/// `filter: brightness(f)` aproximado.
pub fn brighten(c: Color32, f: f32) -> Color32 {
    let ch = |v: u8| ((v as f32) * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(c.r()), ch(c.g()), ch(c.b()), c.a())
}

/// Aplica opacidade (multiplica o alfa).
pub fn with_opacity(c: Color32, opacity: f32) -> Color32 {
    c.gamma_multiply(opacity.clamp(0.0, 1.0))
}

// ---------- Fontes ----------

/// Famílias nomeadas registradas além de Proportional/Monospace.
pub const FAMILY_UI_SEMIBOLD: &str = "ui-semibold";
pub const FAMILY_UI_BOLD: &str = "ui-bold";
pub const FAMILY_MONO_BOLD: &str = "mono-bold";

pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
pub fn ui_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_SEMIBOLD.into()))
}
pub fn ui_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_BOLD.into()))
}
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_MONO_BOLD.into()))
}

/// Candidatos de arquivo de fonte por papel, na ordem de preferência do CSS
/// (`"Segoe UI", Roboto, …, Arial` e `"Cascadia Mono", Consolas, …`).
struct FontCandidates {
    /// Fallback para símbolos ausentes na fonte da interface (▾ ▸ ▶ → ● …).
    symbols: &'static [&'static str],
    ui: &'static [&'static str],
    ui_semibold: &'static [&'static str],
    ui_bold: &'static [&'static str],
    mono: &'static [&'static str],
    mono_bold: &'static [&'static str],
}

#[cfg(windows)]
const CANDIDATES: FontCandidates = FontCandidates {
    symbols: &["seguisym.ttf"],
    ui: &["segoeui.ttf", "arial.ttf"],
    ui_semibold: &["seguisb.ttf", "segoeuib.ttf", "arialbd.ttf"],
    ui_bold: &["segoeuib.ttf", "arialbd.ttf"],
    mono: &["CascadiaMono.ttf", "consola.ttf", "cour.ttf"],
    mono_bold: &["consolab.ttf", "courbd.ttf"],
};

#[cfg(target_os = "macos")]
const CANDIDATES: FontCandidates = FontCandidates {
    symbols: &["/System/Library/Fonts/Apple Symbols.ttf"],
    ui: &["/System/Library/Fonts/SFNS.ttf", "/Library/Fonts/Arial.ttf", "/System/Library/Fonts/Supplemental/Arial.ttf"],
    ui_semibold: &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"],
    ui_bold: &["/System/Library/Fonts/Supplemental/Arial Bold.ttf"],
    mono: &["/System/Library/Fonts/SFNSMono.ttf", "/System/Library/Fonts/Menlo.ttc"],
    mono_bold: &[],
};

#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: FontCandidates = FontCandidates {
    symbols: &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    ],
    ui: &[
        "/usr/share/fonts/truetype/roboto/unhinted/RobotoTTF/Roboto-Regular.ttf",
        "/usr/share/fonts/truetype/roboto/Roboto-Regular.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/google-noto/NotoSans-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
        "/usr/share/fonts/liberation-sans/LiberationSans-Regular.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    ],
    ui_semibold: &[
        "/usr/share/fonts/truetype/roboto/unhinted/RobotoTTF/Roboto-Medium.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/noto/NotoSans-SemiBold.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    ],
    ui_bold: &[
        "/usr/share/fonts/truetype/roboto/unhinted/RobotoTTF/Roboto-Bold.ttf",
        "/usr/share/fonts/truetype/noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/noto/NotoSans-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationSans-Bold.ttf",
        "/usr/share/fonts/liberation-sans/LiberationSans-Bold.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans-Bold.ttf",
    ],
    mono: &[
        "/usr/share/fonts/truetype/cascadia-code/CascadiaMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
    ],
    mono_bold: &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
        "/usr/share/fonts/liberation-mono/LiberationMono-Bold.ttf",
    ],
};

fn resolve_font_path(name: &str) -> PathBuf {
    let p = PathBuf::from(name);
    if p.is_absolute() {
        return p;
    }
    #[cfg(windows)]
    {
        let windir = std::env::var_os("WINDIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
        let sys = windir.join("Fonts").join(name);
        if sys.exists() {
            return sys;
        }
        // Fontes instaladas só para o usuário (ex.: Cascadia Mono via Windows Terminal/winget).
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            let user = PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts").join(name);
            if user.exists() {
                return user;
            }
        }
        sys
    }
    #[cfg(not(windows))]
    {
        p
    }
}

fn load_first(list: &[&str]) -> Option<(String, Vec<u8>)> {
    list.iter().find_map(|name| {
        let path = resolve_font_path(name);
        std::fs::read(&path).ok().map(|bytes| (path.display().to_string(), bytes))
    })
}

/// Fontes carregadas (para log/diagnóstico).
#[derive(Debug, Default, Clone)]
pub struct LoadedFonts {
    pub ui: Option<String>,
    pub mono: Option<String>,
}

/// Carrega as fontes do sistema (quando existirem) mantendo as fontes padrão do egui como
/// fallback — necessárias para emoji (📁 ⚙️ …) e símbolos.
pub fn install_fonts(ctx: &egui::Context) -> LoadedFonts {
    let mut defs = FontDefinitions::default();
    let default_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let default_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut loaded = LoadedFonts::default();

    let add = |defs: &mut FontDefinitions, key: &str, list: &[&str], tweak: Option<egui::FontTweak>| -> Option<String> {
        let (path, bytes) = load_first(list)?;
        let mut data = FontData::from_owned(bytes);
        if let Some(t) = tweak {
            data = data.tweak(t);
        }
        defs.font_data.insert(key.to_owned(), Arc::new(data));
        Some(path)
    };

    let ui = add(&mut defs, "sys-ui", CANDIDATES.ui, None);
    let ui_sb = add(&mut defs, "sys-ui-semibold", CANDIDATES.ui_semibold, None);
    let ui_b = add(&mut defs, "sys-ui-bold", CANDIDATES.ui_bold, None);
    let mono = add(&mut defs, "sys-mono", CANDIDATES.mono, None);
    let mut mono_b = add(&mut defs, "sys-mono-bold", CANDIDATES.mono_bold, None);
    let symbols = add(&mut defs, "sys-symbols", CANDIDATES.symbols, None);

    // Cascadia Mono é uma fonte variável: o negrito vem do eixo `wght`.
    if mono_b.is_none() {
        if let (Some(path), Some(data)) = (mono.as_ref(), defs.font_data.get("sys-mono").cloned()) {
            if path.to_lowercase().contains("cascadia") {
                let mut bold = (*data).clone();
                bold.tweak.coords.push("wght", 700.0);
                defs.font_data.insert("sys-mono-bold".into(), Arc::new(bold));
                mono_b = Some(path.clone());
            }
        }
    }

    let sym_key = symbols.as_ref().map(|_| "sys-symbols");
    let chain = |first: &[Option<&str>], rest: &[String]| -> Vec<String> {
        let mut v: Vec<String> = first.iter().flatten().map(|s| s.to_string()).collect();
        if let Some(k) = sym_key {
            if !v.iter().any(|x| x == k) {
                v.push(k.to_owned());
            }
        }
        for r in rest {
            if !v.contains(r) {
                v.push(r.clone());
            }
        }
        v
    };

    let ui_key = ui.as_ref().map(|_| "sys-ui");
    let mono_key = mono.as_ref().map(|_| "sys-mono");
    defs.families.insert(FontFamily::Proportional, chain(&[ui_key], &default_prop));
    defs.families.insert(FontFamily::Monospace, chain(&[mono_key], &default_mono));
    defs.families
        .insert(FontFamily::Name(FAMILY_UI_SEMIBOLD.into()), chain(&[ui_sb.as_ref().map(|_| "sys-ui-semibold"), ui_key], &default_prop));
    defs.families.insert(
        FontFamily::Name(FAMILY_UI_BOLD.into()),
        chain(&[ui_b.as_ref().map(|_| "sys-ui-bold"), ui_sb.as_ref().map(|_| "sys-ui-semibold"), ui_key], &default_prop),
    );
    defs.families
        .insert(FontFamily::Name(FAMILY_MONO_BOLD.into()), chain(&[mono_b.as_ref().map(|_| "sys-mono-bold"), mono_key], &default_mono));

    loaded.ui = ui;
    loaded.mono = mono;
    ctx.set_fonts(defs);
    loaded
}

/// Estilo base do egui (tema escuro com as cores do CSS). Quase tudo é pintado à mão;
/// isto cobre widgets nativos usados (TextEdit, ScrollArea, tooltips).
pub fn install_style(ctx: &egui::Context) {
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, ui(UI_FONT_SIZE));
        style.text_styles.insert(egui::TextStyle::Button, ui(UI_FONT_SIZE));
        style.text_styles.insert(egui::TextStyle::Small, ui(11.0));
        style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
        style.spacing.window_margin = egui::Margin::same(0);
        style.spacing.button_padding = egui::vec2(0.0, 0.0);
        style.spacing.tooltip_width = 420.0;

        // Barras de rolagem como `::-webkit-scrollbar` (5px, polegar translúcido, sem trilho).
        let mut scroll = egui::style::ScrollStyle::solid();
        scroll.bar_width = 5.0;
        scroll.bar_inner_margin = 0.0;
        scroll.bar_outer_margin = 0.0;
        scroll.handle_min_length = 20.0;
        style.spacing.scroll = scroll;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = BG;
        v.window_fill = PANEL_2;
        v.window_stroke = egui::Stroke::new(1.0, BORDER);
        v.window_corner_radius = egui::CornerRadius::same(6);
        v.window_shadow = egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: rgba(0, 0, 0, 0.5) };
        v.popup_shadow = v.window_shadow;
        v.extreme_bg_color = Color32::TRANSPARENT;
        v.faint_bg_color = PANEL;
        v.selection.bg_fill = rgba(79, 140, 247, 0.45);
        v.selection.stroke = egui::Stroke::new(1.0, WHITE);
        v.text_cursor.stroke = egui::Stroke::new(1.5, INPUT_TEXT);
        v.hyperlink_color = ANTIGRAVITY;

        let thumb = rgba(255, 255, 255, 0.12);
        let thumb_hover = rgba(255, 255, 255, 0.25);
        for (w, fill) in [
            (&mut v.widgets.noninteractive, PANEL_2),
            (&mut v.widgets.inactive, thumb),
            (&mut v.widgets.hovered, thumb_hover),
            (&mut v.widgets.active, thumb_hover),
            (&mut v.widgets.open, thumb_hover),
        ] {
            w.bg_fill = fill;
            w.weak_bg_fill = Color32::TRANSPARENT;
            w.bg_stroke = egui::Stroke::NONE;
            w.corner_radius = egui::CornerRadius::same(3);
            w.expansion = 0.0;
        }
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, TEXT);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, WHITE);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, WHITE);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, BORDER);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_hex_color("#d97757"), Some(Color32::from_rgb(0xd9, 0x77, 0x57)));
        assert_eq!(parse_hex_color("  #4F8CF7 "), Some(Color32::from_rgb(0x4f, 0x8c, 0xf7)));
        assert_eq!(parse_hex_color("#444"), Some(Color32::from_rgb(0x44, 0x44, 0x44)));
        assert_eq!(parse_hex_color("#000000aa"), Some(Color32::from_rgba_unmultiplied(0, 0, 0, 0xaa)));
        assert_eq!(parse_hex_color("d97757"), None);
        assert_eq!(parse_hex_color("#12345"), None);
        assert_eq!(parse_hex_color("#gggggg"), None);
    }

    #[test]
    fn constants_match_css_root() {
        assert_eq!(Some(BG), parse_hex_color("#0b0b0d"));
        assert_eq!(Some(PANEL_2), parse_hex_color("#1b1b21"));
        assert_eq!(Some(CLAUDE), parse_hex_color(commanddeck_core::agents::COLOR_CLAUDE));
        assert_eq!(Some(ANTIGRAVITY), parse_hex_color(commanddeck_core::agents::COLOR_ANTIGRAVITY));
        assert_eq!(Some(OLLAMA), parse_hex_color(commanddeck_core::agents::COLOR_OLLAMA));
        assert_eq!(Some(SHELL), parse_hex_color(commanddeck_core::agents::COLOR_SHELL));
    }

    #[test]
    fn brightness_filter() {
        assert_eq!(brighten(Color32::from_rgb(100, 200, 250), 1.15), Color32::from_rgb(115, 230, 255));
    }
}

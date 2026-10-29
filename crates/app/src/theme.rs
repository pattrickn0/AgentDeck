//! Visual tokens (semantic palette), fonts and egui styling.
//!
//! Colors come from [`Palette`]: the app always reads `theme::pal()`, and a new theme only swaps the palette
//! returned there. The accent color is not in the palette: it belongs to the active agent (see [`accent`]).

use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use egui::{Color32, FontData, FontDefinitions, FontFamily, FontId};

// ---------- Palette ----------

/// Semantic colors of the interface. Names describe role, not value.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Palette {
    /// Window background behind islands.
    pub bg: Color32,
    /// Background of glass islands (left and right sidebars).
    pub glass: Color32,
    /// 1 px island border.
    pub glass_border: Color32,
    /// 1 px highlight at internal top of islands.
    pub glass_highlight: Color32,
    /// Center island (the terminal, nearly opaque).
    pub well: Color32,
    /// Center tab strip.
    pub strip: Color32,
    /// Divider lines inside islands.
    pub separator: Color32,
    /// Tree guide lines.
    pub guide: Color32,
    pub hover: Color32,
    /// Neutral selection (active segment without accent, "Tasks" tab).
    pub selected: Color32,
    /// Selected file.
    pub selected_soft: Color32,
    /// Track for segmented controls and text field background.
    pub recess: Color32,
    /// Badges (branch, counters) and subtle button background.
    pub chip: Color32,
    /// Menus, modals and tooltips: must be opaque.
    pub popup: Color32,
    pub text: Color32,
    /// Selected text.
    pub text_strong: Color32,
    pub text_2: Color32,
    /// Section labels and metadata.
    pub text_3: Color32,
    /// Between `text` and `text_2`: values and badges (usage percentage, branch name).
    pub text_soft: Color32,
    /// Text separators ("/").
    pub text_faint: Color32,
    /// Text over accent color.
    pub on_accent: Color32,
    /// Accent when no tab is focused (light gray).
    pub accent_neutral: Color32,
    pub danger: Color32,
    pub danger_text: Color32,
    pub success: Color32,
    pub warning: Color32,
    /// Information blue (renamed file, commit hash, link).
    pub info: Color32,
    /// Dark backdrop behind a modal.
    pub scrim: Color32,
    /// Bottom-right background glow (white, 3.5%).
    pub glow: Color32,
    /// Color of user messages in Chat ("you"): identity, not state.
    pub user: Color32,
    /// Island shadows.
    pub shadow: Color32,
    /// Syntax highlighting (Documents mode files and diff; see `highlight.rs`).
    pub syn_keyword: Color32,
    pub syn_type: Color32,
    pub syn_function: Color32,
    pub syn_string: Color32,
    pub syn_number: Color32,
    pub syn_comment: Color32,
    pub syn_key: Color32,
    pub syn_meta: Color32,
}

const fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

/// CSS `rgba(r, g, b, a)`, with alpha in 0..=255.
const fn rgba8(r: u8, g: u8, b: u8, a: u8) -> Color32 {
    Color32::from_rgba_unmultiplied_const(r, g, b, a)
}

const fn white(a: u8) -> Color32 {
    rgba8(255, 255, 255, a)
}

/// Dark theme (direction "C": GitHub-style gray with subtle glass).
pub const DARK: Palette = Palette {
    bg: rgb(0x16181c),
    glass: rgba8(44, 47, 54, 128),
    glass_border: white(33),
    glass_highlight: white(20),
    well: rgba8(22, 24, 28, 235),
    strip: rgba8(44, 47, 54, 140),
    separator: white(26),
    guide: white(20),
    hover: white(18),
    selected: white(31),
    selected_soft: white(20),
    recess: rgba8(0, 0, 0, 64),
    chip: white(18),
    popup: rgb(0x23262c),
    text: rgb(0xe6e8eb),
    text_strong: rgb(0xffffff),
    text_2: rgb(0xaab1bb),
    text_3: rgb(0x939aa4),
    text_soft: rgb(0xc9cdd3),
    text_faint: rgb(0x6b717c),
    on_accent: rgb(0x16181c),
    accent_neutral: rgb(0xc9cdd3),
    danger: rgb(0xe5484d),
    danger_text: rgb(0xff8b8f),
    success: rgb(0x3fb97a),
    warning: rgb(0xe5a23a),
    info: rgb(0x7cc4fa),
    scrim: rgba8(0, 0, 0, 128),
    glow: white(9),
    user: rgb(0x10b981),
    shadow: rgba8(0, 0, 0, 71),
    syn_keyword: rgb(0xff7b72),
    syn_type: rgb(0xffa657),
    syn_function: rgb(0xd2a8ff),
    syn_string: rgb(0xa5d6ff),
    syn_number: rgb(0x79c0ff),
    syn_comment: rgb(0x8b949e),
    syn_key: rgb(0x7ee787),
    syn_meta: rgb(0xe3b341),
};

/// Active palette. A new theme changes only what this function returns.
pub fn pal() -> &'static Palette {
    &DARK
}

impl Palette {
    /// Color of a code token; tokens without a category use `plain`.
    pub fn syn(&self, tok: crate::highlight::Tok, plain: Color32) -> Color32 {
        use crate::highlight::Tok;
        match tok {
            Tok::Plain => plain,
            Tok::Keyword => self.syn_keyword,
            Tok::Type => self.syn_type,
            Tok::Function => self.syn_function,
            Tok::String => self.syn_string,
            Tok::Number => self.syn_number,
            Tok::Comment => self.syn_comment,
            Tok::Key => self.syn_key,
            Tok::Meta => self.syn_meta,
        }
    }

    /// Text and background color for a code token with search highlighting: found matches get amber background,
    /// current match gets a stronger background with dark text (gray comments are unreadable on amber).
    pub fn syn_mark(&self, tok: crate::highlight::Tok, mark: crate::highlight::Mark, plain: Color32) -> (Color32, Color32) {
        use crate::highlight::Mark;
        match mark {
            Mark::None => (self.syn(tok, plain), Color32::TRANSPARENT),
            Mark::Found => (self.syn(tok, plain), with_alpha(self.warning, 0.30)),
            Mark::Current => (self.on_accent, with_alpha(self.warning, 0.80)),
        }
    }
}

// ---------- Agent colors (not in palette: identify each agent) ----------

pub const CLAUDE: Color32 = rgb(0xd97757);
pub const ANTIGRAVITY: Color32 = rgb(0x4f8cf7);
pub const SHELL: Color32 = rgb(0x9aa0a6);

// ---------- Accent color = active agent color ----------

/// Bit indicating a color is set (without it, neutral gray applies).
const ACCENT_SET: u32 = 1 << 24;
static ACCENT: AtomicU32 = AtomicU32::new(0);

/// Frame accent color: focused agent's color, or neutral gray when no tab is open. Window rendering
/// (`ui::draw`) updates this at the start of each frame via [`set_accent`]. Never use a fixed brand
/// color for selection or primary action.
pub fn accent() -> Color32 {
    unpack_accent(ACCENT.load(Ordering::Relaxed))
}

/// The stored color (or neutral, without the "color set" bit).
fn unpack_accent(v: u32) -> Color32 {
    if v & ACCENT_SET == 0 {
        return pal().accent_neutral;
    }
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

fn pack_accent(c: Option<Color32>) -> u32 {
    c.map_or(0, |c| ACCENT_SET | (c.r() as u32) << 16 | (c.g() as u32) << 8 | c.b() as u32)
}

/// Stores the frame accent color (`None`: no focused tab).
pub fn set_accent(c: Option<Color32>) {
    ACCENT.store(pack_accent(c), Ordering::Relaxed);
}

/// Accent at 18% alpha: selected row background.
#[allow(dead_code)] // For part 2 (selected row).
pub fn accent_soft(a: Color32) -> Color32 {
    with_alpha(a, 0.18)
}

/// Accent at 10% alpha: focused card background.
pub fn accent_wash(a: Color32) -> Color32 {
    with_alpha(a, 0.10)
}

/// Accent at 50% alpha: focused card border.
pub fn accent_edge(a: Color32) -> Color32 {
    with_alpha(a, 0.50)
}

// ---------- Terminal (xterm theme) ----------

pub const TERM_BG: Color32 = rgb(0x16181c);
pub const TERM_FG: Color32 = rgb(0xe6e8eb);
pub const TERM_CURSOR: Color32 = rgb(0xe6e8eb);
pub const TERM_SELECTION: Color32 = rgb(0x2f3542);
pub const TERM_FONT_SIZE: f32 = 14.0;

// ---------- Metrics ----------

/// Base font size for UI text.
pub const UI_FONT_SIZE: f32 = 12.0;
pub const LINE_HEIGHT: f32 = 1.45;
/// Custom titlebar (window has no system decoration), without background.
pub const TITLEBAR_H: f32 = 32.0;
/// Window margin and island gap.
pub const ISLAND_GAP: f32 = 6.0;
/// Island corner radius.
pub const ISLAND_RADIUS: u8 = 10;
/// Selected agent window (Split and Highlight): 3 px halo in agent color (at 18%)...
pub const PANE_HALO_W: f32 = 3.0;
/// ...and other windows dimmed to 62% opacity (restored to 100% on hover).
pub const PANE_DIM_OPACITY: f32 = 0.62;
/// Right sidebar header (Tasks/Chat/Git/GitHub tabs).
pub const TOPBAR_H: f32 = 32.0;
pub const SIDEBAR_DEFAULT_W: f32 = 220.0;
pub const SIDEBAR_MIN_W: f32 = 150.0;
pub const SIDEBAR_MAX_W: f32 = 480.0;
/// Right sidebar (Unified chat): does not collapse, only resizes.
pub const RIGHT_DEFAULT_W: f32 = 330.0;
pub const RIGHT_MIN_W: f32 = 280.0;
/// Previous default (380): users who never dragged the bar had this saved and upgrade to new default.
pub const RIGHT_OLD_DEFAULT_W: f32 = 380.0;
pub const RIGHT_MAX_W: f32 = 640.0;

/// CSS color with alpha (`rgba(r, g, b, a)`).
pub fn rgba(r: u8, g: u8, b: u8, a: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(r, g, b, (a.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Same color with replacement alpha (0..=1), without inheriting previous alpha.
pub fn with_alpha(c: Color32, alpha: f32) -> Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    Color32::from_rgba_unmultiplied(r, g, b, (alpha.clamp(0.0, 1.0) * 255.0).round() as u8)
}

/// Converts `#rgb`, `#rrggbb` or `#rrggbbaa` to [`Color32`].
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

/// Approximate `filter: brightness(f)`.
pub fn brighten(c: Color32, f: f32) -> Color32 {
    let ch = |v: u8| ((v as f32) * f).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(c.r()), ch(c.g()), ch(c.b()), c.a())
}

/// Applies opacity (multiplies alpha).
pub fn with_opacity(c: Color32, opacity: f32) -> Color32 {
    c.gamma_multiply(opacity.clamp(0.0, 1.0))
}

/// Glass as if opaque (over background): used to hide content behind part of a sidebar.
pub fn glass_solid() -> Color32 {
    over(pal().glass, pal().bg)
}

/// Center well as if opaque (over background).
pub fn well_solid() -> Color32 {
    over(pal().well, pal().bg)
}

/// Tab strip as if opaque (over well): scroll fade end color for tabs.
pub fn strip_solid() -> Color32 {
    over(pal().strip, well_solid())
}

/// `top` drawn over `bottom` (standard alpha compositing). Result is opaque if `bottom` is.
pub fn over(top: Color32, bottom: Color32) -> Color32 {
    let a = top.a() as f32 / 255.0;
    let t = top.to_srgba_unmultiplied();
    let b = bottom.to_srgba_unmultiplied();
    let ba = b[3] as f32 / 255.0;
    let out_a = a + ba * (1.0 - a);
    if out_a <= 0.0 {
        return Color32::TRANSPARENT;
    }
    let ch = |i: usize| ((t[i] as f32 * a + b[i] as f32 * ba * (1.0 - a)) / out_a).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(0), ch(1), ch(2), (out_a * 255.0).round() as u8)
}

// ---------- Fonts ----------

/// Named families registered in addition to Proportional/Monospace.
pub const FAMILY_UI_MEDIUM: &str = "ui-medium";
pub const FAMILY_UI_SEMIBOLD: &str = "ui-semibold";
pub const FAMILY_UI_BOLD: &str = "ui-bold";
pub const FAMILY_MONO_BOLD: &str = "mono-bold";
/// IBM Plex Mono Medium: UPPERCASE section labels.
pub const FAMILY_LABEL: &str = "label";
/// IBM Plex Mono Regular: metadata and badges.
pub const FAMILY_META: &str = "meta";

/// Inter (UI) and IBM Plex Mono (labels and metadata), both under OFL license (see
/// `assets/fonts/OFL-*.txt`). Embedded in executable: visual styling is independent of installed fonts.
const INTER_REGULAR: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
const INTER_MEDIUM: &[u8] = include_bytes!("../assets/fonts/Inter-Medium.ttf");
const INTER_SEMIBOLD: &[u8] = include_bytes!("../assets/fonts/Inter-SemiBold.ttf");
const INTER_BOLD: &[u8] = include_bytes!("../assets/fonts/Inter-Bold.ttf");
const PLEX_MONO_REGULAR: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Regular.ttf");
const PLEX_MONO_MEDIUM: &[u8] = include_bytes!("../assets/fonts/IBMPlexMono-Medium.ttf");

/// UI text (Inter Regular).
pub fn ui(size: f32) -> FontId {
    FontId::new(size, FontFamily::Proportional)
}
#[allow(dead_code)] // For part 2 (agent names).
pub fn ui_medium(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_MEDIUM.into()))
}
pub fn ui_semibold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_SEMIBOLD.into()))
}
pub fn ui_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_UI_BOLD.into()))
}
/// UPPERCASE section labels (IBM Plex Mono Medium). Use with [`LABEL_SPACING_EM`].
pub fn label(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_LABEL.into()))
}
/// Metadata and badges (IBM Plex Mono Regular).
pub fn meta(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_META.into()))
}
/// Terminal font (system font, unchanged).
pub fn mono(size: f32) -> FontId {
    FontId::new(size, FontFamily::Monospace)
}
pub fn mono_bold(size: f32) -> FontId {
    FontId::new(size, FontFamily::Name(FAMILY_MONO_BOLD.into()))
}

/// Letter spacing for section labels: 0.1 em.
pub const LABEL_SPACING_EM: f32 = 0.1;

/// System font candidates by role: terminal and symbols (▾ ▸ ✕ …)
/// missing from Inter. The UI uses embedded fonts.
struct FontCandidates {
    /// Fallback for missing symbols in UI font.
    symbols: &'static [&'static str],
    mono: &'static [&'static str],
    mono_bold: &'static [&'static str],
}

#[cfg(windows)]
const CANDIDATES: FontCandidates = FontCandidates {
    symbols: &["seguisym.ttf"],
    mono: &["CascadiaMono.ttf", "consola.ttf", "cour.ttf"],
    mono_bold: &["consolab.ttf", "courbd.ttf"],
};

#[cfg(target_os = "macos")]
const CANDIDATES: FontCandidates = FontCandidates {
    symbols: &["/System/Library/Fonts/Apple Symbols.ttf"],
    mono: &["/System/Library/Fonts/SFNSMono.ttf", "/System/Library/Fonts/Menlo.ttc"],
    mono_bold: &[],
};

#[cfg(all(unix, not(target_os = "macos")))]
const CANDIDATES: FontCandidates = FontCandidates {
    // Debian/Ubuntu use `/usr/share/fonts/truetype/<family>/`; Fedora installs each
    // subpackage in its own folder (`dejavu-sans-mono-fonts/`); openSUSE puts them all straight in
    // `/usr/share/fonts/truetype/`. Missing path does not fail:
    // egui falls back to embedded font.
    symbols: &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu/DejaVuSans.ttf",
        "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/DejaVuSans.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansSymbols2-Regular.ttf",
        "/usr/share/fonts/truetype/freefont/FreeSans.ttf",
    ],
    mono: &[
        "/usr/share/fonts/truetype/cascadia-code/CascadiaMono.ttf",
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/DejaVuSansMono.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Regular.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Regular.ttf",
        "/usr/share/fonts/liberation-mono/LiberationMono-Regular.ttf",
        "/usr/share/fonts/truetype/LiberationMono-Regular.ttf",
    ],
    mono_bold: &[
        "/usr/share/fonts/truetype/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/dejavu/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/dejavu-sans-mono-fonts/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/DejaVuSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/noto/NotoSansMono-Bold.ttf",
        "/usr/share/fonts/truetype/liberation/LiberationMono-Bold.ttf",
        "/usr/share/fonts/liberation-mono/LiberationMono-Bold.ttf",
        "/usr/share/fonts/truetype/LiberationMono-Bold.ttf",
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
        // User-installed fonts (e.g. Cascadia Mono via Windows Terminal/winget).
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

/// Loaded fonts (for log/diagnostics).
#[derive(Debug, Default, Clone)]
pub struct LoadedFonts {
    pub ui: Option<String>,
    pub mono: Option<String>,
}

/// Registers embedded fonts (UI, labels and metadata) and system fonts (terminal and
/// symbols), keeping egui default fonts as fallback — necessary for emoji (📁 ⚙️ …).
pub fn install_fonts(ctx: &egui::Context) -> LoadedFonts {
    let mut defs = FontDefinitions::default();
    let default_prop = defs.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
    let default_mono = defs.families.get(&FontFamily::Monospace).cloned().unwrap_or_default();
    let mut loaded = LoadedFonts::default();

    // Embedded fonts.
    for (key, bytes) in [
        ("inter", INTER_REGULAR),
        ("inter-medium", INTER_MEDIUM),
        ("inter-semibold", INTER_SEMIBOLD),
        ("inter-bold", INTER_BOLD),
        ("plex-mono", PLEX_MONO_REGULAR),
        ("plex-mono-medium", PLEX_MONO_MEDIUM),
    ] {
        defs.font_data.insert(key.to_owned(), Arc::new(FontData::from_static(bytes)));
    }

    // System fonts.
    let add = |defs: &mut FontDefinitions, key: &str, list: &[&str]| -> Option<String> {
        let (path, bytes) = load_first(list)?;
        defs.font_data.insert(key.to_owned(), Arc::new(FontData::from_owned(bytes)));
        Some(path)
    };
    let mono = add(&mut defs, "sys-mono", CANDIDATES.mono);
    let mut mono_b = add(&mut defs, "sys-mono-bold", CANDIDATES.mono_bold);
    let symbols = add(&mut defs, "sys-symbols", CANDIDATES.symbols);

    // Cascadia Mono is a variable font: bold comes from the `wght` axis.
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

    let mono_key = mono.as_ref().map(|_| "sys-mono");
    defs.families.insert(FontFamily::Proportional, chain(&[Some("inter")], &default_prop));
    defs.families.insert(FontFamily::Monospace, chain(&[mono_key], &default_mono));
    defs.families.insert(FontFamily::Name(FAMILY_UI_MEDIUM.into()), chain(&[Some("inter-medium"), Some("inter")], &default_prop));
    defs.families.insert(FontFamily::Name(FAMILY_UI_SEMIBOLD.into()), chain(&[Some("inter-semibold"), Some("inter")], &default_prop));
    defs.families.insert(FontFamily::Name(FAMILY_UI_BOLD.into()), chain(&[Some("inter-bold"), Some("inter")], &default_prop));
    defs.families.insert(FontFamily::Name(FAMILY_LABEL.into()), chain(&[Some("plex-mono-medium"), Some("inter")], &default_prop));
    defs.families.insert(FontFamily::Name(FAMILY_META.into()), chain(&[Some("plex-mono"), Some("inter")], &default_prop));
    defs.families
        .insert(FontFamily::Name(FAMILY_MONO_BOLD.into()), chain(&[mono_b.as_ref().map(|_| "sys-mono-bold"), mono_key], &default_mono));

    loaded.ui = Some("Inter (embedded)".to_owned());
    loaded.mono = mono;
    ctx.set_fonts(defs);
    loaded
}

/// egui text selection tracks accent color (call every frame; only rewrites style on color change).
pub fn sync_selection(ctx: &egui::Context, accent: Color32) {
    let id = egui::Id::new("selection-accent");
    if ctx.data(|d| d.get_temp::<Color32>(id)) == Some(accent) {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(id, accent));
    ctx.all_styles_mut(|s| s.visuals.selection.bg_fill = with_alpha(accent, 0.35));
}

/// egui base styling (dark theme with palette). Almost everything is custom painted; this covers
/// native widgets used (TextEdit, ScrollArea, tooltips).
pub fn install_style(ctx: &egui::Context) {
    let p = pal();
    ctx.set_theme(egui::Theme::Dark);
    ctx.all_styles_mut(|style| {
        style.text_styles.insert(egui::TextStyle::Body, ui(UI_FONT_SIZE));
        style.text_styles.insert(egui::TextStyle::Button, ui(UI_FONT_SIZE));
        style.text_styles.insert(egui::TextStyle::Small, ui(10.5));
        style.text_styles.insert(egui::TextStyle::Monospace, mono(12.0));
        style.spacing.item_spacing = egui::vec2(0.0, 0.0);
        style.spacing.window_margin = egui::Margin::same(0);
        style.spacing.button_padding = egui::vec2(0.0, 0.0);
        style.spacing.tooltip_width = 420.0;

        // Thin scrollbars (5px, translucent thumb, trackless).
        let mut scroll = egui::style::ScrollStyle::solid();
        scroll.bar_width = 5.0;
        scroll.bar_inner_margin = 0.0;
        scroll.bar_outer_margin = 0.0;
        scroll.handle_min_length = 20.0;
        style.spacing.scroll = scroll;

        let v = &mut style.visuals;
        v.dark_mode = true;
        v.override_text_color = None;
        v.panel_fill = p.bg;
        v.window_fill = p.popup;
        v.window_stroke = egui::Stroke::new(1.0, p.glass_border);
        v.window_corner_radius = egui::CornerRadius::same(6);
        v.window_shadow = egui::Shadow { offset: [0, 4], blur: 12, spread: 0, color: rgba(0, 0, 0, 0.5) };
        v.popup_shadow = v.window_shadow;
        v.extreme_bg_color = Color32::TRANSPARENT;
        v.faint_bg_color = p.chip;
        v.selection.bg_fill = with_alpha(accent(), 0.35);
        v.selection.stroke = egui::Stroke::new(1.0, p.text_strong);
        v.text_cursor.stroke = egui::Stroke::new(1.5, p.text);
        v.hyperlink_color = p.info;

        let thumb = white(31);
        let thumb_hover = white(64);
        for (w, fill) in [
            (&mut v.widgets.noninteractive, p.popup),
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
        v.widgets.noninteractive.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.inactive.fg_stroke = egui::Stroke::new(1.0, p.text);
        v.widgets.hovered.fg_stroke = egui::Stroke::new(1.0, p.text_strong);
        v.widgets.active.fg_stroke = egui::Stroke::new(1.0, p.text_strong);
        v.widgets.noninteractive.bg_stroke = egui::Stroke::new(1.0, p.glass_border);
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
    fn agent_colors_match_the_core() {
        assert_eq!(Some(CLAUDE), parse_hex_color(holt_core::agents::COLOR_CLAUDE));
        assert_eq!(Some(ANTIGRAVITY), parse_hex_color(holt_core::agents::COLOR_ANTIGRAVITY));
        assert_eq!(Some(SHELL), parse_hex_color(holt_core::agents::COLOR_SHELL));
    }

    #[test]
    fn the_palette_follows_the_c_direction() {
        let p = DARK;
        assert_eq!(Some(p.bg), parse_hex_color("#16181c"));
        assert_eq!(Some(TERM_BG), parse_hex_color("#16181c"));
        assert_eq!(p.bg, TERM_BG);
        assert_eq!(Some(p.text), parse_hex_color("#e6e8eb"));
        assert_eq!(Some(p.text_3), parse_hex_color("#939aa4"));
        // rgba(44,47,54,.50) and rgba(255,255,255,.13): alpha is rounded to 0..=255.
        // Color32 stores premultiplied alpha: unmultiplying can vary each channel by 1.
        let near = |c: Color32, want: [u8; 4]| c.to_srgba_unmultiplied().iter().zip(want).all(|(a, b)| a.abs_diff(b) <= 1);
        assert!(near(p.glass, [44, 47, 54, 128]), "{:?}", p.glass);
        assert!(near(p.glass_border, [255, 255, 255, 33]), "{:?}", p.glass_border);
        assert!(near(p.well, [22, 24, 28, 235]), "{:?}", p.well);
    }

    // Does not touch global (`ui::draw` changes it in other tests concurrently): conversion only.
    #[test]
    fn the_accent_is_the_active_agent_color_or_a_neutral_gray() {
        assert_eq!(unpack_accent(pack_accent(None)), pal().accent_neutral);
        assert_eq!(unpack_accent(pack_accent(Some(CLAUDE))), CLAUDE);
        // Black is also a stored color.
        assert_eq!(unpack_accent(pack_accent(Some(Color32::BLACK))), Color32::BLACK);
        assert_eq!(accent_soft(CLAUDE).a(), 46);
        assert_eq!(accent_wash(CLAUDE).a(), 26);
        assert_eq!(accent_edge(CLAUDE).a(), 128);
    }

    #[test]
    fn composes_colors_like_css() {
        let c = over(white(128), Color32::from_rgb(0, 0, 0));
        assert_eq!(c.to_srgba_unmultiplied(), [128, 128, 128, 255]);
        assert_eq!(over(Color32::from_rgb(1, 2, 3), Color32::from_rgb(9, 9, 9)), Color32::from_rgb(1, 2, 3));
    }

    /// Files allowed to create colors: palette, terminal colors and icons (brand).
    const COLOR_HOMES: [&str; 3] = ["theme.rs", "terminal/colors.rs", "icons.rs"];

    /// Files that still have loose colors and parts 2 and 3 of the visual design will migrate to the palette.
    /// The list only shrinks: a cleaned file must be removed (tested below). Empty since part 1.
    const STILL_LOOSE: [&str; 0] = [];

    const LOOSE_COLOR: [&str; 7] =
        ["Color32::from_rgb", "Color32::from_rgba", "from_gray", "from_rgba_", "from_white_alpha", "from_black_alpha", "theme::rgba("];

    fn rust_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(dir).expect("source directory") {
            let path = entry.expect("entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
    }

    /// Lines with loose colors outside tests (everything after `#[cfg(test)]` is test) and outside comments.
    fn loose_colors(text: &str) -> Vec<String> {
        text.lines()
            .take_while(|l| !l.trim_start().starts_with("#[cfg(test)]"))
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| LOOSE_COLOR.iter().any(|p| l.contains(p)))
            .map(|l| l.trim().to_owned())
            .collect()
    }

    #[test]
    fn colors_come_from_the_palette() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut files = Vec::new();
        rust_files(&root, &mut files);
        let mut loose = Vec::new();
        for f in files {
            let rel = f.strip_prefix(&root).expect("inside src").to_string_lossy().replace('\\', "/");
            if COLOR_HOMES.contains(&rel.as_str()) || rel.ends_with("_tests.rs") {
                continue;
            }
            let lines = loose_colors(&std::fs::read_to_string(&f).expect("read file"));
            let listed = STILL_LOOSE.contains(&rel.as_str());
            if lines.is_empty() && listed {
                panic!("{rel} no longer has loose colors: remove it from STILL_LOOSE");
            }
            if !lines.is_empty() && !listed {
                loose.push(format!("{rel}: {}", lines.join(" | ")));
            }
        }
        assert!(
            loose.is_empty(),
            "loose color outside theme.rs, terminal/colors.rs and icons.rs (use theme::pal()):\n{}",
            loose.join("\n")
        );
    }

    #[test]
    fn the_loose_color_scan_knows_what_to_catch() {
        assert_eq!(loose_colors("let a = Color32::from_rgb(1, 2, 3);\nlet b = 1;").len(), 1);
        assert_eq!(loose_colors("x(theme::rgba(0, 0, 0, 0.5))").len(), 1);
        assert_eq!(loose_colors("Color32::from_rgba_unmultiplied(1, 2, 3, 4)").len(), 1);
        assert!(loose_colors("// Color32::from_rgb(1, 2, 3)").is_empty());
        assert!(loose_colors("let c = theme::pal().text;").is_empty());
        // Everything after #[cfg(test)] is a test.
        assert!(loose_colors("fn a() {}\n#[cfg(test)]\nmod t { let c = Color32::from_rgb(1, 2, 3); }").is_empty());
    }

    #[test]
    fn brightness_filter() {
        assert_eq!(brighten(Color32::from_rgb(100, 200, 250), 1.15), Color32::from_rgb(115, 230, 255));
    }

    fn rel_lum(c: Color32) -> f32 {
        let ch = |v: u8| {
            let f = v as f32 / 255.0;
            if f <= 0.04045 {
                f / 12.92
            } else {
                ((f + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * ch(c.r()) + 0.7152 * ch(c.g()) + 0.0722 * ch(c.b())
    }

    fn contrast(c1: Color32, c2: Color32) -> f32 {
        let (l1, l2) = (rel_lum(c1), rel_lum(c2));
        let (lighter, darker) = if l1 >= l2 { (l1, l2) } else { (l2, l1) };
        (lighter + 0.05) / (darker + 0.05)
    }

    #[test]
    fn text_keeps_its_contrast_on_the_islands() {
        let p = pal();
        // Glass over background and background with strongest glow (10% orange) that can appear behind.
        let glow = over(with_alpha(CLAUDE, 0.10), p.bg);
        for under in [p.bg, glow] {
            let glass = over(p.glass, under);
            let well = over(p.well, under);
            for (name, c) in [("text", p.text), ("text_2", p.text_2), ("text_3", p.text_3)] {
                assert!(contrast(c, glass) >= 4.5, "{name} on glass: {}", contrast(c, glass));
                assert!(contrast(c, well) >= 4.5, "{name} on well: {}", contrast(c, well));
            }
        }
        // Text hierarchy: stronger → dimmer.
        assert!(rel_lum(p.text_strong) > rel_lum(p.text));
        assert!(rel_lum(p.text) > rel_lum(p.text_2));
        assert!(rel_lum(p.text_2) > rel_lum(p.text_3));
        assert!(rel_lum(p.text_3) > rel_lum(p.text_faint));
        // Text over each agent color (primary button, active segment).
        for a in [CLAUDE, ANTIGRAVITY, SHELL, p.accent_neutral] {
            assert!(contrast(p.on_accent, a) >= 4.5, "on_accent on {a:?}: {}", contrast(p.on_accent, a));
        }
        // Syntax highlighting over well (file background) and over highlighted search match.
        let well = over(p.well, p.bg);
        for c in [p.syn_keyword, p.syn_type, p.syn_function, p.syn_string, p.syn_number, p.syn_comment, p.syn_key, p.syn_meta] {
            assert!(contrast(c, well) >= 4.5, "syntax {c:?} on well: {}", contrast(c, well));
        }
    }
}

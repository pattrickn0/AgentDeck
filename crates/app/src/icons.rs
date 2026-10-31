//! SVG icons from original interface, rasterized at runtime from the same
//! `path d="…"` as the HTML/JS (no image files). The rasterizer is a scanline
//! fill with nonzero winding rule and exact horizontal coverage (anti-aliasing),
//! sufficient for small, crisp icons at any scale.

use std::collections::HashMap;

use egui::{Color32, Mesh, Pos2, Rect, TextureHandle, TextureOptions, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// `#mic-cancel` (viewBox 24).
    Close,
    /// `#mic` (viewBox 24).
    Mic,
    /// `#attach` (viewBox 24).
    Attach,
    /// Drag and drop overlay cloud (viewBox 24).
    CloudUpload,
    /// File tree chevron (viewBox 16).
    TreeChevron,
    /// Generic file (viewBox 16).
    FileGeneric,
    /// "Git" tab and branch chip: Octicons `git-branch-16` (MIT, viewBox 16).
    GitBranch,
    /// "GitHub" tab: Octicons `mark-github-16` (MIT, viewBox 16).
    GitHub,
    /// Git: fetch. Material Icons `refresh` (Apache-2.0, viewBox 24).
    Refresh,
    /// Git: pull. Material Icons `arrow_downward` (Apache-2.0, viewBox 24).
    ArrowDown,
    /// Git: push. Material Icons `arrow_upward` (Apache-2.0, viewBox 24).
    ArrowUp,
    /// Git: discard changes. Material Icons `undo` (Apache-2.0, viewBox 24).
    Undo,
    /// Bring window to highlight. Material Icons `open_in_full` (Apache-2.0, viewBox 24).
    Expand,
    /// Claude logo: the "spark", orange asterisk (viewBox 24).
    ClaudeLogo,
    /// Google Antigravity logo (viewBox 112).
    AntigravityLogo,
    /// Codex / OpenAI logo (viewBox 24).
    CodexLogo,
    /// Chat view switch. Material Icons `chat_bubble_outline` (Apache-2.0, viewBox 24).
    ChatBubble,
    /// Terminal view switch: a window with a prompt. Material Icons `terminal` (Apache-2.0, viewBox 24).
    Terminal,
    /// Notifications (title bar). Material Icons `notifications_none` (Apache-2.0, viewBox 24).
    Bell,
    /// Holt brand: a "deck" of three cards (24×24 grid). Does not use `path d`: has its own
    /// colors from [`LOGO_CARDS`] and is rasterized by [`logo_rgba`]; `paint`'s `color` acts only as tint (white = as is).
    Holt,
}

impl Icon {
    fn path(self) -> (&'static str, f32) {
        match self {
            Icon::Close => ("M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12 19 6.41z", 24.0),
            Icon::Mic => ("M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2Z", 24.0),
            Icon::Attach => ("M16.5 6.5v10a4.5 4.5 0 0 1-9 0V5a3 3 0 0 1 6 0v10.5a1.5 1.5 0 0 1-3 0V6.5H9v9a3 3 0 0 0 6 0V5a4.5 4.5 0 0 0-9 0v11.5a6 6 0 0 0 12 0v-10h-1.5Z", 24.0),
            Icon::CloudUpload => ("M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z", 24.0),
            Icon::TreeChevron => ("M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06z", 16.0),
            Icon::FileGeneric => ("M9 1H3a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7L9 1zm4 12H3V3h5v5h5v5z", 16.0),
            Icon::GitBranch => ("M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25Zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z", 16.0),
            Icon::GitHub => ("M8 0c4.42 0 8 3.58 8 8a8.013 8.013 0 0 1-5.45 7.59c-.4.08-.55-.17-.55-.38 0-.27.01-1.13.01-2.2 0-.75-.25-1.23-.54-1.48 1.78-.2 3.65-.88 3.65-3.95 0-.88-.31-1.59-.82-2.15.08-.2.36-1.02-.08-2.12 0 0-.67-.22-2.2.82-.64-.18-1.32-.27-2-.27-.68 0-1.36.09-2 .27-1.53-1.03-2.2-.82-2.2-.82-.44 1.1-.16 1.92-.08 2.12-.51.56-.82 1.28-.82 2.15 0 3.06 1.86 3.75 3.64 3.95-.23.2-.44.55-.51 1.07-.46.21-1.61.55-2.33-.66-.15-.24-.6-.83-1.23-.82-.67.01-.27.38.01.53.34.19.73.9.82 1.13.16.45.68 1.31 2.69.94 0 .67.01 1.3.01 1.49 0 .21-.15.45-.55.38A7.995 7.995 0 0 1 0 8c0-4.42 3.58-8 8-8Z", 16.0),
            Icon::Refresh => ("M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z", 24.0),
            Icon::ArrowDown => ("m20 12-1.41-1.41L13 16.17V4h-2v12.17l-5.58-5.59L4 12l8 8 8-8z", 24.0),
            Icon::ArrowUp => ("m4 12 1.41 1.41L11 7.83V20h2V7.83l5.58 5.59L20 12l-8-8-8 8z", 24.0),
            Icon::Undo => ("M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8z", 24.0),
            Icon::Expand => ("M21 11V3h-8l3.29 3.29-10 10L3 13v8h8l-3.29-3.29 10-10L21 11z", 24.0),
            Icon::ClaudeLogo => ("m4.7144 15.9555 4.7174-2.6471.079-.2307-.079-.1275h-.2307l-.7893-.0486-2.6956-.0729-2.3375-.0971-2.2646-.1214-.5707-.1215-.5343-.7042.0546-.3522.4797-.3218.686.0608 1.5179.1032 2.2767.1578 1.6514.0972 2.4468.255h.3886l.0546-.1579-.1336-.0971-.1032-.0972L6.973 9.8356l-2.55-1.6879-1.3356-.9714-.7225-.4918-.3643-.4614-.1578-1.0078.6557-.7225.8803.0607.2246.0607.8925.686 1.9064 1.4754 2.4893 1.8336.3643.3035.1457-.1032.0182-.0728-.164-.2733-1.3539-2.4467-1.445-2.4893-.6435-1.032-.17-.6194c-.0607-.255-.1032-.4674-.1032-.7285L6.287.1335 6.6997 0l.9957.1336.419.3642.6192 1.4147 1.0018 2.2282 1.5543 3.0296.4553.8985.2429.8318.091.255h.1579v-.1457l.1275-1.706.2368-2.0947.2307-2.6957.0789-.7589.3764-.9107.7468-.4918.5828.2793.4797.686-.0668.4433-.2853 1.8517-.5586 2.9021-.3643 1.9429h.2125l.2429-.2429.9835-1.3053 1.6514-2.0643.7286-.8196.85-.9046.5464-.4311h1.0321l.759 1.1293-.34 1.1657-1.0625 1.3478-.8804 1.1414-1.2628 1.7-.7893 1.36.0729.1093.1882-.0183 2.8535-.607 1.5421-.2794 1.8396-.3157.8318.3886.091.3946-.3278.8075-1.967.4857-2.3072.4614-3.4364.8136-.0425.0304.0486.0607 1.5482.1457.6618.0364h1.621l3.0175.2247.7892.522.4736.6376-.079.4857-1.2142.6193-1.6393-.3886-3.825-.9107-1.3113-.3279h-.1822v.1093l1.0929 1.0686 2.0035 1.8092 2.5075 2.3314.1275.5768-.3218.4554-.34-.0486-2.2039-1.6575-.85-.7468-1.9246-1.621h-.1275v.17l.4432.6496 2.3436 3.5214.1214 1.0807-.17.3521-.6071.2125-.6679-.1214-1.3721-1.9246L14.38 17.959l-1.1414-1.9428-.1397.079-.674 7.2552-.3156.3703-.7286.2793-.6071-.4614-.3218-.7468.3218-1.4753.3886-1.9246.3157-1.53.2853-1.9004.17-.6314-.0121-.0425-.1397.0182-1.4328 1.9672-2.1796 2.9446-1.7243 1.8456-.4128.164-.7164-.3704.0667-.6618.4008-.5889 2.386-3.0357 1.4389-1.882.929-1.0868-.0062-.1579h-.0546l-6.3385 4.1164-1.1293.1457-.4857-.4554.0608-.7467.2307-.2429 1.9064-1.3114Z", 24.0),
            Icon::AntigravityLogo => ("M89.6992 93.695C94.3659 97.195 101.366 94.8617 94.9492 88.445C75.6992 69.7783 79.7825 18.445 55.8659 18.445C31.9492 18.445 36.0325 69.7783 16.7825 88.445C9.78251 95.445 17.3658 97.195 22.0325 93.695C40.1159 81.445 38.9492 59.8617 55.8659 59.8617C72.7825 59.8617 71.6159 81.445 89.6992 93.695Z", 112.0),
            Icon::CodexLogo => ("M22.2819 9.8211a5.9847 5.9847 0 0 0-.5157-4.9108 6.0462 6.0462 0 0 0-6.5098-2.9A6.0651 6.0651 0 0 0 4.9807 4.1818a5.9847 5.9847 0 0 0-3.9977 2.9 6.0462 6.0462 0 0 0 .7427 7.0966 5.98 5.98 0 0 0 .511 4.9107 6.051 6.051 0 0 0 6.5146 2.9001A5.9847 5.9847 0 0 0 13.2599 24a6.0557 6.0557 0 0 0 5.7718-4.2058 5.9894 5.9894 0 0 0 3.9977-2.9001 6.0557 6.0557 0 0 0-.7475-7.0729zm-9.022 12.6081a4.4755 4.4755 0 0 1-2.8764-1.0408l.1419-.0804 4.7783-2.7582a.7948.7948 0 0 0 .3927-.6813v-6.7369l2.02 1.1686a.071.071 0 0 1 .038.052v5.5826a4.504 4.504 0 0 1-4.4945 4.4944zm-9.6607-4.1254a4.4708 4.4708 0 0 1-.5346-3.0137l.142.0852 4.783 2.7582a.7712.7712 0 0 0 .7806 0l5.8428-3.3685v2.3324a.0804.0804 0 0 1-.0332.0615L9.74 19.9502a4.4992 4.4992 0 0 1-6.1408-1.6464zM2.3408 7.8956a4.485 4.485 0 0 1 2.3655-1.9728V11.6a.7664.7664 0 0 0 .3879.6765l5.8144 3.3543-2.0201 1.1685a.0757.0757 0 0 1-.071 0l-4.8303-2.7865A4.504 4.504 0 0 1 2.3408 7.872zm16.5963 3.8558L13.1038 8.364 15.1192 7.2a.0757.0757 0 0 1 .071 0l4.8303 2.7913a4.4944 4.4944 0 0 1-.6765 8.1042v-5.6772a.79.79 0 0 0-.407-.667zm2.0107-3.0231l-.142-.0852-4.7735-2.7818a.7759.7759 0 0 0-.7854 0L9.409 9.2297V6.8974a.0662.0662 0 0 1 .0284-.0615l4.8303-2.7866a4.4992 4.4992 0 0 1 6.6802 4.66zM8.3065 12.863l-2.02-1.1638a.0804.0804 0 0 1-.038-.0567V6.0742a4.4992 4.4992 0 0 1 7.3757-3.4537l-.142.0805L8.704 5.459a.7948.7948 0 0 0-.3927.6813zm1.0976-2.3654l2.602-1.4998 2.6069 1.4998v2.9994l-2.5974 1.4997-2.6067-1.4997Z", 24.0),
            Icon::ChatBubble => ("M20 2H4c-1.1 0-2 .9-2 2v18l4-4h14c1.1 0 2-.9 2-2V4c0-1.1-.9-2-2-2zm0 14H5.17L4 17.17V4h16v12z", 24.0),
            Icon::Terminal => ("M20 4H4c-1.11 0-2 .9-2 2v12c0 1.1.89 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H4V8h16v10zm-2-1h-6v-2h6v2zM7.5 17l-1.41-1.41L8.67 13l-2.59-2.59L7.5 9l4 4-4 4z", 24.0),
            Icon::Bell => ("M12 22c1.1 0 2-.9 2-2h-4c0 1.1.89 2 2 2zm6-6v-5c0-3.07-1.64-5.64-4.5-6.32V4c0-.83-.67-1.5-1.5-1.5s-1.5.67-1.5 1.5v.68C7.63 5.36 6 7.92 6 11v5l-2 2v1h16v-1l-2-2zm-2 1H8v-6c0-2.48 1.51-4.5 4-4.5s4 2.02 4 4.5v6z", 24.0),
            Icon::Holt => ("", 24.0),
        }
    }
}

/// Texture cache (icon × size in physical pixels).
#[derive(Default)]
pub struct IconCache {
    textures: HashMap<(Icon, u32), TextureHandle>,
    /// App brandmark, in color (by size in physical pixels).
    logos: HashMap<u32, TextureHandle>,
}

impl IconCache {
    /// Brandmark in color (premultiplied textures); white `tint` renders as is.
    fn paint_logo(&mut self, painter: &egui::Painter, center: Pos2, size: f32, tint: Color32) {
        let ppp = painter.pixels_per_point();
        let px = (size * ppp).round().max(1.0) as u32;
        let ctx = painter.ctx();
        let tex = self
            .logos
            .entry(px)
            .or_insert_with(|| {
                let rgba = logo_rgba(px as usize, crate::theme::pal().bg);
                let img = egui::ColorImage::from_rgba_unmultiplied([px as usize; 2], &rgba);
                ctx.load_texture(format!("icon-logo-{px}"), img, TextureOptions::LINEAR)
            })
            .id();
        let rect = Rect::from_center_size(center, Vec2::splat(px as f32 / ppp));
        let mut mesh = Mesh::with_texture(tex);
        mesh.add_rect_with_uv(rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), tint);
        painter.add(egui::Shape::mesh(mesh));
    }

    fn texture(&mut self, ctx: &egui::Context, icon: Icon, px: u32) -> &TextureHandle {
        self.textures.entry((icon, px)).or_insert_with(|| {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            let size = px.max(1) as usize;
            let cov = rasterize(&polys, vb, size, size);
            let pixels: Vec<Color32> = cov
                .iter()
                .map(|&c| {
                    let a = (c.clamp(0.0, 1.0) * 255.0).round() as u8;
                    Color32::from_rgba_premultiplied(a, a, a, a)
                })
                .collect();
            let img = egui::ColorImage::new([size, size], pixels);
            ctx.load_texture(format!("icon-{icon:?}-{px}"), img, TextureOptions::LINEAR)
        })
    }

    /// Paints the icon centered in `rect` (square of `size` points), tinted with `color`
    /// and rotated `angle` radians around center (for animated chevrons).
    pub fn paint(&mut self, painter: &egui::Painter, icon: Icon, center: Pos2, size: f32, color: Color32, angle: f32) {
        if icon == Icon::Holt {
            return self.paint_logo(painter, center, size, color);
        }
        let ppp = painter.pixels_per_point();
        let px = (size * ppp).round().max(1.0) as u32;
        let tex = self.texture(painter.ctx(), icon, px).id();
        let rect = Rect::from_center_size(center, Vec2::splat(px as f32 / ppp));
        let mut mesh = Mesh::with_texture(tex);
        mesh.add_rect_with_uv(rect, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), color);
        if angle != 0.0 {
            let rot = egui::emath::Rot2::from_angle(angle);
            for v in &mut mesh.vertices {
                v.pos = center + rot * (v.pos - center);
            }
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

// ---------------------------------------------------------------------------
// Holt brand: three cards in a deck and one hole (24×24 grid)
// ---------------------------------------------------------------------------

/// A card from the deck, on the brand's 24×24 grid.
pub struct LogoCard {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub r: f32,
    pub fill: Color32,
    /// Outline of [`LOGO_STROKE`] in background color (separates card from what's behind).
    pub outlined: bool,
}

/// Back to front: back, middle, front.
pub const LOGO_CARDS: [LogoCard; 3] = [
    LogoCard { x: 8.0, y: 2.5, w: 13.5, h: 10.0, r: 2.5, fill: crate::theme::DARK.text_faint, outlined: false },
    LogoCard { x: 5.25, y: 6.75, w: 13.5, h: 10.0, r: 2.5, fill: crate::theme::DARK.text_2, outlined: true },
    LogoCard { x: 2.5, y: 11.0, w: 13.5, h: 10.0, r: 2.5, fill: crate::theme::DARK.text, outlined: true },
];
/// Hole in front card: center and radius, in background color.
pub const LOGO_HOLE: (f32, f32, f32) = (6.75, 16.0, 1.7);
/// Stroke thickness (centered on border, like SVG `stroke`).
pub const LOGO_STROKE: f32 = 1.5;

/// Signed distance (negative inside) from `p` to rounded rectangle of card.
fn card_distance(p: (f32, f32), c: &LogoCard) -> f32 {
    let qx = (p.0 - (c.x + c.w / 2.0)).abs() - (c.w / 2.0 - c.r);
    let qy = (p.1 - (c.y + c.h / 2.0)).abs() - (c.h / 2.0 - c.r);
    qx.max(0.0).hypot(qy.max(0.0)) + qx.max(qy).min(0.0) - c.r
}

/// Brand color at point `p` on 24×24 grid (`None`: transparent background). `outline` is color of
/// card outline and hole.
fn logo_sample(p: (f32, f32), outline: Color32) -> Option<Color32> {
    let mut out = None;
    for c in &LOGO_CARDS {
        let d = card_distance(p, c);
        if c.outlined && d.abs() <= LOGO_STROKE / 2.0 {
            out = Some(outline);
        } else if d <= 0.0 {
            out = Some(c.fill);
        }
    }
    let (hx, hy, hr) = LOGO_HOLE;
    if (p.0 - hx).hypot(p.1 - hy) <= hr {
        out = Some(outline);
    }
    out
}

/// Brandmark rasterized at `size`×`size` pixels: unpremultiplied RGBA, transparent background, with
/// 4×4 samples per pixel for smooth edges. Same design as window icon and `.ico`.
pub fn logo_rgba(size: usize, outline: Color32) -> Vec<u8> {
    const N: usize = 4;
    let scale = 24.0 / size as f32;
    let mut out = Vec::with_capacity(size * size * 4);
    for y in 0..size {
        for x in 0..size {
            let (mut r, mut g, mut b, mut hits) = (0u32, 0u32, 0u32, 0u32);
            for sy in 0..N {
                for sx in 0..N {
                    let p = ((x as f32 + (sx as f32 + 0.5) / N as f32) * scale, (y as f32 + (sy as f32 + 0.5) / N as f32) * scale);
                    if let Some(c) = logo_sample(p, outline) {
                        r += c.r() as u32;
                        g += c.g() as u32;
                        b += c.b() as u32;
                        hits += 1;
                    }
                }
            }
            if hits == 0 {
                out.extend_from_slice(&[0, 0, 0, 0]);
            } else {
                let a = (hits as f32 / (N * N) as f32 * 255.0).round() as u8;
                out.extend_from_slice(&[(r / hits) as u8, (g / hits) as u8, (b / hits) as u8, a]);
            }
        }
    }
    out
}

/// Executable icon sizes (px).
#[cfg(test)]
const ICO_SIZES: [usize; 5] = [16, 24, 32, 48, 256];

/// Executable `.ico`: one PNG per size in [`ICO_SIZES`], with transparent background and outline
/// and hole in `#16181c`. Generated by `scripts/generate-icon.ps1`.
#[cfg(test)]
fn logo_ico() -> Vec<u8> {
    let images: Vec<Vec<u8>> = ICO_SIZES
        .iter()
        .map(|&s| {
            let mut png_bytes = Vec::new();
            let mut enc = png::Encoder::new(&mut png_bytes, s as u32, s as u32);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            let mut w = enc.write_header().expect("PNG header");
            w.write_image_data(&logo_rgba(s, crate::theme::pal().bg)).expect("PNG pixels");
            w.finish().expect("PNG end");
            png_bytes
        })
        .collect();
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(images.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * images.len();
    for (&s, img) in ICO_SIZES.iter().zip(&images) {
        let dim = if s >= 256 { 0 } else { s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0, 1, 0, 32, 0]);
        ico.extend_from_slice(&(img.len() as u32).to_le_bytes());
        ico.extend_from_slice(&(offset as u32).to_le_bytes());
        offset += img.len();
    }
    for img in &images {
        ico.extend_from_slice(img);
    }
    ico
}

// ---------------------------------------------------------------------------
// Parser for `path d` (M L H V C S Q T A Z, absolute and relative) → polygons.
// ---------------------------------------------------------------------------

type Poly = Vec<(f32, f32)>;

struct Tokens<'a> {
    s: &'a [u8],
    i: usize,
}

impl Tokens<'_> {
    fn skip_sep(&mut self) {
        while self.i < self.s.len() && (self.s[self.i].is_ascii_whitespace() || self.s[self.i] == b',') {
            self.i += 1;
        }
    }

    fn peek_cmd(&mut self) -> Option<u8> {
        self.skip_sep();
        self.s.get(self.i).copied().filter(|c| c.is_ascii_alphabetic() && *c != b'e' && *c != b'E')
    }

    fn at_number(&mut self) -> bool {
        self.skip_sep();
        matches!(self.s.get(self.i), Some(c) if c.is_ascii_digit() || *c == b'-' || *c == b'+' || *c == b'.')
    }

    /// Reads a number in SVG compact format (`.5.5`, `1.06-1.06`, `1e-3`).
    fn number(&mut self) -> Option<f32> {
        self.skip_sep();
        let start = self.i;
        let s = self.s;
        let mut i = self.i;
        if i < s.len() && (s[i] == b'-' || s[i] == b'+') {
            i += 1;
        }
        let mut seen_dot = false;
        let mut digits = 0;
        while i < s.len() {
            match s[i] {
                b'0'..=b'9' => digits += 1,
                b'.' if !seen_dot => seen_dot = true,
                _ => break,
            }
            i += 1;
        }
        if digits == 0 {
            return None;
        }
        if i < s.len() && (s[i] == b'e' || s[i] == b'E') {
            let mut j = i + 1;
            if j < s.len() && (s[j] == b'-' || s[j] == b'+') {
                j += 1;
            }
            if j < s.len() && s[j].is_ascii_digit() {
                while j < s.len() && s[j].is_ascii_digit() {
                    j += 1;
                }
                i = j;
            }
        }
        self.i = i;
        std::str::from_utf8(&s[start..i]).ok()?.parse().ok()
    }

    /// Arc flags may be adjacent (`0 01.5 2`): reads a single digit.
    fn flag(&mut self) -> Option<bool> {
        self.skip_sep();
        let c = *self.s.get(self.i)?;
        self.i += 1;
        match c {
            b'0' => Some(false),
            b'1' => Some(true),
            _ => None,
        }
    }
}

const CURVE_STEPS: usize = 16;

pub(crate) fn flatten_path(d: &str) -> Vec<Poly> {
    let mut t = Tokens { s: d.as_bytes(), i: 0 };
    let mut polys: Vec<Poly> = Vec::new();
    let mut cur: Poly = Vec::new();
    let (mut x, mut y) = (0.0f32, 0.0f32);
    let (mut sx, mut sy) = (0.0f32, 0.0f32);
    // Last control point (for S/T).
    let mut last_ctrl: Option<(f32, f32)> = None;
    let mut last_cmd = b' ';
    let mut cmd = b' ';

    let close = |cur: &mut Poly, polys: &mut Vec<Poly>| {
        if cur.len() > 2 {
            polys.push(std::mem::take(cur));
        } else {
            cur.clear();
        }
    };

    loop {
        if let Some(c) = t.peek_cmd() {
            cmd = c;
            t.i += 1;
        } else if !t.at_number() {
            break;
        } else if cmd == b'M' {
            cmd = b'L'; // extra pairs after M become L
        } else if cmd == b'm' {
            cmd = b'l';
        }
        let rel = cmd.is_ascii_lowercase();
        let (ox, oy) = if rel { (x, y) } else { (0.0, 0.0) };
        match cmd.to_ascii_uppercase() {
            b'M' => {
                let (Some(nx), Some(ny)) = (t.number(), t.number()) else { break };
                close(&mut cur, &mut polys);
                x = ox + nx;
                y = oy + ny;
                sx = x;
                sy = y;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'L' => {
                let (Some(nx), Some(ny)) = (t.number(), t.number()) else { break };
                x = ox + nx;
                y = oy + ny;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'H' => {
                let Some(nx) = t.number() else { break };
                x = ox + nx;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'V' => {
                let Some(ny) = t.number() else { break };
                y = oy + ny;
                cur.push((x, y));
                last_ctrl = None;
            }
            b'C' | b'S' => {
                let (c1x, c1y) = if cmd.eq_ignore_ascii_case(&b'C') {
                    let (Some(a), Some(b)) = (t.number(), t.number()) else { break };
                    (ox + a, oy + b)
                } else {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), b'C' | b'S') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                };
                let (Some(a), Some(b), Some(ex), Some(ey)) = (t.number(), t.number(), t.number(), t.number()) else { break };
                let (c2x, c2y) = (ox + a, oy + b);
                let (ex, ey) = (ox + ex, oy + ey);
                for i in 1..=CURVE_STEPS {
                    let u = i as f32 / CURVE_STEPS as f32;
                    let m = 1.0 - u;
                    let px = m * m * m * x + 3.0 * m * m * u * c1x + 3.0 * m * u * u * c2x + u * u * u * ex;
                    let py = m * m * m * y + 3.0 * m * m * u * c1y + 3.0 * m * u * u * c2y + u * u * u * ey;
                    cur.push((px, py));
                }
                last_ctrl = Some((c2x, c2y));
                x = ex;
                y = ey;
            }
            b'Q' | b'T' => {
                let (cx, cy) = if cmd.eq_ignore_ascii_case(&b'Q') {
                    let (Some(a), Some(b)) = (t.number(), t.number()) else { break };
                    (ox + a, oy + b)
                } else {
                    match (last_ctrl, last_cmd.to_ascii_uppercase()) {
                        (Some((lx, ly)), b'Q' | b'T') => (2.0 * x - lx, 2.0 * y - ly),
                        _ => (x, y),
                    }
                };
                let (Some(ex), Some(ey)) = (t.number(), t.number()) else { break };
                let (ex, ey) = (ox + ex, oy + ey);
                for i in 1..=CURVE_STEPS {
                    let u = i as f32 / CURVE_STEPS as f32;
                    let m = 1.0 - u;
                    cur.push((m * m * x + 2.0 * m * u * cx + u * u * ex, m * m * y + 2.0 * m * u * cy + u * u * ey));
                }
                last_ctrl = Some((cx, cy));
                x = ex;
                y = ey;
            }
            b'A' => {
                let (Some(rx), Some(ry), Some(rot)) = (t.number(), t.number(), t.number()) else { break };
                let (Some(large), Some(sweep)) = (t.flag(), t.flag()) else { break };
                let (Some(ex), Some(ey)) = (t.number(), t.number()) else { break };
                let (ex, ey) = (ox + ex, oy + ey);
                arc_points(&mut cur, (x, y), (ex, ey), rx, ry, rot, large, sweep);
                x = ex;
                y = ey;
                last_ctrl = None;
            }
            b'Z' => {
                close(&mut cur, &mut polys);
                x = sx;
                y = sy;
                cur.push((x, y));
                last_ctrl = None;
            }
            _ => break,
        }
        last_cmd = cmd;
    }
    close(&mut cur, &mut polys);
    polys
}

/// Converts elliptical arc (endpoint parameterization, SVG 1.1 F.6.5) to segments.
#[allow(clippy::too_many_arguments)]
fn arc_points(out: &mut Poly, p0: (f32, f32), p1: (f32, f32), rx: f32, ry: f32, rot_deg: f32, large: bool, sweep: bool) {
    if p0 == p1 {
        return;
    }
    let (mut rx, mut ry) = (rx.abs(), ry.abs());
    if rx == 0.0 || ry == 0.0 {
        out.push(p1);
        return;
    }
    let phi = rot_deg.to_radians();
    let (sin_p, cos_p) = phi.sin_cos();
    let dx = (p0.0 - p1.0) / 2.0;
    let dy = (p0.1 - p1.1) / 2.0;
    let x1 = cos_p * dx + sin_p * dy;
    let y1 = -sin_p * dx + cos_p * dy;
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        let s = lambda.sqrt();
        rx *= s;
        ry *= s;
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut coef = if den == 0.0 { 0.0 } else { (num / den).max(0.0).sqrt() };
    if large == sweep {
        coef = -coef;
    }
    let cx1 = coef * rx * y1 / ry;
    let cy1 = -coef * ry * x1 / rx;
    let cx = cos_p * cx1 - sin_p * cy1 + (p0.0 + p1.0) / 2.0;
    let cy = sin_p * cx1 + cos_p * cy1 + (p0.1 + p1.1) / 2.0;
    let angle = |ux: f32, uy: f32, vx: f32, vy: f32| {
        let dot = ux * vx + uy * vy;
        let len = (ux * ux + uy * uy).sqrt() * (vx * vx + vy * vy).sqrt();
        let mut a = (dot / len).clamp(-1.0, 1.0).acos();
        if ux * vy - uy * vx < 0.0 {
            a = -a;
        }
        a
    };
    let ux = (x1 - cx1) / rx;
    let uy = (y1 - cy1) / ry;
    let vx = (-x1 - cx1) / rx;
    let vy = (-y1 - cy1) / ry;
    let theta1 = angle(1.0, 0.0, ux, uy);
    let mut dtheta = angle(ux, uy, vx, vy);
    let tau = std::f32::consts::TAU;
    if !sweep && dtheta > 0.0 {
        dtheta -= tau;
    } else if sweep && dtheta < 0.0 {
        dtheta += tau;
    }
    let steps = ((dtheta.abs() / tau) * 48.0).ceil().max(4.0) as usize;
    for i in 1..=steps {
        let th = theta1 + dtheta * i as f32 / steps as f32;
        let (s, c) = th.sin_cos();
        out.push((cos_p * rx * c - sin_p * ry * s + cx, sin_p * rx * c + cos_p * ry * s + cy));
    }
}

/// Rasterizes polygons (coordinates in viewBox `vb`×`vb`) onto a `w`×`h` grid with coverage 0..1.
pub(crate) fn rasterize(polys: &[Poly], vb: f32, w: usize, h: usize) -> Vec<f32> {
    const SUB: usize = 5; // sub-lines per pixel
    let sx = w as f32 / vb;
    let sy = h as f32 / vb;
    let mut edges: Vec<(f32, f32, f32, f32)> = Vec::new();
    for p in polys {
        for i in 0..p.len() {
            let a = p[i];
            let b = p[(i + 1) % p.len()];
            if a.1 != b.1 {
                edges.push((a.0 * sx, a.1 * sy, b.0 * sx, b.1 * sy));
            }
        }
    }
    let mut cov = vec![0.0f32; w * h];
    let mut xs: Vec<(f32, i32)> = Vec::new();
    let weight = 1.0 / SUB as f32;
    for py in 0..h {
        let row = &mut cov[py * w..(py + 1) * w];
        for s in 0..SUB {
            let y = py as f32 + (s as f32 + 0.5) / SUB as f32;
            xs.clear();
            for &(x0, y0, x1, y1) in &edges {
                let (lo, hi) = if y0 < y1 { (y0, y1) } else { (y1, y0) };
                if y < lo || y >= hi {
                    continue;
                }
                let t = (y - y0) / (y1 - y0);
                xs.push((x0 + t * (x1 - x0), if y1 > y0 { 1 } else { -1 }));
            }
            xs.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut winding = 0;
            let mut start = 0.0;
            for &(x, dir) in &xs {
                let before = winding;
                winding += dir;
                if before == 0 && winding != 0 {
                    start = x;
                } else if before != 0 && winding == 0 {
                    add_span(row, start, x, weight);
                }
            }
        }
    }
    cov
}

fn add_span(row: &mut [f32], x0: f32, x1: f32, weight: f32) {
    let w = row.len() as f32;
    let (x0, x1) = (x0.clamp(0.0, w), x1.clamp(0.0, w));
    if x1 <= x0 {
        return;
    }
    let i0 = x0.floor() as usize;
    let i1 = x1.floor() as usize;
    if i0 == i1 {
        row[i0.min(row.len() - 1)] += (x1 - x0) * weight;
        return;
    }
    row[i0] += (i0 as f32 + 1.0 - x0) * weight;
    for v in row.iter_mut().take(i1).skip(i0 + 1) {
        *v += weight;
    }
    if i1 < row.len() {
        row[i1] += (x1 - i1 as f32) * weight;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_compact_numbers() {
        let mut t = Tokens { s: b".5.5-1.06-1.06 1e1,2", i: 0 };
        let v: Vec<f32> = std::iter::from_fn(|| t.number()).collect();
        assert_eq!(v, vec![0.5, 0.5, -1.06, -1.06, 10.0, 2.0]);
    }

    #[test]
    fn square_fills_fully() {
        let polys = flatten_path("M0 0H16V16H0z");
        let cov = rasterize(&polys, 16.0, 8, 8);
        assert!(cov.iter().all(|c| (c - 1.0).abs() < 1e-3), "{cov:?}");
    }

    #[test]
    fn hole_with_nonzero_rule() {
        // Outer clockwise square + inner counterclockwise square = frame (hole in the middle).
        let polys = flatten_path("M0 0H16V16H0zM4 4V12H12V4z");
        let cov = rasterize(&polys, 16.0, 16, 16);
        assert!(cov[8 * 16 + 8] < 0.01);
        assert!(cov[16 + 1] > 0.99);
    }

    #[test]
    fn every_icon_rasterizes() {
        for icon in [
            Icon::Close,
            Icon::Mic,
            Icon::Attach,
            Icon::CloudUpload,
            Icon::TreeChevron,
            Icon::FileGeneric,
            Icon::GitBranch,
            Icon::GitHub,
            Icon::Refresh,
            Icon::ArrowDown,
            Icon::ArrowUp,
            Icon::Undo,
            Icon::Expand,
            Icon::ClaudeLogo,
            Icon::AntigravityLogo,
            Icon::CodexLogo,
            Icon::ChatBubble,
            Icon::Terminal,
        ] {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            assert!(!polys.is_empty(), "{icon:?}");
            let cov = rasterize(&polys, vb, 24, 24);
            let total: f32 = cov.iter().sum();
            assert!(total > 10.0, "{icon:?} almost empty: {total}");
            assert!(cov.iter().all(|c| *c <= 1.0001), "{icon:?} coverage > 1");
        }
    }

    #[test]
    fn agent_logos_rasterize_cleanly() {
        for (icon, name) in [(Icon::ClaudeLogo, "Claude"), (Icon::AntigravityLogo, "Antigravity"), (Icon::CodexLogo, "Codex")] {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            assert!(!polys.is_empty(), "{name} has no polygons");
            let cov = rasterize(&polys, vb, 24, 24);
            let total: f32 = cov.iter().sum();
            assert!(total > 20.0 && total < 400.0, "{name} unexpected coverage: {total}");
        }
    }

    fn pixel(rgba: &[u8], size: usize, gx: f32, gy: f32) -> [u8; 4] {
        let (x, y) = ((gx / 24.0 * size as f32) as usize, (gy / 24.0 * size as f32) as usize);
        let i = (y * size + x) * 4;
        [rgba[i], rgba[i + 1], rgba[i + 2], rgba[i + 3]]
    }

    #[test]
    fn the_logo_is_three_cards_and_a_hole() {
        let bg = crate::theme::pal().bg;
        let rgba = logo_rgba(96, bg);
        let at = |x, y| pixel(&rgba, 96, x, y);
        // Each card in its color (in areas not covered by card in front).
        assert_eq!(at(19.0, 4.0), [0x6b, 0x71, 0x7c, 255], "back card");
        assert_eq!(at(17.0, 9.0), [0xaa, 0xb1, 0xbb, 255], "middle card");
        assert_eq!(at(14.0, 19.5), [0xe6, 0xe8, 0xeb, 255], "front card");
        // Hole and outline use background color.
        assert_eq!(at(6.75, 16.0), [bg.r(), bg.g(), bg.b(), 255], "hole");
        assert_eq!(at(10.0, 11.0), [bg.r(), bg.g(), bg.b(), 255], "front card outline");
        // Outside the cards, background is transparent.
        assert_eq!(at(1.0, 1.0)[3], 0);
        assert_eq!(at(22.5, 22.5)[3], 0);
    }

    #[test]
    fn the_logo_survives_the_small_sizes() {
        for size in [16, 24, 32] {
            let rgba = logo_rgba(size, crate::theme::pal().bg);
            assert_eq!(rgba.len(), size * size * 4);
            let covered = rgba.chunks(4).filter(|p| p[3] > 0).count();
            assert!(covered > size * size / 3, "{size}px almost empty: {covered}");
        }
    }

    /// The committed `.ico` has the five sizes and is what `logo_ico` generates today.
    #[test]
    fn the_committed_ico_has_the_five_sizes() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/holt.ico");
        let ico = std::fs::read(path).expect("crates/app/assets/holt.ico (run scripts/generate-icon.ps1)");
        assert_eq!(&ico[..4], &[0, 0, 1, 0]);
        assert_eq!(u16::from_le_bytes([ico[4], ico[5]]) as usize, ICO_SIZES.len());
        for (i, &s) in ICO_SIZES.iter().enumerate() {
            let entry = &ico[6 + 16 * i..6 + 16 * (i + 1)];
            let dim = if s >= 256 { 0 } else { s as u8 };
            assert_eq!((entry[0], entry[1]), (dim, dim), "size {s}");
        }
        assert_eq!(ico, logo_ico(), "the .ico is outdated: run scripts/generate-icon.ps1");
    }

    /// The 256 px image inside the `.ico`: the Linux icon (`assets/holt.png`, used by the `.desktop` entry).
    fn largest_png(ico: &[u8]) -> &[u8] {
        let entry = &ico[6 + 16 * (ICO_SIZES.len() - 1)..6 + 16 * ICO_SIZES.len()];
        let size = u32::from_le_bytes([entry[8], entry[9], entry[10], entry[11]]) as usize;
        let offset = u32::from_le_bytes([entry[12], entry[13], entry[14], entry[15]]) as usize;
        &ico[offset..offset + size]
    }

    #[test]
    fn the_committed_png_is_the_largest_ico_image() {
        let path = concat!(env!("CARGO_MANIFEST_DIR"), "/assets/holt.png");
        let png = std::fs::read(path).expect("crates/app/assets/holt.png (run scripts/generate-icon.ps1)");
        assert_eq!(png, largest_png(&logo_ico()), "the .png is outdated: run scripts/generate-icon.ps1");
    }

    /// Generates `assets/holt.ico` and `assets/holt.png`. Does not run automatically: `scripts/generate-icon.ps1`.
    #[test]
    #[ignore = "generates the files; use scripts/generate-icon.ps1"]
    fn write_the_ico_file() {
        let ico = logo_ico();
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/holt.ico"), &ico).expect("write the .ico");
        std::fs::write(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/holt.png"), largest_png(&ico)).expect("write the .png");
    }

    #[test]
    fn the_branch_icon_keeps_its_holes() {
        // Octicons circles are rings: the center (hole) stays empty under the nonzero rule.
        let (d, vb) = Icon::GitBranch.path();
        let cov = rasterize(&flatten_path(d), vb, 64, 64);
        let at = |x: f32, y: f32| cov[(y / vb * 64.0) as usize * 64 + (x / vb * 64.0) as usize];
        assert!(at(4.25, 3.25) < 0.3, "top-left circle hole");
        assert!(at(11.75, 3.25) < 0.3, "right circle hole");
        assert!(at(4.25, 12.75) < 0.3, "bottom circle hole");
        assert!(at(4.25, 8.0) > 0.7, "line between circles is solid");
    }
}

//! Ícones SVG da interface original, rasterizados em tempo de execução a partir dos
//! mesmos `path d="…"` do HTML/JS (sem arquivos de imagem). O rasterizador é um
//! preenchimento por varredura com regra "nonzero" e cobertura horizontal exata
//! (anti-aliasing), suficiente para ícones pequenos e nítidos em qualquer escala.

use std::collections::HashMap;

use egui::{Color32, Mesh, Pos2, Rect, TextureHandle, TextureOptions, Vec2};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Icon {
    /// `#sidebar-toggle-icon` (viewBox 16).
    SidebarChevron,
    /// Engrenagem de `#settings-btn` (viewBox 24).
    Gear,
    /// Ícone do estado vazio (viewBox 24).
    TerminalBox,
    /// `#mic-cancel` (viewBox 24).
    Close,
    /// `#mic` (viewBox 24).
    Mic,
    /// `#attach` (viewBox 24).
    Attach,
    /// Nuvem do overlay de arrastar e soltar (viewBox 24).
    CloudUpload,
    /// Chevron da árvore de arquivos (viewBox 16).
    TreeChevron,
    /// Pasta aberta (viewBox 16).
    FolderOpen,
    /// Pasta fechada (viewBox 16).
    FolderClosed,
    /// Arquivo genérico (viewBox 16).
    FileGeneric,
    /// Aba "Conversa integrada": Material Icons `forum` (Apache-2.0, viewBox 24).
    Chat,
    /// Aba "Mapa de agentes": Material Icons `account_tree` (Apache-2.0, viewBox 24).
    Diagram,
    /// Aba "Git" e chip da branch: Octicons `git-branch-16` (MIT, viewBox 16).
    GitBranch,
    /// Git: buscar (fetch). Material Icons `refresh` (Apache-2.0, viewBox 24).
    Refresh,
    /// Git: puxar (pull). Material Icons `arrow_downward` (Apache-2.0, viewBox 24).
    ArrowDown,
    /// Git: enviar (push). Material Icons `arrow_upward` (Apache-2.0, viewBox 24).
    ArrowUp,
    /// Git: descartar alterações. Material Icons `undo` (Apache-2.0, viewBox 24).
    Undo,
}

impl Icon {
    fn path(self) -> (&'static str, f32) {
        match self {
            Icon::SidebarChevron => ("M10.354 3.646a.5.5 0 0 1 0 .708L6.707 8l3.647 3.646a.5.5 0 0 1-.708.708l-4-4a.5.5 0 0 1 0-.708l4-4a.5.5 0 0 1 .708 0z", 16.0),
            Icon::Gear => ("M19.14 12.94c.04-.3.06-.61.06-.94 0-.32-.02-.64-.07-.94l2.03-1.58a.49.49 0 0 0 .12-.61l-1.92-3.32a.488.488 0 0 0-.59-.22l-2.39.96c-.5-.38-1.03-.7-1.62-.94l-.36-2.54a.484.484 0 0 0-.48-.41h-3.84c-.24 0-.43.17-.47.41l-.36 2.54c-.59.24-1.13.57-1.62.94l-2.39-.96c-.22-.08-.47 0-.59.22L2.74 8.87c-.12.21-.08.47.12.61l2.03 1.58c-.05.3-.09.63-.09.94s.02.64.07.94l-2.03 1.58a.49.49 0 0 0-.12.61l1.92 3.32c.12.22.37.29.59.22l2.39-.96c.5.38 1.03.7 1.62.94l.36 2.54c.05.24.24.41.48.41h3.84c.24 0 .44-.17.47-.41l.36-2.54c.59-.24 1.13-.56 1.62-.94l2.39.96c.22.08.47 0 .59-.22l1.92-3.32c.12-.22.07-.47-.12-.61l-2.01-1.58zM12 15.6c-1.98 0-3.6-1.62-3.6-3.6s1.62-3.6 3.6-3.6 3.6 1.62 3.6 3.6-1.62 3.6-3.6 3.6z", 24.0),
            Icon::TerminalBox => ("M20 4H4c-1.1 0-2 .9-2 2v12c0 1.1.9 2 2 2h16c1.1 0 2-.9 2-2V6c0-1.1-.9-2-2-2zm0 14H4V8h16v10zm-2-1h-4v-2h4v2zm-6.29-2.71a.996.996 0 0 0 0-1.41L9.42 11.6l2.3-2.3a.996.996 0 1 0-1.41-1.41L7.59 10.6c-.39.39-.39 1.02 0 1.41l2.71 2.71c.39.39 1.02.39 1.41-.01z", 24.0),
            Icon::Close => ("M19 6.41L17.59 5 12 10.59 6.41 5 5 6.41 10.59 12 5 17.59 6.41 19 12 13.41 17.59 19 19 17.59 13.41 12 19 6.41z", 24.0),
            Icon::Mic => ("M12 14a3 3 0 0 0 3-3V5a3 3 0 0 0-6 0v6a3 3 0 0 0 3 3Zm5-3a5 5 0 0 1-10 0H5a7 7 0 0 0 6 6.92V21h2v-3.08A7 7 0 0 0 19 11h-2Z", 24.0),
            Icon::Attach => ("M16.5 6.5v10a4.5 4.5 0 0 1-9 0V5a3 3 0 0 1 6 0v10.5a1.5 1.5 0 0 1-3 0V6.5H9v9a3 3 0 0 0 6 0V5a4.5 4.5 0 0 0-9 0v11.5a6 6 0 0 0 12 0v-10h-1.5Z", 24.0),
            Icon::CloudUpload => ("M19.35 10.04C18.67 6.59 15.64 4 12 4 9.11 4 6.6 5.64 5.35 8.04 2.34 8.36 0 10.91 0 14c0 3.31 2.69 6 6 6h13c2.76 0 5-2.24 5-5 0-2.64-2.05-4.78-4.65-4.96zM14 13v4h-4v-4H7l5-5 5 5h-3z", 24.0),
            Icon::TreeChevron => ("M6.22 3.22a.75.75 0 0 1 1.06 0l4.25 4.25a.75.75 0 0 1 0 1.06l-4.25 4.25a.75.75 0 0 1-1.06-1.06L9.94 8 6.22 4.28a.75.75 0 0 1 0-1.06z", 16.0),
            Icon::FolderOpen => ("M1.5 2A1.5 1.5 0 0 0 0 3.5v9A1.5 1.5 0 0 0 1.5 14h13a1.5 1.5 0 0 0 1.5-1.5v-7A1.5 1.5 0 0 0 14.5 4H7.414L5.707 2.293A1 1 0 0 0 5 2H1.5zm0 1H5l1.707 1.707A1 1 0 0 0 7.414 5H14.5a.5.5 0 0 1 .5.5v1.25H1V3.5a.5.5 0 0 1 .5-.5z", 16.0),
            Icon::FolderClosed => ("M14.5 3H7.71l-.85-.85L6.51 2h-5l-.5.5v11l.5.5h13l.5-.5v-10l-.5-.5zm-.5 10H2V3h4.29l.86.85.35.15H14v9z", 16.0),
            Icon::FileGeneric => ("M9 1H3a2 2 0 0 0-2 2v10a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V7L9 1zm4 12H3V3h5v5h5v5z", 16.0),
            Icon::Chat => ("M21 6h-2v9H6v2c0 .55.45 1 1 1h11l4 4V7c0-.55-.45-1-1-1zm-4 6V3c0-.55-.45-1-1-1H3c-.55 0-1 .45-1 1v14l4-4h10c.55 0 1-.45 1-1z", 24.0),
            Icon::Diagram => ("M22 11V3h-7v3H9V3H2v8h7V8h2v10h4v3h7v-8h-7v3h-2V8h2v3z", 24.0),
            Icon::GitBranch => ("M9.5 3.25a2.25 2.25 0 1 1 3 2.122V6A2.5 2.5 0 0 1 10 8.5H6a1 1 0 0 0-1 1v1.128a2.251 2.251 0 1 1-1.5 0V5.372a2.25 2.25 0 1 1 1.5 0v1.836A2.493 2.493 0 0 1 6 7h4a1 1 0 0 0 1-1v-.628A2.25 2.25 0 0 1 9.5 3.25Zm-6 0a.75.75 0 1 0 1.5 0 .75.75 0 0 0-1.5 0Zm8.25-.75a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5ZM4.25 12a.75.75 0 1 0 0 1.5.75.75 0 0 0 0-1.5Z", 16.0),
            Icon::Refresh => ("M17.65 6.35A7.958 7.958 0 0 0 12 4c-4.42 0-7.99 3.58-7.99 8s3.57 8 7.99 8c3.73 0 6.84-2.55 7.73-6h-2.08A5.99 5.99 0 0 1 12 18c-3.31 0-6-2.69-6-6s2.69-6 6-6c1.66 0 3.14.69 4.22 1.78L13 11h7V4l-2.35 2.35z", 24.0),
            Icon::ArrowDown => ("m20 12-1.41-1.41L13 16.17V4h-2v12.17l-5.58-5.59L4 12l8 8 8-8z", 24.0),
            Icon::ArrowUp => ("m4 12 1.41 1.41L11 7.83V20h2V7.83l5.58 5.59L20 12l-8-8-8 8z", 24.0),
            Icon::Undo => ("M12.5 8c-2.65 0-5.05.99-6.9 2.6L2 7v9h9l-3.62-3.62c1.39-1.16 3.16-1.88 5.12-1.88 3.54 0 6.55 2.31 7.6 5.5l2.37-.78C21.08 11.03 17.15 8 12.5 8z", 24.0),
        }
    }
}

/// Cache de texturas (ícone × tamanho em pixels físicos).
#[derive(Default)]
pub struct IconCache {
    textures: HashMap<(Icon, u32), TextureHandle>,
}

impl IconCache {
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

    /// Pinta o ícone centralizado em `rect` (quadrado de lado `size` pontos), tingido com `color`
    /// e girado `angle` radianos em torno do centro (para os chevrons animados).
    pub fn paint(&mut self, painter: &egui::Painter, icon: Icon, center: Pos2, size: f32, color: Color32, angle: f32) {
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
// Parser de `path d` (M L H V C S Q T A Z, absolutos e relativos) → polígonos.
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

    /// Lê um número no formato compacto do SVG (`.5.5`, `1.06-1.06`, `1e-3`).
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

    /// Flags de arco podem vir coladas (`0 01.5 2`): lê um único dígito.
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
    // Último ponto de controle (para S/T).
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
            cmd = b'L'; // pares extras após M viram L
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

/// Converte um arco elíptico (parametrização por extremos, SVG 1.1 F.6.5) em segmentos.
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

/// Rasteriza os polígonos (coordenadas no viewBox `vb`×`vb`) numa grade `w`×`h` de cobertura 0..1.
pub(crate) fn rasterize(polys: &[Poly], vb: f32, w: usize, h: usize) -> Vec<f32> {
    const SUB: usize = 5; // sub-linhas por pixel
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
        // Quadrado externo horário + interno anti-horário = moldura (furo no meio).
        let polys = flatten_path("M0 0H16V16H0zM4 4V12H12V4z");
        let cov = rasterize(&polys, 16.0, 16, 16);
        assert!(cov[8 * 16 + 8] < 0.01);
        assert!(cov[16 + 1] > 0.99);
    }

    #[test]
    fn every_icon_rasterizes() {
        for icon in [
            Icon::SidebarChevron,
            Icon::Gear,
            Icon::TerminalBox,
            Icon::Close,
            Icon::Mic,
            Icon::Attach,
            Icon::CloudUpload,
            Icon::TreeChevron,
            Icon::FolderOpen,
            Icon::FolderClosed,
            Icon::FileGeneric,
            Icon::Chat,
            Icon::Diagram,
            Icon::GitBranch,
            Icon::Refresh,
            Icon::ArrowDown,
            Icon::ArrowUp,
            Icon::Undo,
        ] {
            let (d, vb) = icon.path();
            let polys = flatten_path(d);
            assert!(!polys.is_empty(), "{icon:?}");
            let cov = rasterize(&polys, vb, 24, 24);
            let total: f32 = cov.iter().sum();
            assert!(total > 10.0, "{icon:?} quase vazio: {total}");
            assert!(cov.iter().all(|c| *c <= 1.0001), "{icon:?} cobertura > 1");
        }
    }

    #[test]
    fn the_branch_icon_keeps_its_holes() {
        // Os círculos do Octicons são anéis: o centro (furo) fica vazio com a regra nonzero.
        let (d, vb) = Icon::GitBranch.path();
        let cov = rasterize(&flatten_path(d), vb, 64, 64);
        let at = |x: f32, y: f32| cov[(y / vb * 64.0) as usize * 64 + (x / vb * 64.0) as usize];
        assert!(at(4.25, 3.25) < 0.3, "furo do círculo de cima à esquerda");
        assert!(at(11.75, 3.25) < 0.3, "furo do círculo da direita");
        assert!(at(4.25, 12.75) < 0.3, "furo do círculo de baixo");
        assert!(at(4.25, 8.0) > 0.7, "o traço entre os círculos é cheio");
    }
}

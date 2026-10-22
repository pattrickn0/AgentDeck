//! Fundo da janela: `bg` com dois brilhos radiais suaves (o egui não tem gradiente radial, então
//! cada brilho é um leque de triângulos com a cor no centro e transparente na borda).

use egui::epaint::{Mesh, Vertex, WHITE_UV};
use egui::{Color32, Painter, Pos2, Rect, Vec2};

use crate::theme;

/// Até onde o brilho vai, como fração do raio: `transparent 70%` do protótipo.
const FADE_AT: f32 = 0.7;
/// Lados do polígono que aproxima a elipse.
const SEGMENTS: usize = 64;

/// Um brilho radial: cor (com o alfa do centro) que some linearmente até `FADE_AT` do raio.
struct Glow {
    center: Pos2,
    radii: Vec2,
    color: Color32,
}

impl Glow {
    /// Alfa (0..=1) do brilho em `pos`, em fração do alfa do centro.
    fn strength(&self, pos: Pos2) -> f32 {
        let t = ((pos.x - self.center.x) / self.radii.x).hypot((pos.y - self.center.y) / self.radii.y);
        (1.0 - t / FADE_AT).max(0.0)
    }
}

/// Os brilhos de `window`: o de cima à esquerda na cor de destaque (10%) e o de baixo à direita branco (3,5%).
fn glows(window: Rect, accent: Color32) -> [Glow; 2] {
    [
        Glow {
            center: Pos2::new(window.min.x + window.width() * 0.06, window.min.y),
            radii: Vec2::new(900.0, 520.0),
            color: theme::with_alpha(accent, 0.10),
        },
        Glow { center: window.max, radii: Vec2::new(800.0, 600.0), color: Color32::from_white_alpha(9) },
    ]
}

/// A cor do fundo (já com os brilhos) em `pos`. Serve para repintar os cantos das ilhas.
pub fn color_at(pos: Pos2, window: Rect, accent: Color32) -> Color32 {
    let mut c = theme::pal().bg;
    // O de baixo à direita fica por baixo do de cima à esquerda (como a ordem dos gradientes no CSS).
    for g in glows(window, accent).iter().rev() {
        let a = g.strength(pos) * g.color.a() as f32 / 255.0;
        c = theme::over(theme::with_alpha(g.color, a), c);
    }
    c
}

/// Pinta o fundo e os dois brilhos. `accent` é a cor do agente ativo.
pub fn draw(painter: &Painter, window: Rect, accent: Color32) {
    painter.rect_filled(window, 0.0, theme::pal().bg);
    for g in glows(window, accent).iter().rev() {
        let mut mesh = Mesh::default();
        let mut vertex = |pos: Pos2, color: Color32| {
            mesh.vertices.push(Vertex { pos, uv: WHITE_UV, color });
            mesh.vertices.len() as u32 - 1
        };
        let center = vertex(g.center, g.color);
        let r = g.radii * FADE_AT;
        let rim: Vec<u32> = (0..SEGMENTS)
            .map(|i| {
                let th = i as f32 / SEGMENTS as f32 * std::f32::consts::TAU;
                vertex(g.center + Vec2::new(r.x * th.cos(), r.y * th.sin()), Color32::TRANSPARENT)
            })
            .collect();
        for i in 0..SEGMENTS {
            mesh.add_triangle(center, rim[i], rim[(i + 1) % SEGMENTS]);
        }
        painter.add(egui::Shape::mesh(mesh));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window() -> Rect {
        Rect::from_min_size(Pos2::ZERO, Vec2::new(1440.0, 900.0))
    }

    #[test]
    fn the_top_left_glow_peaks_at_the_corner_in_the_accent_color() {
        let bg = theme::pal().bg;
        let at_center = color_at(Pos2::new(1440.0 * 0.06, 0.0), window(), theme::CLAUDE);
        // 10% de laranja sobre o fundo.
        let [r, g, b, _] = at_center.to_srgba_unmultiplied();
        assert!(r > bg.r() + 10 && g >= bg.g() && b <= bg.b() + 8, "{at_center:?}");
        // Longe dos dois brilhos, o fundo é o `bg`.
        assert_eq!(color_at(Pos2::new(720.0, 450.0), window(), theme::CLAUDE), bg);
    }

    #[test]
    fn the_glow_fades_out_before_the_edge_of_the_radius() {
        let w = window();
        let near = color_at(Pos2::new(86.0 + 100.0, 0.0), w, theme::ANTIGRAVITY);
        let far = color_at(Pos2::new(86.0 + 900.0 * FADE_AT + 1.0, 0.0), w, theme::ANTIGRAVITY);
        assert_ne!(near, theme::pal().bg);
        assert_eq!(far, theme::pal().bg);
    }

    #[test]
    fn the_bottom_right_glow_is_a_faint_white() {
        let bg = theme::pal().bg;
        let c = color_at(w_max(), window(), theme::CLAUDE);
        assert!(c.r() > bg.r() && c.r() < bg.r() + 12, "{c:?}");
        assert_eq!(c.r() as i32 - bg.r() as i32, c.b() as i32 - bg.b() as i32);
    }

    fn w_max() -> Pos2 {
        window().max
    }
}

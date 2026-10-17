//! Pintura da interface (equivalente ao `index.html` + `styles.css`).
//! O layout é calculado à mão para reproduzir as medidas do CSS ao pixel.

mod composer;
mod empty_state;
mod git;
mod github;
mod map;
mod menus;
mod overlay;
mod panes;
mod projects;
mod right_panel;
mod shortcuts;
mod sidebar;
mod tabs;
mod titlebar;
mod usage;

use std::sync::Arc;

use egui::{Align2, Color32, CursorIcon, FontId, Galley, Id, Order, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::deck::{Deck, RightView};
use crate::theme;

pub use composer::COMPOSER_INPUT_ID;
pub use panes::pane_body;

/// Largura mínima que sobra para o terminal ao lado da barra direita.
const TERM_MIN_W: f32 = 360.0;

/// Largura efetiva da barra direita e o máximo que a janela permite agora. O valor salvo não é
/// alterado: se a janela crescer de novo, a barra volta à largura escolhida.
pub fn right_width(saved: f32, win_w: f32, left: f32) -> (f32, f32) {
    let max = theme::RIGHT_MAX_W.min(win_w * 0.5).min(win_w - left - TERM_MIN_W).max(260.0);
    (saved.max(theme::RIGHT_MIN_W).min(max), max)
}

/// Desenha a janela inteira.
pub fn draw(deck: &mut Deck, ui: &mut Ui) {
    let window = ui.max_rect();
    ui.painter().rect_filled(window, 0.0, theme::BG);
    let ctx = ui.ctx().clone();

    // ---------- Layout: barra de título em cima; embaixo, barra esquerda | terminal | barra direita ----------
    let titlebar = Rect::from_min_size(window.min, Vec2::new(window.width(), theme::TITLEBAR_H));
    let full = Rect::from_min_max(Pos2::new(window.min.x, titlebar.max.y), window.max);
    let collapsed = deck.sidebar_collapsed;
    let sw = deck.sidebar_w.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W);
    let left = if collapsed { 0.0 } else { sw + 1.0 };
    let (right_w, right_max) = right_width(deck.right_w, full.width(), left);
    let center = Rect::from_min_max(Pos2::new(full.min.x + left, full.min.y), Pos2::new(full.max.x - right_w - 1.0, full.max.y));
    let term_rect = center;
    // A barra tem sempre a mesma largura: o Mapa e o diff do Git rolam dentro dela.
    let panel = Rect::from_min_max(Pos2::new(full.max.x - right_w, full.min.y), full.max);
    // Nas abas Git e GitHub o compositor some para dar espaço às listas, diffs e logs.
    let comp_h = if matches!(deck.right_view, RightView::Git | RightView::GitHub) {
        0.0
    } else {
        composer::measure(deck, ui, right_w)
    };

    // ---------- Terminais (tela dividida por tipo de agente) ----------
    panes::draw(deck, ui, term_rect);

    // ---------- Barra lateral ----------
    if !collapsed {
        let side = Rect::from_min_size(full.min, Vec2::new(sw, full.height()));
        sidebar::draw(deck, ui, side);
    }

    // ---------- Barra de título ----------
    let menus = titlebar::draw(deck, ui, titlebar);

    // ---------- Divisores (por último no fundo: ganham o clique na faixa de ±3px) ----------
    if !collapsed {
        sidebar::splitter(deck, ui, Rect::from_min_size(Pos2::new(full.min.x + sw, full.min.y), Vec2::new(1.0, full.height())), full);
    }
    let line = Rect::from_min_size(Pos2::new(full.max.x - right_w - 1.0, full.min.y), Vec2::new(1.0, full.height()));
    right_panel::splitter(deck, ui, line, full, right_w, right_max);

    // ---------- Barra direita (numa camada acima dos terminais) ----------
    egui::Area::new(Id::new("right-panel")).order(Order::Middle).fixed_pos(panel.min).show(&ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(panel.size(), Sense::hover());
        right_panel::draw_panel(deck, ui, rect, comp_h);
    });

    // ---------- Menus suspensos e overlay ----------
    menus::new_tab_menu(deck, &ctx, projects::new_agent_anchor(&ctx, menus.fallback_anchor));
    menus::settings_menu(deck, &ctx, menus.settings_anchor);
    menus::tab_context_menu(deck, &ctx);
    menus::rename_tab_modal(deck, &ctx);
    shortcuts::shortcuts_modal(deck, &ctx);
    overlay::drop_overlay(deck, &ctx, window);

    // O compositor tem foco de teclado → o terminal não.
    let egui_focus = ctx.memory(|m| m.focused());
    deck.composer_focused = egui_focus == Some(Id::new(COMPOSER_INPUT_ID));
    if egui_focus.is_some() {
        deck.term_focus = false;
        deck.ime_preedit.clear();
    }
}

// ---------------------------------------------------------------------------
// Utilitários de pintura
// ---------------------------------------------------------------------------

/// Texto numa linha (sem quebra).
pub fn galley(painter: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    painter.layout_no_wrap(text.to_owned(), font, color)
}

/// Texto numa linha com reticências (`text-overflow: ellipsis`).
pub fn galley_trunc(painter: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// Largura do texto numa linha.
pub fn text_w(painter: &Painter, text: &str, font: FontId) -> f32 {
    galley(painter, text, font, Color32::WHITE).size().x
}

/// Pinta a galley alinhada dentro de `rect` (centralizada verticalmente) e devolve o retângulo ocupado.
pub fn place(painter: &Painter, rect: Rect, align: Align2, g: Arc<Galley>, color: Color32) -> Rect {
    let r = align.align_size_within_rect(g.size(), rect);
    painter.galley(r.min, g, color);
    r
}

/// Texto alinhado dentro de um retângulo.
pub fn text_in(painter: &Painter, rect: Rect, align: Align2, text: &str, font: FontId, color: Color32) -> Rect {
    let g = galley(painter, text, font, color);
    place(painter, rect, align, g, color)
}

/// `.dot` (8×8, círculo).
pub fn dot(painter: &Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 4.0, color);
}

/// `box-shadow: 0 <y>px <blur>px <color>`.
pub fn shadow(painter: &Painter, rect: Rect, radius: u8, offset_y: i8, blur: u8, color: Color32) {
    let s = egui::Shadow { offset: [0, offset_y], blur, spread: 0, color };
    painter.add(s.as_shape(rect, radius));
}

/// Área clicável com cursor de mão (como `cursor: pointer`).
pub fn clickable(ui: &Ui, rect: Rect, id: Id) -> Response {
    let r = ui.interact(rect, id, Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    r
}

/// Tooltip no estilo do atributo `title`.
pub fn tip(resp: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(text).font(theme::ui(12.0)).color(theme::TEXT));
    })
}

/// Divisor arrastável: linha de 1px que brilha na cor do agente ativo ao passar/arrastar.
pub fn paint_splitter(painter: &Painter, line: Rect, hot: bool, color: Color32) {
    if hot {
        // box-shadow: 0 0 3px <cor>
        for (grow, alpha) in [(3.0, 0.12), (2.0, 0.22), (1.0, 0.4)] {
            painter.rect_filled(line.expand(grow), 0.0, theme::with_opacity(color, alpha));
        }
        painter.rect_filled(line, 0.0, color);
    } else {
        painter.rect_filled(line, 0.0, theme::BORDER);
    }
}

/// Moldura arredondada com borda de 1px.
pub fn frame(painter: &Painter, rect: Rect, radius: f32, fill: Color32, border: Color32) {
    painter.rect(rect, radius, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_right_panel_keeps_room_for_the_terminal() {
        // Janela larga: a largura salva vale (respeitando o mínimo e o máximo).
        assert_eq!(right_width(380.0, 1280.0, 221.0), (380.0, 640.0));
        assert_eq!(right_width(100.0, 1280.0, 221.0).0, theme::RIGHT_MIN_W);
        assert_eq!(right_width(900.0, 1280.0, 221.0).0, 640.0);
        // Janela estreita: o terminal fica com pelo menos TERM_MIN_W, e a barra com pelo menos 260.
        let (w, max) = right_width(380.0, 820.0, 221.0);
        assert_eq!((w, max), (260.0, 260.0));
        assert!(820.0 - 221.0 - w >= 300.0);
        let (w, _) = right_width(380.0, 1000.0, 0.0);
        assert_eq!(w, 380.0);
    }
}

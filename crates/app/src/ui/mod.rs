//! Pintura da interface (equivalente ao `index.html` + `styles.css`).
//! O layout é calculado à mão para reproduzir as medidas do CSS ao pixel.

mod composer;
mod empty_state;
mod feed;
mod menus;
mod overlay;
mod sidebar;
mod topbar;

use std::sync::Arc;

use egui::{Align2, Color32, CursorIcon, FontId, Galley, Id, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::deck::Deck;
use crate::terminal::view;
use crate::theme;

pub use composer::COMPOSER_INPUT_ID;

/// Altura de `#feed-head` (padding 4px + botão com linha de 13px × 1,45 + padding 2px).
pub const FEED_HEAD_H: f32 = 31.0;

/// Desenha a janela inteira.
pub fn draw(deck: &mut Deck, ui: &mut Ui) {
    let full = ui.max_rect();
    ui.painter().rect_filled(full, 0.0, theme::BG);
    let ctx = ui.ctx().clone();

    // ---------- Layout (flex do #app-wrap / #main-content) ----------
    let collapsed = deck.sidebar_collapsed;
    let sw = deck.sidebar_w.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W);
    let main_x = if collapsed { full.min.x } else { full.min.x + sw + 1.0 };
    let main = Rect::from_min_max(Pos2::new(main_x, full.min.y), full.max);
    let topbar = Rect::from_min_size(main.min, Vec2::new(main.width(), theme::TOPBAR_H));

    let comp_h = composer::measure(deck, ui, main.width());
    let min_h = (FEED_HEAD_H + comp_h + 60.0).max(200.0);
    if !deck.feed_collapsed && deck.bottom_h < min_h {
        deck.bottom_h = min_h; // ensureBottomMinHeight()
    }
    let bottom_h = if deck.feed_collapsed { FEED_HEAD_H + comp_h } else { deck.bottom_h };
    let term_h = (main.height() - theme::TOPBAR_H - 1.0 - bottom_h).max(120.0);
    let term_rect = Rect::from_min_size(Pos2::new(main.min.x, topbar.max.y), Vec2::new(main.width(), term_h));
    let splitter_y = term_rect.max.y;
    let bottom = Rect::from_min_size(Pos2::new(main.min.x, splitter_y + 1.0), Vec2::new(main.width(), bottom_h));

    // ---------- Terminal ativo ----------
    draw_terminals(deck, ui, term_rect);

    // ---------- Barra lateral ----------
    if !collapsed {
        let side = Rect::from_min_size(full.min, Vec2::new(sw, full.height()));
        sidebar::draw(deck, ui, side);
    }

    // ---------- Topbar, área inferior ----------
    let menus = topbar::draw(deck, ui, topbar);
    let composer_anchor = feed::draw_bottom(deck, ui, bottom, comp_h);

    // ---------- Divisores (por último: ganham o clique na faixa de ±3px) ----------
    if !collapsed {
        sidebar::splitter(deck, ui, Rect::from_min_size(Pos2::new(full.min.x + sw, full.min.y), Vec2::new(1.0, full.height())), full);
    }
    feed::row_splitter(deck, ui, Rect::from_min_size(Pos2::new(main.min.x, splitter_y), Vec2::new(main.width(), 1.0)), full, comp_h);

    // ---------- Menus suspensos e overlay ----------
    menus::new_tab_menu(deck, &ctx, menus.new_tab_anchor);
    menus::settings_menu(deck, &ctx, menus.settings_anchor);
    menus::target_menu(deck, &ctx, composer_anchor);
    overlay::drop_overlay(deck, &ctx, full);

    // O compositor tem foco de teclado → o terminal não.
    let egui_focus = ctx.memory(|m| m.focused());
    deck.composer_focused = egui_focus == Some(Id::new(COMPOSER_INPUT_ID));
    if egui_focus.is_some() {
        deck.term_focus = false;
        deck.ime_preedit.clear();
    }
}

fn draw_terminals(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    deck.fit_active(rect);
    if deck.tabs.is_empty() {
        if deck.initialized {
            empty_state::draw(deck, ui, rect);
        }
        return;
    }
    let Some(idx) = deck.active else { return };
    let Some(metrics) = deck.metrics.clone() else { return };
    let ctx = ui.ctx().clone();
    let window_focused = deck.window_focused;
    let focused = deck.term_focus && window_focused;
    let elapsed = deck.blink_epoch.elapsed().as_millis();
    let blink_on = (elapsed / 600) % 2 == 0;
    let preedit = deck.ime_preedit.clone();
    let tab = &mut deck.tabs[idx];
    let id = Id::new(("terminal", tab.uid));
    let resp = view::show(ui, rect, id, &tab.session, &mut tab.view, focused, blink_on, &preedit, &metrics);
    let bytes = resp.to_pty;
    // Habilita o IME do sistema ancorado no cursor do terminal (candidatos e seletor de emoji).
    if let (true, Some(cell)) = (focused, resp.cursor_rect) {
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Terminal,
                rect: cell,
                cursor_rect: cell,
                should_interrupt_composition: false,
            });
        });
    }
    if resp.focus {
        ctx.memory_mut(|m| {
            if let Some(f) = m.focused() {
                m.surrender_focus(f);
            }
        });
        deck.set_term_focus(true);
        deck.new_tab_menu_open = false;
    }
    if !bytes.is_empty() {
        deck.terminal_mouse_bytes(idx, &bytes);
    }
    if focused {
        let next = 600 - (elapsed % 600) as u64;
        ctx.request_repaint_after(std::time::Duration::from_millis(next));
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

//! Faixa de abas do cabeçalho de cada janela da tela dividida (antes ficava na topbar).

use egui::{Align2, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{dot, galley_trunc, place, text_in, text_w, tip};
use crate::deck::Deck;
use crate::theme;

pub enum StripClick {
    Activate(usize),
    Close(usize),
    Rename(usize),
    ContextMenu(usize, Pos2),
}

const TAB_FONT: f32 = 13.0;
const LABEL_MAX: f32 = 140.0;

/// Pinta as abas `tabs` (índices de `deck.tabs`) em `rect`, com rolagem horizontal quando não
/// cabem. `shown` é a aba visível da janela; `focused`, se a janela tem o foco.
pub fn strip(deck: &Deck, ui: &Ui, rect: Rect, tabs: &[usize], shown: usize, focused: bool, scroll_id: Id) -> Option<StripClick> {
    let ctx = ui.ctx().clone();
    let painter = ui.painter_at(rect);
    let font = theme::ui(TAB_FONT);
    let close_w = 12.0 + text_w(&painter, "×", font.clone());
    let widths: Vec<f32> = tabs
        .iter()
        .map(|&i| {
            let lw = text_w(&painter, &deck.tabs[i].label, font.clone()).min(LABEL_MAX);
            12.0 + 8.0 + 8.0 + lw + 4.0 + close_w + 6.0 + 1.0
        })
        .collect();
    let total: f32 = widths.iter().sum();

    // Rolagem horizontal (sem barra visível); a aba visível fica sempre à vista.
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    if total > rect.width() {
        let over = ui.interact(rect, scroll_id.with("area"), Sense::hover());
        if over.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            scroll -= d.x + d.y;
        }
        if let Some(a) = tabs.iter().position(|&i| i == shown) {
            let start: f32 = widths[..a].iter().sum();
            let end = start + widths[a];
            if start < scroll {
                scroll = start;
            } else if end > scroll + rect.width() {
                scroll = end - rect.width();
            }
        }
        scroll = scroll.clamp(0.0, total - rect.width());
    } else {
        scroll = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(scroll_id, scroll));

    let mut x = rect.min.x - scroll;
    let mut click = None;
    for (n, &i) in tabs.iter().enumerate() {
        let tab = &deck.tabs[i];
        let w = widths[n];
        let r = Rect::from_min_size(Pos2::new(x, rect.min.y), Vec2::new(w - 1.0, rect.height()));
        x += w;
        if r.max.x < rect.min.x || r.min.x > rect.max.x {
            continue;
        }
        let vis = r.intersect(rect);
        let is_shown = i == shown;
        let resp = ui.interact(vis, Id::new(("tab", tab.uid)), Sense::click());
        let close_rect = Rect::from_min_size(Pos2::new(r.max.x - 6.0 - close_w, r.center().y - 10.5), Vec2::new(close_w, 21.0));
        let close_resp = ui.interact(close_rect.intersect(rect), Id::new(("tab-close", tab.uid)), Sense::click());

        let bg = if is_shown {
            Some(theme::BG)
        } else if resp.hovered() || close_resp.hovered() {
            Some(theme::PANEL_2)
        } else {
            None
        };
        if let Some(bg) = bg {
            painter.rect_filled(r, 0.0, bg);
        }
        if is_shown {
            // Linha superior na cor do agente (esmaecida se a janela não tem o foco).
            let c = if focused { tab.color } else { theme::with_opacity(tab.color, 0.45) };
            painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width(), 2.0)), 0.0, c);
        }
        painter.rect_filled(Rect::from_min_size(Pos2::new(r.max.x, r.min.y), Vec2::new(1.0, r.height())), 0.0, theme::BORDER);

        let color = if is_shown && focused { theme::TEXT } else { theme::MUTED };
        dot(&painter, Pos2::new(r.min.x + 12.0 + 4.0, r.center().y), tab.color);
        let label_x = r.min.x + 12.0 + 8.0 + 8.0;
        let g = galley_trunc(&painter, &tab.label, font.clone(), color, LABEL_MAX);
        let lr = place(
            &painter,
            Rect::from_min_max(Pos2::new(label_x, r.min.y), Pos2::new(label_x + LABEL_MAX, r.max.y)),
            Align2::LEFT_CENTER,
            g,
            color,
        );
        if !tab.alive {
            painter.hline(lr.min.x..=lr.max.x, lr.center().y + 0.5, Stroke::new(1.0, color));
        }

        let close_hot = close_resp.hovered();
        if close_hot {
            painter.rect_filled(close_rect, 4.0, theme::BORDER);
        }
        if close_hot || resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        text_in(&painter, close_rect, Align2::CENTER_CENTER, "×", font.clone(), if close_hot { theme::TEXT } else { theme::MUTED });

        if tip(close_resp, "Fechar").clicked() {
            click = Some(StripClick::Close(i));
        } else if resp.double_clicked() {
            click = Some(StripClick::Rename(i));
        } else if resp.secondary_clicked() {
            let pos = ui.input(|inp| inp.pointer.latest_pos()).unwrap_or_else(|| r.center());
            click = Some(StripClick::ContextMenu(i, pos));
        } else if tip(resp, format!("{}\nDuplo clique ou botão direito para renomear", tab.label)).clicked() {
            click = Some(StripClick::Activate(i));
        }
    }
    click
}

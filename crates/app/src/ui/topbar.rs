//! `#topbar`: botão da barra lateral, abas, "+", pasta de trabalho e configurações.

use egui::{Align2, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, dot, galley, galley_trunc, place, text_in, text_w, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub struct Anchors {
    pub new_tab_anchor: Rect,
    pub settings_anchor: Rect,
}

const TAB_FONT: f32 = 13.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) -> Anchors {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL);
    // border-bottom: 1px
    let inner = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let ctx = ui.ctx().clone();

    // ---------- #sidebar-toggle-btn (20px) ----------
    let toggle = Rect::from_min_size(inner.min, Vec2::new(20.0, inner.height()));
    let r = clickable(ui, toggle, Id::new("sidebar-toggle"));
    let hovered = r.hovered();
    painter.rect_filled(Rect::from_min_max(toggle.min, Pos2::new(toggle.max.x - 1.0, toggle.max.y)), 0.0, if hovered { theme::PANEL_2 } else { theme::PANEL });
    painter.rect_filled(Rect::from_min_max(Pos2::new(toggle.max.x - 1.0, toggle.min.y), toggle.max), 0.0, theme::BORDER);
    let t = ctx.animate_bool_with_time(Id::new("sidebar-chevron"), deck.sidebar_collapsed, 0.15);
    deck.icons.paint(&painter, Icon::SidebarChevron, Pos2::new(toggle.center().x - 0.5, toggle.center().y), 9.0, if hovered { theme::TEXT } else { theme::MUTED }, t * std::f32::consts::PI);
    let title = if deck.sidebar_collapsed { "Expandir barra lateral (Ctrl+B)" } else { "Recolher barra lateral (Ctrl+B)" };
    if tip(r, title).clicked() {
        deck.toggle_sidebar();
    }

    // ---------- Lado direito: engrenagem e pasta ----------
    let settings = Rect::from_min_max(Pos2::new(inner.max.x - 38.0, inner.min.y), inner.max);
    let ws_text = format!("📁 {}", deck.workspace.display());
    let ws_font = theme::ui(TAB_FONT);
    let ws_max = ctx.content_rect().width() * 0.35;
    let ws_w = (text_w(&painter, &ws_text, ws_font.clone()) + 28.0).min(ws_max);
    let ws_rect = Rect::from_min_max(Pos2::new(settings.min.x - ws_w, inner.min.y), Pos2::new(settings.min.x, inner.max.y));

    // ---------- Abas ----------
    let tabs_x0 = toggle.max.x;
    let plus_w = 38.0;
    let tabs_avail = (ws_rect.min.x - plus_w - tabs_x0).max(0.0);
    let font = theme::ui(TAB_FONT);
    let close_w = 12.0 + text_w(&painter, "×", font.clone());
    let widths: Vec<f32> = deck
        .tabs
        .iter()
        .map(|t| {
            let lw = text_w(&painter, &t.label, font.clone()).min(150.0);
            14.0 + 8.0 + 8.0 + 2.0 + lw + 2.0 + 6.0 + close_w + 10.0 + 1.0
        })
        .collect();
    let total: f32 = widths.iter().sum();
    let tabs_w = total.min(tabs_avail);
    let tabs_rect = Rect::from_min_size(Pos2::new(tabs_x0, inner.min.y), Vec2::new(tabs_w, inner.height()));

    // Rolagem horizontal das abas (overflow-x: auto, sem barra visível).
    let scroll_id = Id::new("tabs-scroll");
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    if total > tabs_w {
        let over = ui.interact(tabs_rect, Id::new("tabs-scroll-area"), Sense::hover());
        if over.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            scroll -= d.x + d.y;
        }
        if let Some(a) = deck.active {
            // Garante a aba ativa visível.
            let start: f32 = widths[..a].iter().sum();
            let end = start + widths[a];
            if start < scroll {
                scroll = start;
            } else if end > scroll + tabs_w {
                scroll = end - tabs_w;
            }
        }
        scroll = scroll.clamp(0.0, total - tabs_w);
    } else {
        scroll = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(scroll_id, scroll));

    let tab_painter = painter.with_clip_rect(tabs_rect);
    let mut x = tabs_x0 - scroll;
    let mut activate = None;
    let mut close = None;
    for (i, tab) in deck.tabs.iter().enumerate() {
        let w = widths[i];
        let r = Rect::from_min_size(Pos2::new(x, inner.min.y), Vec2::new(w - 1.0, inner.height()));
        x += w;
        if r.max.x < tabs_rect.min.x || r.min.x > tabs_rect.max.x {
            continue;
        }
        let vis = r.intersect(tabs_rect);
        let is_active = deck.active == Some(i);
        let resp = ui.interact(vis, Id::new(("tab", tab.uid)), Sense::click());
        // Botão fechar (×): padding 2px 6px, margin-left 6px.
        let close_rect = Rect::from_min_size(Pos2::new(r.max.x - 10.0 - close_w, r.center().y - 11.5), Vec2::new(close_w, 23.0));
        let close_resp = ui.interact(close_rect.intersect(tabs_rect), Id::new(("tab-close", tab.uid)), Sense::click());

        let bg = if is_active {
            Some(theme::BG)
        } else if resp.hovered() || close_resp.hovered() {
            Some(theme::PANEL_2)
        } else {
            None
        };
        if let Some(bg) = bg {
            tab_painter.rect_filled(r, 0.0, bg);
        }
        if is_active {
            // box-shadow: inset 0 2px 0 var(--tab-color)
            tab_painter.rect_filled(Rect::from_min_size(r.min, Vec2::new(r.width(), 2.0)), 0.0, tab.color);
        }
        // border-right
        tab_painter.rect_filled(Rect::from_min_size(Pos2::new(r.max.x, r.min.y), Vec2::new(1.0, r.height())), 0.0, theme::BORDER);

        let color = if is_active { theme::TEXT } else { theme::MUTED };
        dot(&tab_painter, Pos2::new(r.min.x + 14.0 + 4.0, r.center().y), tab.color);
        let label_x = r.min.x + 14.0 + 8.0 + 8.0 + 2.0;
        let g = galley_trunc(&tab_painter, &tab.label, font.clone(), color, 150.0);
        let lr = place(&tab_painter, Rect::from_min_max(Pos2::new(label_x, r.min.y), Pos2::new(label_x + 150.0, r.max.y)), Align2::LEFT_CENTER, g, color);
        if !tab.alive && tab.viewer.is_none() {
            // .tab.dead .label { text-decoration: line-through }
            let y = lr.center().y + 0.5;
            tab_painter.hline(lr.min.x..=lr.max.x, y, Stroke::new(1.0, color));
        }

        let close_hot = close_resp.hovered();
        if close_hot {
            tab_painter.rect_filled(close_rect, 4.0, theme::BORDER);
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        } else if resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        text_in(&tab_painter, close_rect, Align2::CENTER_CENTER, "×", font.clone(), if close_hot { theme::TEXT } else { theme::MUTED });

        if tip(close_resp, "Fechar").clicked() {
            close = Some(i);
        } else if tip(resp, tab.label.clone()).clicked() {
            activate = Some(i);
        }
    }
    if let Some(i) = close {
        deck.close_tab(i);
    } else if let Some(i) = activate {
        deck.activate(i);
    }

    // ---------- #new-tab (+) ----------
    let plus = Rect::from_min_size(Pos2::new(tabs_rect.max.x, inner.min.y), Vec2::new(plus_w, inner.height()));
    let r = clickable(ui, plus, Id::new("new-tab"));
    let hot = r.hovered() || deck.new_tab_menu_open;
    if r.hovered() {
        painter.rect_filled(plus, 0.0, theme::PANEL_2);
    }
    let pg = galley(&painter, "+", theme::ui(18.0), theme::MUTED);
    place(&painter, plus, Align2::CENTER_CENTER, pg, if hot { theme::TEXT } else { theme::MUTED });
    if tip(r, "Nova aba (Ctrl+T)").clicked() {
        deck.new_tab_menu_open = !deck.new_tab_menu_open;
        deck.settings_open = false;
        if deck.new_tab_menu_open {
            deck.refresh_agents();
        }
    }

    // ---------- #workspace ----------
    let r = clickable(ui, ws_rect, Id::new("workspace"));
    let wc = if r.hovered() { theme::TEXT } else { theme::MUTED };
    let g = galley_trunc(&painter, &ws_text, ws_font, wc, ws_rect.width() - 28.0);
    place(&painter, ws_rect.shrink2(Vec2::new(14.0, 0.0)), Align2::LEFT_CENTER, g, wc);
    let ws_title = format!("{}\nClique para trocar (vale para as próximas abas)", deck.workspace.display());
    if tip(r, ws_title).clicked() {
        deck.pick_workspace();
    }

    // ---------- #settings-btn ----------
    let r = clickable(ui, settings, Id::new("settings-btn"));
    let hot = r.hovered();
    if hot {
        painter.rect_filled(settings, 0.0, theme::PANEL_2);
    }
    deck.icons.paint(&painter, Icon::Gear, settings.center(), 18.0, if hot { theme::TEXT } else { theme::MUTED }, 0.0);
    if tip(r, "Configurações").clicked() {
        deck.settings_open = !deck.settings_open;
        deck.new_tab_menu_open = false;
    }

    Anchors { new_tab_anchor: plus, settings_anchor: settings }
}

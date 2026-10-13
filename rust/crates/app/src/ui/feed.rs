//! `#bottom`: cabeçalho da "Conversa integrada", mensagens e divisor horizontal.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};

use super::{composer, galley, paint_splitter, place, text_in, tip, FEED_HEAD_H};
use crate::deck::{Deck, FeedItem};
use commanddeck_core::FeedKind;
use crate::theme;

/// Desenha `#bottom` (cabeçalho, conversa e compositor). Devolve a âncora do seletor "Enviar para".
pub fn draw_bottom(deck: &mut Deck, ui: &mut Ui, rect: Rect, comp_h: f32) -> Rect {
    ui.painter().rect_filled(rect, 0.0, theme::BG);
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), FEED_HEAD_H));
    draw_head(deck, ui, head);
    let comp = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - comp_h), rect.max);
    if !deck.feed_collapsed {
        let feed = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), Pos2::new(rect.max.x, comp.min.y));
        if feed.height() > 1.0 {
            draw_feed(deck, ui, feed);
        }
    }
    composer::draw(deck, ui, comp)
}

fn draw_head(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    let font = theme::ui(13.0);
    // padding: 4px 10px; botões com padding 2px 4px.
    let label = format!("{} Conversa integrada", if deck.feed_collapsed { "▸" } else { "▾" });
    let lw = galley(&painter, &label, font.clone(), theme::MUTED).size().x + 8.0;
    let btn = Rect::from_min_size(Pos2::new(rect.min.x + 10.0, rect.min.y + 4.0), Vec2::new(lw, rect.height() - 8.0));
    let r = super::clickable(ui, btn, Id::new("feed-toggle"));
    text_in(&painter, btn, Align2::CENTER_CENTER, &label, font.clone(), if r.hovered() { theme::TEXT } else { theme::MUTED });
    if r.clicked() {
        deck.toggle_feed();
    }
    // Badge de não vistas.
    if deck.unseen > 0 {
        let txt = deck.unseen.to_string();
        let g = galley(&painter, &txt, theme::ui(11.0), theme::WHITE);
        let size = Vec2::new(g.size().x + 14.0, 16.0);
        let b = Rect::from_min_size(Pos2::new(btn.max.x + 8.0, rect.center().y - size.y / 2.0), size);
        painter.rect_filled(b, 9.0, theme::ACCENT);
        place(&painter, b, Align2::CENTER_CENTER, g, theme::WHITE);
    }
    // "Limpar" à direita.
    let cw = galley(&painter, "Limpar", font.clone(), theme::MUTED).size().x + 8.0;
    let clear = Rect::from_min_size(Pos2::new(rect.max.x - 10.0 - cw, rect.min.y + 4.0), Vec2::new(cw, rect.height() - 8.0));
    let r = super::clickable(ui, clear, Id::new("feed-clear"));
    text_in(&painter, clear, Align2::CENTER_CENTER, "Limpar", font, if r.hovered() { theme::TEXT } else { theme::MUTED });
    if r.clicked() {
        deck.clear_feed();
    }
}

fn draw_feed(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    // border-bottom: 1px solid var(--border)
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let area = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    let ctx = ui.ctx().clone();

    // Se a conversa estava no fim e a área mudou de altura (compositor cresceu), mantém no fim.
    let h_id = Id::new("feed-last-h");
    let last_h = ctx.data(|d| d.get_temp::<f32>(h_id)).unwrap_or(area.height());
    if (last_h - area.height()).abs() > 0.5 && deck.feed_near_bottom {
        deck.feed_scroll_to_bottom = true;
    }
    ctx.data_mut(|d| d.insert_temp(h_id, area.height()));

    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(area);
    let scroll_to_bottom = std::mem::take(&mut deck.feed_scroll_to_bottom);
    let thinking_alpha = blink_alpha(ctx.input(|i| i.time), 1.0);
    let has_thinking = deck.feed.iter().any(|f| f.kind == FeedKind::Thinking);
    let out = egui::ScrollArea::vertical().id_salt("feed").auto_shrink([false, false]).animated(false).show(&mut child, |ui| {
        let full_w = ui.available_width();
        // padding: 4px 14px 8px
        let content_w = (full_w - 28.0).max(40.0);
        ui.add_space(4.0);
        if deck.feed.is_empty() {
            empty_text(ui, content_w);
        }
        let mut last = None;
        let n = deck.feed.len();
        for (i, item) in deck.feed.iter().enumerate() {
            // Margens verticais de 8px colapsam entre irmãos.
            ui.add_space(8.0);
            let r = message(ui, item, content_w, thinking_alpha);
            last = Some(r);
            if i + 1 == n {
                ui.add_space(8.0);
            }
        }
        ui.add_space(8.0);
        if scroll_to_bottom {
            if let Some(r) = last {
                ui.scroll_to_rect(r.expand2(Vec2::new(0.0, 16.0)), Some(Align::BOTTOM));
            }
        }
    });
    let dist = out.content_size.y - out.state.offset.y - out.inner_rect.height();
    deck.feed_near_bottom = dist < 40.0;
    if has_thinking {
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// Opacidade da animação `blink` (1 → 0,2 → 1, `ease`) num período `period` s.
pub fn blink_alpha(time: f64, period: f64) -> f32 {
    let p = (time % period) / period;
    let half = if p < 0.5 { p * 2.0 } else { (1.0 - p) * 2.0 };
    // aproximação da curva `ease`
    let e = half * half * (3.0 - 2.0 * half);
    (1.0 - 0.8 * e) as f32
}

fn empty_text(ui: &mut Ui, w: f32) {
    let mut job = LayoutJob::default();
    job.wrap.max_width = w;
    let base = TextFormat { font_id: theme::ui(13.0), color: theme::MUTED, line_height: Some(13.0 * theme::LINE_HEIGHT), ..Default::default() };
    job.append("Aqui aparecem as conversas entre os agentes (via ", 0.0, base.clone());
    job.append(" ask-agent ", 0.0, TextFormat { font_id: theme::mono(12.0), background: theme::PANEL_2, ..base.clone() });
    job.append(") e as mesas de orquestração.", 0.0, base);
    let g = ui.painter().layout_job(job);
    let (r, _) = ui.allocate_exact_size(Vec2::new(w + 28.0, g.size().y + 16.0), Sense::hover());
    ui.painter().galley(Pos2::new(r.min.x + 14.0, r.min.y + 8.0), g, theme::MUTED);
}

/// `.msg` (article). Devolve o retângulo ocupado.
fn message(ui: &mut Ui, item: &FeedItem, w: f32, thinking_alpha: f32) -> Rect {
    let plain = matches!(item.kind, FeedKind::System | FeedKind::Thinking);
    let (pad_x, pad_y) = (10.0, if plain { 2.0 } else { 8.0 });
    let inner_w = (w - 3.0 - pad_x * 2.0).max(20.0);
    let painter = ui.painter().clone();

    // Cabeçalho: "quem → para" + hora à direita (12px, muted).
    let hfont = theme::ui(12.0);
    let time_g = galley(&painter, &item.time, hfont.clone(), theme::MUTED);
    let mut head = LayoutJob::default();
    head.wrap.max_width = (inner_w - time_g.size().x - 6.0).max(10.0);
    head.wrap.max_rows = 1;
    head.wrap.overflow_character = Some('…');
    let hfmt = TextFormat { font_id: hfont.clone(), color: theme::MUTED, line_height: Some(12.0 * theme::LINE_HEIGHT), ..Default::default() };
    if let Some(who) = &item.who {
        head.append(who, 0.0, TextFormat { font_id: theme::ui_semibold(12.0), color: item.color, ..hfmt.clone() });
    }
    if let Some(to) = &item.to {
        head.append(&format!(" → {to}"), if item.who.is_some() { 6.0 } else { 0.0 }, hfmt.clone());
    }
    let head_g = painter.layout_job(head);
    let head_h = 12.0 * theme::LINE_HEIGHT;

    // Corpo (pre-wrap, 13px, line-height 1.5).
    let body_color = match item.kind {
        FeedKind::Error => theme::RED_TEXT,
        FeedKind::System | FeedKind::Thinking => theme::MUTED,
        _ => theme::TEXT,
    };
    let mut body = LayoutJob::default();
    body.wrap.max_width = inner_w;
    let bfmt = TextFormat { font_id: theme::ui(13.0), color: body_color, line_height: Some(13.0 * 1.5), ..Default::default() };
    body.append(&item.text, 0.0, bfmt.clone());
    if item.kind == FeedKind::Thinking {
        body.append(" ●", 0.0, TextFormat { color: theme::with_opacity(body_color, thinking_alpha), ..bfmt });
    }
    let body_g = painter.layout_job(body);

    let h = pad_y * 2.0 + head_h + 4.0 + body_g.size().y.max(13.0 * 1.5);
    let (outer, _) = ui.allocate_exact_size(Vec2::new(w + 28.0, h), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + 14.0, outer.min.y), Vec2::new(w, h));

    if !plain {
        let border = if item.kind == FeedKind::Error { theme::RED } else { item.color };
        // Borda esquerda de 3px acompanhando o raio de 8px.
        painter.rect_filled(r, 8.0, border);
        let inner = Rect::from_min_max(Pos2::new(r.min.x + 3.0, r.min.y), r.max);
        painter.rect_filled(inner, egui::CornerRadius { nw: 5, sw: 5, ne: 8, se: 8 }, theme::PANEL_2);
    }
    let x = r.min.x + 3.0 + pad_x;
    let y = r.min.y + pad_y;
    let head_rect = Rect::from_min_size(Pos2::new(x, y), Vec2::new(inner_w, head_h));
    place(&painter, head_rect, Align2::LEFT_CENTER, head_g, theme::MUTED);
    place(&painter, head_rect, Align2::RIGHT_CENTER, time_g, theme::MUTED);
    painter.galley(Pos2::new(x, y + head_h + 4.0), body_g, body_color);
    let _: Color32 = body_color;
    r
}

/// `#splitter` entre terminal e conversa (arrastar redimensiona; expande se recolhida).
pub fn row_splitter(deck: &mut Deck, ui: &mut Ui, line: Rect, full: Rect, comp_h: f32) {
    let hit = line.expand2(Vec2::new(0.0, 3.0));
    let r = ui.interact(hit, Id::new("row-splitter"), Sense::drag());
    if r.hovered() || r.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    let min_h = (super::FEED_HEAD_H + comp_h + 60.0).max(200.0);
    if r.drag_started() {
        if let Some(p) = r.interact_pointer_pos() {
            let start_h = if deck.feed_collapsed { min_h } else { deck.bottom_h };
            deck.row_dragging = Some((p.y, start_h, deck.feed_collapsed));
        }
    }
    if r.dragged() {
        if let (Some((y0, h0, _)), Some(p)) = (deck.row_dragging, ui.input(|i| i.pointer.latest_pos())) {
            let mut apply = true;
            if deck.feed_collapsed {
                if y0 - p.y > 10.0 {
                    deck.toggle_feed();
                } else {
                    apply = false;
                }
            }
            if apply {
                let win_h = full.height();
                let max_h = min_h.max((win_h * 0.75).min(win_h - 140.0));
                deck.bottom_h = (h0 + y0 - p.y).clamp(min_h, max_h);
            }
        }
    }
    if r.drag_stopped() {
        deck.row_dragging = None;
        deck.persist_layout();
    }
    let hot = r.hovered() || deck.row_dragging.is_some();
    let painter = ui.painter_at(full);
    paint_splitter(&painter, line, hot, deck.active_color());
    tip(r, "Arraste para redimensionar");
}

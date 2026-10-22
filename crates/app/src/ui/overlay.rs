//! Destaque de arrastar e soltar arquivos (`#drop-overlay`): destaca o painel de agente (ou o
//! formulário "Nova tarefa") que vai receber os arquivos soltos.

use egui::{Align2, Id, LayerId, Order, Pos2, Rect, Stroke, Vec2};

use super::{galley_trunc, place};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub fn drop_overlay(deck: &mut Deck, ctx: &egui::Context, full: Rect) {
    let hovering = deck.drop_hover || ctx.input(|i| !i.raw.hovered_files.is_empty());
    if !hovering {
        return;
    }
    let pointer = ctx.input(|i| i.pointer.latest_pos());
    let Some(zone) = deck.resolve_drop_zone(pointer, full) else {
        return;
    };
    deck.last_drop_target = Some(zone.target);

    let r = zone.rect;
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("drop-overlay")));
    p.rect_filled(r, 0.0, theme::with_alpha(theme::pal().bg, 0.78));
    p.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0, theme::accent()), egui::StrokeKind::Inside);

    let max_card_w = (r.width() - 24.0).max(160.0);
    let max_text_w = (max_card_w - 48.0).max(100.0);
    let title = galley_trunc(&p, &zone.title, theme::ui_semibold(15.0), theme::pal().text_strong, max_text_w);
    let sub = galley_trunc(&p, &zone.subtitle, theme::ui(12.5), theme::pal().text_2, max_text_w);
    let icon_sz = 40.0;
    let content_w = title.size().x.max(sub.size().x).max(icon_sz);
    let content_h = icon_sz + 10.0 + title.size().y + 8.0 + sub.size().y;
    let size = Vec2::new((content_w + 56.0).min(max_card_w), (content_h + 48.0).min((r.height() - 16.0).max(120.0)));
    let b = Rect::from_center_size(r.center(), size);
    p.rect_filled(b, 14.0, theme::accent_wash(theme::accent()));
    dashed_round_rect(&p, b.shrink(1.0), 13.0, Stroke::new(2.0, theme::accent()));

    let mut y = b.center().y - content_h / 2.0;
    deck.icons.paint(&p, Icon::CloudUpload, Pos2::new(b.center().x, y + icon_sz / 2.0), icon_sz, theme::accent(), 0.0);
    y += icon_sz + 10.0;
    let tr = Rect::from_min_size(Pos2::new(b.min.x + 12.0, y), Vec2::new(b.width() - 24.0, title.size().y));
    place(&p, tr, Align2::CENTER_CENTER, title, theme::pal().text_strong);
    y += tr.height() + 8.0;
    let sr = Rect::from_min_size(Pos2::new(b.min.x + 12.0, y), Vec2::new(b.width() - 24.0, sub.size().y));
    place(&p, sr, Align2::CENTER_CENTER, sub, theme::pal().text_2);
}

/// Borda tracejada (`border: 2px dashed`) acompanhando os cantos arredondados.
fn dashed_round_rect(p: &egui::Painter, r: Rect, radius: f32, stroke: Stroke) {
    // Contorno amostrado e dividido em traços de 6px com 6px de espaço (padrão do Chromium ≈ 3×largura).
    let mut pts = Vec::new();
    let corners = [
        (Pos2::new(r.max.x - radius, r.min.y + radius), -90.0f32),
        (Pos2::new(r.max.x - radius, r.max.y - radius), 0.0),
        (Pos2::new(r.min.x + radius, r.max.y - radius), 90.0),
        (Pos2::new(r.min.x + radius, r.min.y + radius), 180.0),
    ];
    pts.push(Pos2::new(r.min.x + radius, r.min.y));
    for (c, start) in corners {
        for i in 0..=12 {
            let a = (start + 90.0 * i as f32 / 12.0).to_radians();
            pts.push(Pos2::new(c.x + radius * a.cos(), c.y + radius * a.sin()));
        }
    }
    pts.push(Pos2::new(r.min.x + radius, r.min.y));
    let shapes = egui::Shape::dashed_line(&pts, stroke, 6.0, 6.0);
    p.extend(shapes);
}

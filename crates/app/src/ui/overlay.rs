//! `#drop-overlay`: aparece enquanto arquivos são arrastados sobre a janela.

use egui::{Align2, Id, LayerId, Order, Pos2, Rect, Stroke, Vec2};

use super::{galley, place};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub fn drop_overlay(deck: &mut Deck, ctx: &egui::Context, full: Rect) {
    if !deck.drop_hover {
        return;
    }
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("drop-overlay")));
    p.rect_filled(full, 0.0, theme::rgba(11, 11, 13, 0.85));

    let title = galley(&p, "Solte arquivos ou imagens aqui", theme::ui_semibold(17.0), theme::WHITE);
    let sub = galley(&p, "Eles serão anexados automaticamente à sua mensagem", theme::ui(13.0), theme::MUTED);
    // padding 36px 54px, gap 10px, ícone 48px
    let content_w = title.size().x.max(sub.size().x).max(48.0);
    let content_h = 48.0 + 10.0 + title.size().y + 10.0 + sub.size().y;
    let size = Vec2::new(content_w + 108.0 + 4.0, content_h + 72.0 + 4.0);
    let b = Rect::from_center_size(full.center(), size);
    p.rect_filled(b, 16.0, theme::rgba(79, 140, 247, 0.08));
    dashed_round_rect(&p, b.shrink(1.0), 15.0, Stroke::new(2.0, theme::DROP_BLUE));

    let mut y = b.min.y + 2.0 + 36.0;
    deck.icons.paint(&p, Icon::CloudUpload, Pos2::new(b.center().x, y + 24.0), 48.0, theme::DROP_BLUE, 0.0);
    y += 48.0 + 10.0;
    let tr = Rect::from_min_size(Pos2::new(b.min.x, y), Vec2::new(b.width(), title.size().y));
    place(&p, tr, Align2::CENTER_CENTER, title, theme::WHITE);
    y += tr.height() + 10.0;
    let sr = Rect::from_min_size(Pos2::new(b.min.x, y), Vec2::new(b.width(), sub.size().y));
    place(&p, sr, Align2::CENTER_CENTER, sub, theme::MUTED);
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

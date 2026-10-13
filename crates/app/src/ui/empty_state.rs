//! `#terminals-empty`: cartão "Nenhum agente aberto" com botões para abrir agentes.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Id, Pos2, Rect, Ui, Vec2};

use super::{clickable, dot, frame, galley, place};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);

    let title = galley(&painter, "Nenhum agente aberto", theme::ui_semibold(16.0), theme::TEXT);
    // <p> com o "+" em destaque, quebrando em até 420px.
    let mut job = LayoutJob::default();
    job.wrap.max_width = 420.0;
    job.halign = egui::Align::Center;
    let fmt = TextFormat { font_id: theme::ui(13.0), color: theme::MUTED, line_height: Some(13.0 * 1.5), ..Default::default() };
    job.append("Selecione um agente abaixo ou clique no ", 0.0, fmt.clone());
    job.append("+", 0.0, TextFormat { font_id: theme::ui_bold(13.0), color: theme::TEXT, ..fmt.clone() });
    job.append(" acima para iniciar uma sessão:", 0.0, fmt);
    let para = painter.layout_job(job);

    // Botões (.empty-agent-btn): padding 7px 14px, 12px/500, gap 10px, quebra de linha.
    let items = deck.agents.empty_actions.clone();
    let font = theme::ui(12.0);
    let btn_h = 12.0 * theme::LINE_HEIGHT + 14.0 + 2.0;
    let widths: Vec<f32> = items.iter().map(|m| 14.0 + 8.0 + 8.0 + 6.0 + galley(&painter, &m.label, font.clone(), theme::TEXT).size().x + 14.0 + 2.0).collect();
    let max_row = 420.0f32.min(rect.width() - 48.0);
    let mut rows: Vec<Vec<usize>> = vec![Vec::new()];
    let mut row_w = 0.0;
    for (i, w) in widths.iter().enumerate() {
        let need = if rows.last().is_some_and(|r| r.is_empty()) { *w } else { row_w + 10.0 + w };
        if need > max_row && !rows.last().is_some_and(|r| r.is_empty()) {
            rows.push(vec![i]);
            row_w = *w;
        } else {
            if let Some(r) = rows.last_mut() {
                r.push(i);
            }
            row_w = need;
        }
    }
    let actions_h = rows.len() as f32 * btn_h + (rows.len().saturating_sub(1)) as f32 * 10.0;

    let total_h = 58.0 + 12.0 + title.size().y + 12.0 + para.size().y + 12.0 + 8.0 + actions_h;
    let mut y = rect.center().y - total_h / 2.0;
    let cx = rect.center().x;

    // Ícone (58×58, painel, borda, raio 12)
    let icon_box = Rect::from_min_size(Pos2::new(cx - 29.0, y), Vec2::splat(58.0));
    frame(&painter, icon_box, 12.0, theme::PANEL, theme::BORDER);
    deck.icons.paint(&painter, Icon::TerminalBox, icon_box.center(), 32.0, theme::MUTED, 0.0);
    y = icon_box.max.y + 12.0;

    let tr = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), title.size().y));
    place(&painter, tr, Align2::CENTER_CENTER, title, theme::TEXT);
    y = tr.max.y + 12.0;
    // Com `halign: Center` as linhas ficam centradas em x = 0 da galley.
    let para_h = para.size().y;
    painter.galley(Pos2::new(cx, y), para, theme::MUTED);
    y += para_h + 12.0 + 8.0;

    let mut open = None;
    for row in rows {
        let rw: f32 = row.iter().map(|&i| widths[i]).sum::<f32>() + 10.0 * row.len().saturating_sub(1) as f32;
        let mut x = cx - rw / 2.0;
        for i in row {
            let m = &items[i];
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(widths[i], btn_h));
            x += widths[i] + 10.0;
            let resp = clickable(ui, r, Id::new(("empty-btn", &m.kind)));
            let hot = resp.hovered();
            // :hover → fundo panel-2, borda na cor do agente, sobe 1px
            let rr = if hot { r.translate(Vec2::new(0.0, -1.0)) } else { r };
            frame(&painter, rr, 6.0, if hot { theme::PANEL_2 } else { theme::PANEL }, if hot { m.color } else { theme::BORDER });
            dot(&painter, Pos2::new(rr.min.x + 15.0 + 4.0, rr.center().y), m.color);
            let g = galley(&painter, &m.label, font.clone(), theme::TEXT);
            place(&painter, Rect::from_min_max(Pos2::new(rr.min.x + 15.0 + 8.0 + 8.0 + 6.0, rr.min.y), rr.max), Align2::LEFT_CENTER, g, theme::TEXT);
            if resp.clicked() {
                open = Some(m.kind.clone());
            }
        }
        y += btn_h + 10.0;
    }
    if let Some(kind) = open {
        deck.open_tab(&kind, false);
    }
}

//! `#terminals-empty`: cartão "Nenhum agente aberto" com botões para abrir agentes.

use egui::text::{LayoutJob, TextFormat};
use egui::{Align2, Id, Pos2, Rect, Ui, Vec2};

use super::{button, galley, place, ButtonKind};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

/// Botão "Abrir pasta…" do cartão sem projeto.
const NEW_PROJECT: &str = "#novo-projeto";

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);

    // Sem projeto não há onde abrir um agente: o cartão pede uma pasta.
    let no_project = deck.active_project.is_empty();
    let title = if no_project { "Nenhum projeto aberto" } else { "Nenhum agente aberto" };
    let title = galley(&painter, title, theme::ui_semibold(16.0), theme::pal().text);
    // <p> com o "+ Novo agente" em destaque, quebrando em até 420px (menos na janela estreita).
    let mut job = LayoutJob::default();
    job.wrap.max_width = 420.0f32.min(rect.width() - 48.0).max(120.0);
    job.halign = egui::Align::Center;
    let fmt = TextFormat { font_id: theme::ui(13.0), color: theme::pal().text_2, line_height: Some(13.0 * 1.5), ..Default::default() };
    if no_project {
        job.append("Abra uma pasta como projeto: cada projeto tem os seus agentes.", 0.0, fmt);
    } else {
        // Com a barra lateral recolhida o "+ Novo agente" não está à vista: o atalho é o Ctrl+T.
        let (key, place) = if deck.sidebar_collapsed { ("Ctrl+T", "") } else { ("+ Novo agente", " na barra lateral") };
        job.append("Selecione um agente abaixo ou use ", 0.0, fmt.clone());
        job.append(key, 0.0, TextFormat { font_id: theme::ui_bold(13.0), color: theme::pal().text, ..fmt.clone() });
        job.append(&format!("{place} para iniciar uma sessão:"), 0.0, fmt);
    }
    let para = painter.layout_job(job);

    // Botões (.empty-agent-btn): padding 7px 14px, 12px/500, gap 10px, quebra de linha.
    let items = if no_project {
        vec![crate::agents::MenuItem { kind: NEW_PROJECT.into(), label: "Abrir pasta…".into(), color: theme::pal().text_2 }]
    } else {
        deck.agents.empty_actions.clone()
    };
    let font = theme::ui(12.0);
    let btn_h = 12.0 * theme::LINE_HEIGHT + 14.0 + 2.0;
    // Ícone à esquerda: o logo do modelo ou, sem logo, o ponto do agente; "Abrir pasta…" vai sem ícone.
    let lead = |m: &crate::agents::MenuItem| if m.kind == NEW_PROJECT { 0.0 } else { 8.0 + 8.0 };
    let widths: Vec<f32> = items
        .iter()
        .map(|m| 14.0 + lead(m) + 6.0 + galley(&painter, &m.label, font.clone(), theme::pal().text).size().x + 14.0 + 2.0)
        .collect();
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

    // A marca do app (56 px) no lugar do antigo ícone de terminal
    let icon_box = Rect::from_min_size(Pos2::new(cx - 29.0, y), Vec2::splat(58.0));
    deck.icons.paint(&painter, Icon::AgentDeck, icon_box.center(), 56.0, theme::pal().text_strong, 0.0);
    y = icon_box.max.y + 12.0;

    let tr = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), title.size().y));
    place(&painter, tr, Align2::CENTER_CENTER, title, theme::pal().text);
    y = tr.max.y + 12.0;
    // Com `halign: Center` as linhas ficam centradas em x = 0 da galley.
    let para_h = para.size().y;
    painter.galley(Pos2::new(cx, y), para, theme::pal().text_2);
    y += para_h + 12.0 + 8.0;

    let mut open = None;
    for row in rows {
        let rw: f32 = row.iter().map(|&i| widths[i]).sum::<f32>() + 10.0 * row.len().saturating_sub(1) as f32;
        let mut x = cx - rw / 2.0;
        for i in row {
            let m = &items[i];
            let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(widths[i], btn_h));
            x += widths[i] + 10.0;
            // O botão secundário da base (borda, fundo e hover); o ponto do agente e o nome vão por cima.
            let resp = button(ui, r, Id::new(("empty-btn", &m.kind)), "", ButtonKind::Secondary);
            if m.kind != NEW_PROJECT {
                super::projects::paint_kind_mark(
                    &mut deck.icons,
                    &painter,
                    Pos2::new(r.min.x + 15.0 + 4.0, r.center().y),
                    &m.kind,
                    m.color,
                );
            }
            let g = galley(&painter, &m.label, font.clone(), theme::pal().text);
            place(
                &painter,
                Rect::from_min_max(Pos2::new(r.min.x + 15.0 + lead(m) + 6.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            if resp.clicked() {
                open = Some(m.kind.clone());
            }
        }
        y += btn_h + 10.0;
    }
    match open.as_deref() {
        Some(NEW_PROJECT) => deck.new_project_dialog(),
        Some(kind) => deck.open_tab(kind, false),
        None => {}
    }
}

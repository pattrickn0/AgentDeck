//! Aba "Mapa" da barra direita: quem está conversando com quem agora, em texto monoespaçado
//! (estilo terminal). O conteúdo vem de `agent_map::render_map`; aqui só pinta e trata o clique
//! (clicar num agente ativa a aba dele).

use std::time::Duration;

use egui::text::{LayoutJob, TextFormat};
use egui::{Align, Color32, Pos2, Rect, Sense, Ui, Vec2};

use crate::agent_map::{self, Style};
use crate::deck::Deck;
use crate::theme;

const FONT_SIZE: f32 = 12.5;
const LINE_H: f32 = 17.0;
const PAD_X: f32 = 12.0;

fn color(style: Style) -> Color32 {
    match style {
        Style::Heading => theme::MUTED,
        Style::Name => theme::TEXT,
        Style::Muted => theme::MUTED,
        Style::Busy => theme::OLLAMA,
        Style::Idle => theme::BADGE_JSON,
        Style::Ended => theme::RED_TEXT,
        Style::Id => theme::SEND_DEFAULT,
        Style::Error => theme::RED_TEXT,
        Style::Faded => theme::with_opacity(theme::MUTED, 0.6),
    }
}

/// Largura máxima do mapa em colunas: com a barra alargada, os números das perguntas não ficam
/// longe demais dos nomes.
const MAX_COLS: usize = 64;

/// Quantas colunas cabem em `width` px na fonte do mapa.
pub fn columns(ui: &Ui, width: f32) -> usize {
    let cell = ui.fonts_mut(|f| f.glyph_width(&theme::mono(FONT_SIZE), 'M')).max(1.0);
    (((width - 2.0 * PAD_X) / cell).floor().max(0.0) as usize).min(MAX_COLS)
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    ui.painter().rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let area = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    let now = commanddeck_core::util::now_ms();
    let snap = deck.map_snapshot();
    let lines = agent_map::render_map(&snap, columns(ui, area.width()), now);

    let mut clicked: Option<String> = None;
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(Align::Min)));
    child.set_clip_rect(area);
    egui::ScrollArea::vertical().id_salt("agent-map").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(8.0);
        for line in &lines {
            let sense = if line.node.is_some() { Sense::click() } else { Sense::hover() };
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), LINE_H), sense);
            if line.node.is_some() && resp.hovered() {
                ui.painter().rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 3.0, theme::PANEL_2);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let mut job = LayoutJob::default();
            for (text, style) in &line.spans {
                let font = if *style == Style::Heading { theme::mono_bold(FONT_SIZE) } else { theme::mono(FONT_SIZE) };
                job.append(text, 0.0, TextFormat { font_id: font, color: color(*style), ..Default::default() });
            }
            let g = ui.painter().layout_job(job);
            let y = r.center().y - g.size().y / 2.0;
            ui.painter().galley(Pos2::new(r.min.x + PAD_X, y), g, theme::TEXT);
            if resp.clicked() {
                clicked = line.node.clone();
            }
        }
        ui.add_space(10.0);
        let legend = "● ocupado   ○ livre   × encerrado";
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), LINE_H), Sense::hover());
        ui.painter().text(
            Pos2::new(r.min.x + PAD_X, r.center().y),
            egui::Align2::LEFT_CENTER,
            legend,
            theme::mono(11.0),
            theme::with_opacity(theme::MUTED, 0.7),
        );
        ui.add_space(8.0);
    });

    if let Some(id) = clicked {
        if let Some(idx) = deck.tabs.iter().position(|t| t.ident.id() == id) {
            deck.activate(idx);
        }
    }
    // O tempo de espera e as conversas recentes mudam sozinhos (o 202 nem emite evento).
    if snap.is_live(now) {
        ui.ctx().request_repaint_after(Duration::from_secs(1));
    }
}

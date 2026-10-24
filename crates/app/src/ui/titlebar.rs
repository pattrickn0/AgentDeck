//! Barra de título própria (a janela não tem decoração do sistema e a barra não tem fundo): menu ☰,
//! botão da barra lateral, marca, a chave Agentes | Documentos (só com a barra lateral recolhida),
//! projeto e branch, o uso das assinaturas e minimizar/maximizar/fechar.

use egui::{Align2, CursorIcon, Id, Order, PointerButton, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2, ViewportCommand};

use super::{chip, clickable, galley, galley_spaced, place, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub struct Anchors {
    pub settings_anchor: Rect,
    /// Âncora do menu de novo agente quando a linha da barra lateral não está à vista (Ctrl+T com a barra recolhida).
    pub fallback_anchor: Rect,
}

/// Largura de cada botão da janela (minimizar, maximizar, fechar).
const WIN_BTN_W: f32 = 40.0;
/// Tamanho dos glifos dos botões da janela.
const WIN_GLYPH: f32 = 9.0;
/// Espessura das faixas de redimensionar nas bordas da janela.
const RESIZE_EDGE: f32 = 5.0;
/// Botões de ícone da esquerda (menu, barra lateral).
const ICON_BTN: Vec2 = Vec2::new(26.0, 24.0);
/// Espaço entre os itens da esquerda.
const GAP: f32 = 8.0;

/// A branch a mostrar na etiqueta: o nome, ou o início do commit com o HEAD solto.
fn branch_label(deck: &Deck) -> Option<String> {
    let st = deck.git.status()?;
    st.branch.clone().or_else(|| st.head.as_deref().map(|h| h.chars().take(7).collect()))
}

/// Botão de ícone da barra (26×24, cantos 5, fundo `hover`). Devolve a resposta e se está aceso.
fn icon_button(ui: &Ui, painter: &egui::Painter, rect: Rect, id: &str, lit: bool) -> (egui::Response, bool) {
    let r = clickable(ui, rect, Id::new(id));
    let hot = r.hovered() || lit;
    if hot {
        painter.rect_filled(rect, 5.0, theme::pal().hover);
    }
    (r, hot)
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) -> Anchors {
    let painter = ui.painter_at(rect);
    let p = theme::pal();
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));
    let cy = rect.center().y;

    // ---------- Área de arrastar (por baixo de tudo: os botões, criados depois, ganham o clique) ----------
    let drag = ui.interact(rect, Id::new("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    // ---------- ☰ Menu (configurações) ----------
    let mut x = rect.min.x + theme::ISLAND_GAP + 2.0;
    let menu = Rect::from_min_size(Pos2::new(x, cy - ICON_BTN.y / 2.0), ICON_BTN);
    let (r, hot) = icon_button(ui, &painter, menu, "settings-btn", deck.settings_open);
    let c = if hot { p.text } else { p.text_2 };
    for dy in [-4.0, 0.0, 4.0] {
        let y = menu.center().y + dy;
        painter.line_segment([Pos2::new(menu.center().x - 5.5, y), Pos2::new(menu.center().x + 5.5, y)], Stroke::new(1.5, c));
    }
    if tip(r, "Menu").clicked() {
        deck.settings_open = !deck.settings_open;
        deck.new_tab_menu_open = false;
    }
    x = menu.max.x + GAP;

    // ---------- Barra lateral (recolher/expandir) ----------
    let toggle = Rect::from_min_size(Pos2::new(x, cy - ICON_BTN.y / 2.0), ICON_BTN);
    let (r, hovered) = icon_button(ui, &painter, toggle, "sidebar-toggle", false);
    let expanded = 1.0 - ctx.animate_bool_with_time(Id::new("sidebar-toggle-anim"), deck.sidebar_collapsed, 0.15);
    paint_sidebar_icon(&painter, toggle.center(), if hovered { p.text } else { p.text_2 }, expanded);
    let title = if deck.sidebar_collapsed { "Expandir barra lateral (Ctrl+B)" } else { "Recolher barra lateral (Ctrl+B)" };
    if tip(r, title).clicked() {
        deck.toggle_sidebar();
    }
    x = toggle.max.x + GAP;

    // ---------- Separador, marca, nome do app, projeto e branch ----------
    super::separator(&painter, Pos2::new(x + 0.5, cy - 8.0), Pos2::new(x + 0.5, cy + 8.0));
    x += 1.0 + GAP;
    deck.icons.paint(&painter, Icon::AgentDeck, Pos2::new(x + 8.0, cy), 16.0, p.text_strong, 0.0);
    x += 16.0 + GAP;
    let name = galley_spaced(&painter, "AgentDeck", theme::ui_bold(13.0), p.text, -0.26);
    x = place(&painter, Rect::from_min_max(Pos2::new(x, rect.min.y), Pos2::new(rect.max.x, rect.max.y)), Align2::LEFT_CENTER, name, p.text)
        .max
        .x
        + GAP;
    let project = deck.project_views().into_iter().find(|v| v.active).map(|v| v.name);
    if let Some(project) = project {
        let slash = galley(&painter, "/", theme::ui(12.0), p.text_faint);
        x = place(&painter, Rect::from_min_max(Pos2::new(x, rect.min.y), rect.max), Align2::LEFT_CENTER, slash, p.text_faint).max.x + GAP;
        let g = galley(&painter, &project, theme::ui(12.0), p.text_2);
        x = place(&painter, Rect::from_min_max(Pos2::new(x, rect.min.y), rect.max), Align2::LEFT_CENTER, g, p.text_2).max.x + GAP;
        if let Some(branch) = branch_label(deck) {
            x = chip(&painter, Pos2::new(x, cy), &branch, 10.5, p.text_soft).max.x;
        }
    }
    // Com a barra lateral recolhida a chave Agentes | Documentos volta para cá, compacta e depois do nome do
    // projeto (aberta, ela é a primeira coisa da barra lateral).
    if deck.sidebar_collapsed {
        let size = Vec2::new(super::docs::switch_compact_width(&painter, deck), super::docs::SWITCH_COMPACT_H);
        super::docs::switch(deck, ui, Rect::from_min_size(Pos2::new(x + GAP, cy - size.y / 2.0), size), "title-center");
        x += GAP + size.x + GAP;
    }

    // ---------- Botões da janela ----------
    let close = Rect::from_min_max(Pos2::new(rect.max.x - WIN_BTN_W, rect.min.y), rect.max);
    let max_btn = close.translate(Vec2::new(-WIN_BTN_W, 0.0));
    let min_btn = max_btn.translate(Vec2::new(-WIN_BTN_W, 0.0));
    if window_button(ui, &painter, min_btn, "win-min", Glyph::Minimize, false) {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
    let glyph = if maximized { Glyph::Restore } else { Glyph::Maximize };
    if window_button(ui, &painter, max_btn, "win-max", glyph, false) {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    }
    if window_button(ui, &painter, close, "win-close", Glyph::Close, true) {
        deck.request_quit();
    }

    // ---------- Uso das assinaturas (à esquerda dos botões da janela) ----------
    let usage = Rect::from_min_max(Pos2::new(x + 12.0, rect.min.y), Pos2::new(min_btn.min.x - 6.0, rect.max.y));
    if usage.width() > 0.0 {
        super::usage::draw(deck, ui, usage);
    }

    if !maximized {
        resize_edges(&ctx);
    }

    Anchors { settings_anchor: menu, fallback_anchor: Rect::from_min_size(Pos2::new(toggle.max.x, rect.max.y), Vec2::new(34.0, 0.0)) }
}

/// Ícone "painel lateral": janela arredondada (12×10) com a coluna da esquerda, preenchida conforme a
/// barra está aberta (`expanded` de 0 a 1, animado).
fn paint_sidebar_icon(painter: &egui::Painter, center: Pos2, color: egui::Color32, expanded: f32) {
    let frame = Rect::from_center_size(center, Vec2::new(12.0, 10.0));
    let col = Rect::from_min_max(frame.min, Pos2::new(frame.min.x + 4.0, frame.max.y));
    if expanded > 0.0 {
        let fill = col.shrink(1.5);
        let fill = Rect::from_min_max(fill.min, Pos2::new(fill.min.x + fill.width() * expanded, fill.max.y));
        painter.rect_filled(fill, 1.0, theme::with_opacity(color, 0.85));
    }
    painter.rect_stroke(frame, 2.0, Stroke::new(1.4, color), egui::StrokeKind::Inside);
    painter.line_segment([Pos2::new(col.max.x, frame.min.y + 1.0), Pos2::new(col.max.x, frame.max.y - 1.0)], Stroke::new(1.4, color));
}

#[derive(Clone, Copy)]
enum Glyph {
    Minimize,
    Maximize,
    Restore,
    Close,
}

/// Botão da janela (largura `WIN_BTN_W`, altura da barra). Devolve `true` no clique.
fn window_button(ui: &Ui, painter: &egui::Painter, rect: Rect, id: &str, glyph: Glyph, danger: bool) -> bool {
    let r = ui.interact(rect, Id::new(id), Sense::click());
    let hot = r.hovered();
    if hot {
        painter.rect_filled(rect, 0.0, if danger { theme::pal().danger } else { theme::pal().hover });
    }
    let c = if hot && danger {
        theme::pal().text_strong
    } else if hot {
        theme::pal().text
    } else {
        theme::pal().text_3
    };
    let s = Stroke::new(1.0, c);
    let ctr = rect.center();
    let h = WIN_GLYPH / 2.0;
    match glyph {
        Glyph::Minimize => {
            painter.line_segment([Pos2::new(ctr.x - h, ctr.y), Pos2::new(ctr.x + h, ctr.y)], s);
        }
        Glyph::Maximize => {
            painter.rect_stroke(Rect::from_center_size(ctr, Vec2::splat(WIN_GLYPH)), 1.0, s, egui::StrokeKind::Inside);
        }
        Glyph::Restore => {
            // Dois quadrados sobrepostos: o de trás só aparece acima e à direita.
            let front = Rect::from_center_size(ctr + Vec2::new(-1.0, 1.0), Vec2::splat(WIN_GLYPH - 2.0));
            painter.rect_stroke(front, 1.0, s, egui::StrokeKind::Inside);
            let (x0, y0, x1, y1) = (front.min.x + 2.0, front.min.y - 2.0, front.max.x + 2.0, front.max.y - 2.0);
            painter.line_segment([Pos2::new(x0, front.min.y), Pos2::new(x0, y0)], s);
            painter.line_segment([Pos2::new(x0, y0), Pos2::new(x1, y0)], s);
            painter.line_segment([Pos2::new(x1, y0), Pos2::new(x1, y1)], s);
            painter.line_segment([Pos2::new(x1, y1), Pos2::new(front.max.x, y1)], s);
        }
        Glyph::Close => {
            painter.line_segment([ctr + Vec2::new(-h, -h), ctr + Vec2::new(h, h)], s);
            painter.line_segment([ctr + Vec2::new(-h, h), ctr + Vec2::new(h, -h)], s);
        }
    }
    let label = match glyph {
        Glyph::Minimize => "Minimizar",
        Glyph::Maximize => "Maximizar",
        Glyph::Restore => "Restaurar",
        Glyph::Close => "Fechar",
    };
    tip(r, label).clicked()
}

/// Faixas nas bordas e cantos da janela para redimensionar (sem decoração, o sistema não oferece).
/// Cada uma numa camada de primeiro plano: ficam acima da barra direita e dos terminais.
fn resize_edges(ctx: &egui::Context) {
    let s = ctx.content_rect();
    let e = RESIZE_EDGE;
    let c = 2.0 * e;
    let edges = [
        (Rect::from_min_max(s.min, Pos2::new(s.min.x + c, s.min.y + c)), ResizeDirection::NorthWest, CursorIcon::ResizeNorthWest),
        (
            Rect::from_min_max(Pos2::new(s.max.x - c, s.min.y), Pos2::new(s.max.x, s.min.y + c)),
            ResizeDirection::NorthEast,
            CursorIcon::ResizeNorthEast,
        ),
        (
            Rect::from_min_max(Pos2::new(s.min.x, s.max.y - c), Pos2::new(s.min.x + c, s.max.y)),
            ResizeDirection::SouthWest,
            CursorIcon::ResizeSouthWest,
        ),
        (Rect::from_min_max(Pos2::new(s.max.x - c, s.max.y - c), s.max), ResizeDirection::SouthEast, CursorIcon::ResizeSouthEast),
        (
            Rect::from_min_max(Pos2::new(s.min.x + c, s.min.y), Pos2::new(s.max.x - c, s.min.y + e)),
            ResizeDirection::North,
            CursorIcon::ResizeNorth,
        ),
        (
            Rect::from_min_max(Pos2::new(s.min.x + c, s.max.y - e), Pos2::new(s.max.x - c, s.max.y)),
            ResizeDirection::South,
            CursorIcon::ResizeSouth,
        ),
        (
            Rect::from_min_max(Pos2::new(s.min.x, s.min.y + c), Pos2::new(s.min.x + e, s.max.y - c)),
            ResizeDirection::West,
            CursorIcon::ResizeWest,
        ),
        (
            Rect::from_min_max(Pos2::new(s.max.x - e, s.min.y + c), Pos2::new(s.max.x, s.max.y - c)),
            ResizeDirection::East,
            CursorIcon::ResizeEast,
        ),
    ];
    for (i, (rect, dir, cursor)) in edges.into_iter().enumerate() {
        egui::Area::new(Id::new(("resize-edge", i))).order(Order::Foreground).fixed_pos(rect.min).constrain(false).show(ctx, |ui| {
            let (_, r) = ui.allocate_exact_size(rect.size(), Sense::drag());
            if r.hovered() || r.dragged() {
                ctx.set_cursor_icon(cursor);
            }
            if r.drag_started_by(PointerButton::Primary) {
                ctx.send_viewport_cmd(ViewportCommand::BeginResize(dir));
            }
        });
    }
}

//! Barra de título própria (a janela não tem decoração do sistema): menu ☰, nome, botão da
//! barra lateral, "+", tela dividida, área de arrastar, uso dos agentes e minimizar/maximizar/fechar.

use egui::{Align2, CursorIcon, Id, Order, PointerButton, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2, ViewportCommand};

use super::{clickable, galley, place, text_in, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub struct Anchors {
    pub new_tab_anchor: Rect,
    pub settings_anchor: Rect,
}

/// Largura de cada botão da janela (minimizar, maximizar, fechar).
const WIN_BTN_W: f32 = 34.0;
/// Tamanho dos glifos dos botões da janela.
const WIN_GLYPH: f32 = 9.0;
/// Espessura das faixas de redimensionar nas bordas da janela.
const RESIZE_EDGE: f32 = 5.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) -> Anchors {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::PANEL);
    let inner = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, rect.max.y - 1.0));
    painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - 1.0), rect.max), 0.0, theme::BORDER);
    let ctx = ui.ctx().clone();
    let maximized = ctx.input(|i| i.viewport().maximized.unwrap_or(false));

    // ---------- Área de arrastar (por baixo de tudo: os botões, criados depois, ganham o clique) ----------
    let drag = ui.interact(inner, Id::new("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    // ---------- ☰ Menu (configurações) ----------
    let menu = Rect::from_min_size(inner.min, Vec2::new(40.0, inner.height()));
    let r = clickable(ui, menu, Id::new("settings-btn"));
    let hot = r.hovered() || deck.settings_open;
    if hot {
        painter.rect_filled(menu, 0.0, theme::PANEL_2);
    }
    let c = if hot { theme::TEXT } else { theme::MUTED };
    for dy in [-4.0, 0.0, 4.0] {
        let y = menu.center().y + dy;
        painter.line_segment([Pos2::new(menu.center().x - 7.0, y), Pos2::new(menu.center().x + 7.0, y)], Stroke::new(1.5, c));
    }
    if tip(r, "Menu").clicked() {
        deck.settings_open = !deck.settings_open;
        deck.new_tab_menu_open = false;
    }

    // ---------- Nome ----------
    let title_r = text_in(
        &painter,
        Rect::from_min_max(Pos2::new(menu.max.x + 4.0, inner.min.y), inner.max),
        Align2::LEFT_CENTER,
        "CommandDeck",
        theme::ui_semibold(13.0),
        theme::TEXT,
    );

    // ---------- Barra lateral (recolher/expandir) ----------
    let toggle = Rect::from_min_size(Pos2::new(title_r.max.x + 14.0, inner.min.y), Vec2::new(30.0, inner.height()));
    let r = clickable(ui, toggle, Id::new("sidebar-toggle"));
    let hovered = r.hovered();
    if hovered {
        painter.rect_filled(toggle, 0.0, theme::PANEL_2);
    }
    let t = ctx.animate_bool_with_time(Id::new("sidebar-chevron"), deck.sidebar_collapsed, 0.15);
    deck.icons.paint(
        &painter,
        Icon::SidebarChevron,
        Pos2::new(toggle.center().x - 0.5, toggle.center().y),
        9.0,
        if hovered { theme::TEXT } else { theme::MUTED },
        t * std::f32::consts::PI,
    );
    let title = if deck.sidebar_collapsed { "Expandir barra lateral (Ctrl+B)" } else { "Recolher barra lateral (Ctrl+B)" };
    if tip(r, title).clicked() {
        deck.toggle_sidebar();
    }

    // ---------- Nova aba (+) (as abas ficam no cabeçalho de cada janela; ver `panes.rs`) ----------
    let plus = Rect::from_min_size(Pos2::new(toggle.max.x, inner.min.y), Vec2::new(34.0, inner.height()));
    let r = clickable(ui, plus, Id::new("new-tab"));
    let hot = r.hovered() || deck.new_tab_menu_open;
    if hot {
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

    // ---------- Tela dividida (liga/desliga) ----------
    let split_btn = Rect::from_min_size(Pos2::new(plus.max.x, inner.min.y), Vec2::new(34.0, inner.height()));
    let r = clickable(ui, split_btn, Id::new("split-toggle"));
    let on = deck.split_enabled;
    if r.hovered() {
        painter.rect_filled(split_btn, 0.0, theme::PANEL_2);
    }
    let c = if on {
        deck.active_color()
    } else if r.hovered() {
        theme::TEXT
    } else {
        theme::MUTED
    };
    deck.icons.paint(&painter, Icon::Split, split_btn.center(), 15.0, c, 0.0);
    let title = if on {
        "Tela dividida: ligada\nCada tipo de agente numa janela · clique para ver uma aba por vez"
    } else {
        "Tela dividida: desligada\nClique para abrir uma janela por tipo de agente"
    };
    if tip(r, title).clicked() {
        deck.toggle_split();
    }

    // ---------- Botões da janela ----------
    let close = Rect::from_min_max(Pos2::new(inner.max.x - WIN_BTN_W, inner.min.y), inner.max);
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
        deck.quit_requested = true;
    }

    // ---------- Uso dos agentes (à esquerda dos botões da janela) ----------
    let usage = Rect::from_min_max(Pos2::new(split_btn.max.x + 12.0, inner.min.y), Pos2::new(min_btn.min.x - 10.0, inner.max.y));
    if usage.width() > 0.0 {
        super::usage::draw(deck, ui, usage);
    }

    if !maximized {
        resize_edges(&ctx);
    }

    Anchors { new_tab_anchor: plus, settings_anchor: menu }
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
        painter.rect_filled(rect, 0.0, if danger { theme::RED } else { theme::PANEL_2 });
    }
    let c = if hot && danger {
        theme::WHITE
    } else if hot {
        theme::TEXT
    } else {
        theme::MUTED
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

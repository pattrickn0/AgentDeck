//! The app's own title bar (the window has no system decoration and the bar has no background): ☰ menu,
//! sidebar button, logo, the Agents | Documents switch (only with the sidebar collapsed),
//! project and branch, subscription usage and minimize/maximize/close.

use egui::{Align2, CursorIcon, Id, Order, PointerButton, Pos2, Rect, ResizeDirection, Sense, Stroke, Ui, Vec2, ViewportCommand};

use super::{chip, clickable, galley, galley_spaced, place, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;
use holt_core::t;

pub struct Anchors {
    pub settings_anchor: Rect,
    /// Anchor of the new-agent menu when the sidebar's row is not in view (Ctrl+T with the sidebar collapsed).
    pub fallback_anchor: Rect,
}

/// Width of each window button (minimize, maximize, close).
const WIN_BTN_W: f32 = 40.0;
/// Size of the window buttons' glyphs.
const WIN_GLYPH: f32 = 9.0;
/// Thickness of the resize strips on the window edges.
const RESIZE_EDGE: f32 = 5.0;
/// Icon buttons on the left (menu, sidebar).
const ICON_BTN: Vec2 = Vec2::new(26.0, 24.0);
/// Space between the items on the left.
const GAP: f32 = 8.0;

/// The branch to show on the chip: its name, or the start of the commit with a detached HEAD.
fn branch_label(deck: &Deck) -> Option<String> {
    let st = deck.git.status()?;
    st.branch.clone().or_else(|| st.head.as_deref().map(|h| h.chars().take(7).collect()))
}

/// Icon button of the bar (26×24, corners 5, `hover` background). Returns the response and whether it is lit.
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

    // ---------- Drag area (under everything: the buttons, created later, win the click) ----------
    let drag = ui.interact(rect, Id::new("titlebar-drag"), Sense::click_and_drag());
    if drag.double_clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized));
    } else if drag.drag_started_by(PointerButton::Primary) {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }

    // ---------- ☰ Menu (settings) ----------
    let mut x = rect.min.x + theme::ISLAND_GAP + 2.0;
    let menu = Rect::from_min_size(Pos2::new(x, cy - ICON_BTN.y / 2.0), ICON_BTN);
    let (r, hot) = icon_button(ui, &painter, menu, "settings-btn", deck.settings_open);
    let c = if hot { p.text } else { p.text_2 };
    for dy in [-4.0, 0.0, 4.0] {
        let y = menu.center().y + dy;
        painter.line_segment([Pos2::new(menu.center().x - 5.5, y), Pos2::new(menu.center().x + 5.5, y)], Stroke::new(1.5, c));
    }
    if tip(r, t!("Menu")).clicked() {
        deck.settings_open = !deck.settings_open;
        deck.new_tab_menu_open = false;
    }
    x = menu.max.x + GAP;

    // ---------- Sidebar (collapse/expand) ----------
    let toggle = Rect::from_min_size(Pos2::new(x, cy - ICON_BTN.y / 2.0), ICON_BTN);
    let (r, hovered) = icon_button(ui, &painter, toggle, "sidebar-toggle", false);
    let expanded = 1.0 - ctx.animate_bool_with_time(Id::new("sidebar-toggle-anim"), deck.sidebar_collapsed, 0.15);
    paint_sidebar_icon(&painter, toggle.center(), if hovered { p.text } else { p.text_2 }, expanded);
    let title = if deck.sidebar_collapsed { t!("Expand sidebar (Ctrl+B)") } else { t!("Collapse sidebar (Ctrl+B)") };
    if tip(r, title).clicked() {
        deck.toggle_sidebar();
    }
    x = toggle.max.x + GAP;

    // ---------- Separator, logo, app name, project and branch ----------
    super::separator(&painter, Pos2::new(x + 0.5, cy - 8.0), Pos2::new(x + 0.5, cy + 8.0));
    x += 1.0 + GAP;
    deck.icons.paint(&painter, Icon::Holt, Pos2::new(x + 8.0, cy), 16.0, p.text_strong, 0.0);
    x += 16.0 + GAP;
    let name = galley_spaced(&painter, "Holt", theme::ui_bold(13.0), p.text, -0.26);
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
    // With the sidebar collapsed the Agents | Documents switch comes back here, compact and after the
    // project name (expanded, it is the first thing in the sidebar).
    if deck.sidebar_collapsed {
        let size = Vec2::new(super::docs::switch_compact_width(&painter, deck), super::docs::SWITCH_COMPACT_H);
        super::docs::switch(deck, ui, Rect::from_min_size(Pos2::new(x + GAP, cy - size.y / 2.0), size), "title-center");
        x += GAP + size.x + GAP;
    }

    // ---------- Window buttons ----------
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
        deck.close_requested = true;
    }

    // ---------- Subscription usage (left of the window buttons) ----------
    let usage = Rect::from_min_max(Pos2::new(x + 12.0, rect.min.y), Pos2::new(min_btn.min.x - 6.0, rect.max.y));
    if usage.width() > 0.0 {
        super::usage::draw(deck, ui, usage);
    }

    if !maximized {
        resize_edges(&ctx);
    }

    Anchors { settings_anchor: menu, fallback_anchor: Rect::from_min_size(Pos2::new(toggle.max.x, rect.max.y), Vec2::new(34.0, 0.0)) }
}

/// "Side panel" icon: a rounded window (12×10) with the left column, filled as far as the
/// bar is open (`expanded` from 0 to 1, animated).
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

/// Window button (`WIN_BTN_W` wide, as tall as the bar). Returns `true` on a click.
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
            // Two overlapping squares: the one behind only shows above and to the right.
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
        Glyph::Minimize => t!("Minimize"),
        Glyph::Maximize => t!("Maximize"),
        Glyph::Restore => t!("Restore"),
        Glyph::Close => t!("Close"),
    };
    tip(r, label).clicked()
}

/// Strips on the window's edges and corners to resize it (without decoration, the system offers none).
/// Each one on a foreground layer: they sit above the right bar and the terminals.
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

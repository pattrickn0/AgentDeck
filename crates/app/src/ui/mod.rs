//! Interface painting (the equivalent of `index.html` + `styles.css`).
//! The layout is computed by hand to reproduce the CSS measurements to the pixel.

mod background;
mod chat;
mod code;
mod diff_view;
mod docs;
mod empty_state;
mod git;
mod git_views;
mod github;
mod menus;
mod notices;
mod overlay;
mod panes;
mod projects;
mod right_panel;
mod select;
mod shortcuts;
mod sidebar;
mod tabs;
mod tasks;
mod titlebar;
mod usage;

use std::sync::Arc;

use egui::{Align2, Color32, CursorIcon, FontId, Galley, Id, Order, Painter, Pos2, Rect, Response, Sense, Stroke, Ui, Vec2};

use crate::deck::Deck;
use crate::theme;

pub use panes::pane_body;

/// Minimum width left for the terminal next to the right bar.
const TERM_MIN_W: f32 = 360.0;

/// Effective width of the right bar and the maximum the window allows now. The saved value is
/// not changed: if the window grows again, the bar returns to the chosen width.
pub fn right_width(saved: f32, win_w: f32, left: f32) -> (f32, f32) {
    let max = theme::RIGHT_MAX_W.min(win_w * 0.5).min(win_w - left - TERM_MIN_W).max(260.0);
    (saved.max(theme::RIGHT_MIN_W).min(max), max)
}

/// Draws the whole window.
pub fn draw(deck: &mut Deck, ui: &mut Ui) {
    let window = ui.max_rect();
    // The accent color of the frame (the one of the focused agent) applies to everything drawn below.
    theme::set_accent(deck.accent_opt());
    background::draw(ui.painter(), window, deck.accent());
    theme::sync_selection(ui.ctx(), deck.accent());
    let ctx = ui.ctx().clone();
    // Each modal drawn in this frame sets it again (see `modal_backdrop`).
    deck.modal_open = false;

    // ---------- Layout: title bar on top; below, the islands (left bar | center | right bar) ----------
    let gap = theme::ISLAND_GAP;
    let titlebar = Rect::from_min_size(window.min, Vec2::new(window.width(), theme::TITLEBAR_H));
    let body = Rect::from_min_max(Pos2::new(window.min.x + gap, titlebar.max.y), Pos2::new(window.max.x - gap, window.max.y - gap));
    let collapsed = deck.sidebar_collapsed;
    let sw = deck.sidebar_w.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W);
    let left = if collapsed { 0.0 } else { sw + gap };
    let (right_w, right_max) = right_width(deck.right_w, body.width(), left + gap);
    let center = Rect::from_min_max(Pos2::new(body.min.x + left, body.min.y), Pos2::new(body.max.x - right_w - gap, body.max.y));
    let side = Rect::from_min_size(body.min, Vec2::new(sw, body.height()));
    // The bar always has the same width: the lists and the Git diff scroll inside it.
    let panel = Rect::from_min_max(Pos2::new(body.max.x - right_w, body.min.y), body.max);
    deck.right_panel_rect = Some(panel);

    // ---------- Island shadows (all before any background, only outside the islands) ----------
    deck.docs.sync(&deck.active_project);
    let docs_mode = deck.docs.view == crate::docs::CenterView::Docs;
    let mut islands = if docs_mode { vec![center] } else { panes::island_rects(deck, center) };
    islands.push(panel);
    if !collapsed {
        islands.push(side);
    }
    island_shadows(ui.painter(), Rect::from_min_max(Pos2::new(window.min.x, titlebar.max.y), window.max), &islands);

    // ---------- Center: the terminals (split by agent type) or the documents ----------
    if docs_mode {
        // The terminals keep running hidden, and the PTY is not resized: no `fit_tab` here.
        deck.terminal_rect = Some(panes::pane_body(center));
        deck.panes.clear();
        deck.pane_drag = None;
        island(ui, center, IslandKind::Well);
        docs::draw(deck, ui, center);
        island_edge(ui, center, IslandKind::Well);
    } else {
        panes::draw(deck, ui, center);
    }

    // ---------- Sidebar ----------
    if !collapsed {
        island(ui, side, IslandKind::Glass);
        sidebar::draw(deck, ui, side);
        island_edge(ui, side, IslandKind::Glass);
    }

    // ---------- Title bar ----------
    let menus = titlebar::draw(deck, ui, titlebar);

    // ---------- Splitters (last in the background: they get the click in the gap between the islands) ----------
    if !collapsed {
        sidebar::splitter(deck, ui, Rect::from_min_size(Pos2::new(side.max.x, body.min.y), Vec2::new(gap, body.height())), body);
    }
    let line = Rect::from_min_size(Pos2::new(center.max.x, body.min.y), Vec2::new(gap, body.height()));
    right_panel::splitter(deck, ui, line, body, right_w, right_max);

    // ---------- Right bar (on a layer above the terminals) ----------
    egui::Area::new(Id::new("right-panel")).order(Order::Middle).fixed_pos(panel.min).show(&ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(panel.size(), Sense::hover());
        island(ui, rect, IslandKind::Glass);
        right_panel::draw_panel(deck, ui, rect);
        island_edge(ui, rect, IslandKind::Glass);
    });

    // ---------- Popup menus and overlay ----------
    menus::new_tab_menu(deck, &ctx, projects::new_agent_anchor(&ctx, menus.fallback_anchor));
    menus::settings_menu(deck, &ctx, menus.settings_anchor);
    menus::tab_context_menu(deck, &ctx);
    menus::rename_tab_modal(deck, &ctx);
    docs::unsaved_modal(deck, &ctx);
    menus::dictation_modal(deck, &ctx);
    shortcuts::shortcuts_modal(deck, &ctx);
    notices::draw(deck, &ctx, menus.notices_anchor, Pos2::new(panel.max.x - 8.0, panel.max.y - notices::TOAST_BOTTOM));
    overlay::drop_overlay(deck, &ctx, window);

    // An egui text field (e.g. the task form or the commit) has keyboard focus → the terminal does not.
    let egui_focus = ctx.memory(|m| m.focused());
    if egui_focus.is_some() {
        deck.term_focus = false;
        deck.ime_preedit.clear();
    }
}

// ---------------------------------------------------------------------------
// Islands
// ---------------------------------------------------------------------------

/// The look of an island: translucent glass (bars) or the almost solid well of the terminal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IslandKind {
    Glass,
    Well,
}

impl IslandKind {
    fn fill(self) -> Color32 {
        match self {
            IslandKind::Glass => theme::pal().glass,
            IslandKind::Well => theme::pal().well,
        }
    }
}

fn island_radius() -> f32 {
    theme::ISLAND_RADIUS as f32
}

/// `area` minus each rectangle of `holes`: only the gaps and margins are left, where the shadow can show.
pub fn free_rects(area: Rect, holes: &[Rect]) -> Vec<Rect> {
    let mut free = vec![area];
    for hole in holes {
        free = free
            .into_iter()
            .flat_map(|r| {
                let h = r.intersect(*hole);
                if h.width() <= 0.0 || h.height() <= 0.0 {
                    return vec![r];
                }
                let mut parts = Vec::with_capacity(4);
                if h.min.y > r.min.y {
                    parts.push(Rect::from_min_max(r.min, Pos2::new(r.max.x, h.min.y)));
                }
                if h.max.y < r.max.y {
                    parts.push(Rect::from_min_max(Pos2::new(r.min.x, h.max.y), r.max));
                }
                if h.min.x > r.min.x {
                    parts.push(Rect::from_min_max(Pos2::new(r.min.x, h.min.y), Pos2::new(h.min.x, h.max.y)));
                }
                if h.max.x < r.max.x {
                    parts.push(Rect::from_min_max(Pos2::new(h.max.x, h.min.y), Pos2::new(r.max.x, h.max.y)));
                }
                parts
            })
            .collect();
    }
    free
}

/// Soft shadow (y 8, blur 24) of each island, only in the gaps and margins of `area`: the glass is
/// translucent, so a shadow under it would darken it (in the CSS it is clipped inside).
pub fn island_shadows(painter: &Painter, area: Rect, islands: &[Rect]) {
    let shape =
        |r: Rect| egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: theme::pal().shadow }.as_shape(r, theme::ISLAND_RADIUS);
    for free in free_rects(area, islands) {
        let clipped = painter.with_clip_rect(free.intersect(painter.clip_rect()));
        // Only the islands whose shadow (up to 32 px) reaches this leftover.
        for r in islands.iter().filter(|r| r.expand(32.0).intersects(free)) {
            clipped.add(shape(*r));
        }
    }
}

/// Background of an island: 10 px corners. Draw the content afterwards and call [`island_edge`] last.
pub fn island(ui: &Ui, rect: Rect, kind: IslandKind) {
    ui.painter().rect_filled(rect, island_radius(), kind.fill());
}

/// Finish of an island, after the content: repaints the background at the four corners (the content is
/// rectangular and would leak through the rounded corner), the 1 px border and, on glass, the light line at the inner top.
pub fn island_edge(ui: &Ui, rect: Rect, kind: IslandKind) {
    let painter = ui.painter();
    let window = ui.ctx().content_rect();
    let accent = theme::accent();
    let r = island_radius();
    // Each corner: the fan between the square's vertex and the arc of the corner circle.
    let mut mesh = egui::epaint::Mesh::default();
    for (corner, dx, dy) in
        [(rect.left_top(), 1.0, 1.0), (rect.right_top(), -1.0, 1.0), (rect.left_bottom(), 1.0, -1.0), (rect.right_bottom(), -1.0, -1.0)]
    {
        let center = corner + Vec2::new(dx * r, dy * r);
        let mut vertex = |pos: Pos2| {
            mesh.vertices.push(egui::epaint::Vertex { pos, uv: egui::epaint::WHITE_UV, color: background::color_at(pos, window, accent) });
            mesh.vertices.len() as u32 - 1
        };
        let apex = vertex(corner);
        const STEPS: usize = 8;
        // From the point on the vertical edge to the point on the horizontal edge.
        let arc: Vec<u32> = (0..=STEPS)
            .map(|i| {
                let th = i as f32 / STEPS as f32 * std::f32::consts::FRAC_PI_2;
                vertex(center + Vec2::new(-dx * r * th.sin(), -dy * r * th.cos()))
            })
            .collect();
        for w in arc.windows(2) {
            mesh.add_triangle(apex, w[0], w[1]);
        }
    }
    painter.add(egui::Shape::mesh(mesh));
    // Inner line at the top, between the corners (only glass has it; the well does not).
    if kind == IslandKind::Glass {
        painter.line_segment(
            [Pos2::new(rect.min.x + r, rect.min.y + 1.0), Pos2::new(rect.max.x - r, rect.min.y + 1.0)],
            Stroke::new(1.0, theme::pal().glass_highlight),
        );
    }
    painter.rect_stroke(rect, r, Stroke::new(1.0, theme::pal().glass_border), egui::StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// Painting utilities
// ---------------------------------------------------------------------------

/// Darkened background of a modal. It sits on a layer above everything except the modal itself and holds
/// the click, the drag and the hover of what is behind; it sets `modal_open`, which takes the keyboard away from
/// the terminal and from the app shortcuts while the modal is open.
pub fn modal_backdrop(deck: &mut Deck, ctx: &egui::Context, id: &str) {
    deck.modal_open = true;
    let screen = ctx.content_rect();
    egui::Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(screen.min).constrain(false).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(screen.size(), Sense::click_and_drag());
        ui.painter().rect_filled(r, 0.0, theme::pal().scrim);
    });
}

/// Text on a single line (no wrapping).
pub fn galley(painter: &Painter, text: &str, font: FontId, color: Color32) -> Arc<Galley> {
    painter.layout_no_wrap(text.to_owned(), font, color)
}

/// Text on a single line with an ellipsis (`text-overflow: ellipsis`).
pub fn galley_trunc(painter: &Painter, text: &str, font: FontId, color: Color32, max_w: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping { max_width: max_w.max(1.0), max_rows: 1, break_anywhere: true, overflow_character: Some('…') };
    painter.layout_job(job)
}

/// Text on a single line with extra spacing between the letters (`letter-spacing`, in px).
pub fn galley_spaced(painter: &Painter, text: &str, font: FontId, color: Color32, spacing: f32) -> Arc<Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(text, 0.0, egui::TextFormat { font_id: font, color, extra_letter_spacing: spacing, ..Default::default() });
    painter.layout_job(job)
}

/// Width of the text on a single line.
pub fn text_w(painter: &Painter, text: &str, font: FontId) -> f32 {
    galley(painter, text, font, Color32::WHITE).size().x
}

/// Paints the galley aligned inside `rect` (vertically centered) and returns the rectangle it occupies.
pub fn place(painter: &Painter, rect: Rect, align: Align2, g: Arc<Galley>, color: Color32) -> Rect {
    let r = align.align_size_within_rect(g.size(), rect);
    painter.galley(r.min, g, color);
    r
}

/// Text aligned inside a rectangle.
pub fn text_in(painter: &Painter, rect: Rect, align: Align2, text: &str, font: FontId, color: Color32) -> Rect {
    let g = galley(painter, text, font, color);
    place(painter, rect, align, g, color)
}

/// `.dot` (8×8, circle).
pub fn dot(painter: &Painter, center: Pos2, color: Color32) {
    painter.circle_filled(center, 4.0, color);
}

/// `box-shadow: 0 <y>px <blur>px <color>`.
pub fn shadow(painter: &Painter, rect: Rect, radius: u8, offset_y: i8, blur: u8, color: Color32) {
    let s = egui::Shadow { offset: [0, offset_y], blur, spread: 0, color };
    painter.add(s.as_shape(rect, radius));
}

/// Clickable area with a hand cursor (like `cursor: pointer`).
pub fn clickable(ui: &Ui, rect: Rect, id: Id) -> Response {
    let r = ui.interact(rect, id, Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    r
}

/// Tooltip in the style of the `title` attribute.
pub fn tip(resp: Response, text: impl Into<String>) -> Response {
    let text = text.into();
    resp.on_hover_ui(|ui| {
        ui.label(egui::RichText::new(text).font(theme::ui(11.5)).color(theme::pal().text));
    })
}

/// The 2 px wire in the middle of a gap between islands (vertical or horizontal, according to the gap).
fn splitter_wire(line: Rect) -> Rect {
    if line.width() < line.height() {
        Rect::from_center_size(line.center(), Vec2::new(2.0, line.height()))
    } else {
        Rect::from_center_size(line.center(), Vec2::new(line.width(), 2.0))
    }
}

/// Draggable splitter between the islands (`line` is the whole gap): it only shows on hover or
/// drag, like a wire that glows in the active agent's color.
pub fn paint_splitter(painter: &Painter, line: Rect, hot: bool, color: Color32) {
    if !hot {
        return;
    }
    let wire = splitter_wire(line);
    // box-shadow: 0 0 3px <cor>
    for (grow, alpha) in [(2.0, 0.12), (1.0, 0.25)] {
        painter.rect_filled(wire.expand(grow), 1.0, theme::with_opacity(color, alpha));
    }
    painter.rect_filled(wire, 1.0, color);
}

/// Rounded frame with a 1 px border.
pub fn frame(painter: &Painter, rect: Rect, radius: f32, fill: Color32, border: Color32) {
    painter.rect(rect, radius, fill, Stroke::new(1.0, border), egui::StrokeKind::Inside);
}

// ---------------------------------------------------------------------------
// Common components (for parts 2 and 3 of the visual)
// ---------------------------------------------------------------------------

/// 1 px divider line inside an island, from `a` to `b` (horizontal or vertical).
pub fn separator(painter: &Painter, a: Pos2, b: Pos2) {
    painter.line_segment([a, b], Stroke::new(1.0, theme::pal().separator));
}

/// Label (branch, counter): `chip` background, 4 px corners, IBM Plex Mono text. `at` is the
/// left tip, vertically centered on `at.y`; returns the rectangle it occupies.
pub fn chip(painter: &Painter, at: Pos2, text: &str, size: f32, color: Color32) -> Rect {
    let g = galley(painter, text, theme::meta(size), color);
    let rect = Rect::from_min_size(Pos2::new(at.x, at.y - 7.0), Vec2::new(g.size().x + 12.0, 14.0));
    painter.rect_filled(rect, 4.0, theme::pal().chip);
    place(painter, rect, Align2::CENTER_CENTER, g, color);
    rect
}

/// Height of a [`section_label`] row.
pub const SECTION_LABEL_H: f32 = 22.0;

/// Section label ("PROJECTS"): IBM Plex Mono Medium 10 px, UPPERCASE, `text_3`, with 0.1 em of
/// spacing, 22 px of height and 10 px of margin. `rect` is the row (only `min` and the width count);
/// returns the row's rectangle.
pub fn section_label(painter: &Painter, rect: Rect, text: &str) -> Rect {
    let row = Rect::from_min_size(rect.min, Vec2::new(rect.width(), SECTION_LABEL_H));
    let color = theme::pal().text_3;
    let g = galley_spaced(painter, &text.to_uppercase(), theme::label(10.0), color, 10.0 * theme::LABEL_SPACING_EM);
    place(painter, Rect::from_min_max(Pos2::new(row.min.x + 10.0, row.min.y), row.max), Align2::LEFT_CENTER, g, color);
    row
}

/// Segmented control ("Tabs | Split | Highlight"): `recess` track (7 px corners), segments with
/// 5 px corners. The active one takes the accent color with `on_accent` text (`accent = true`) or the
/// `selected` background with `text_strong` text (`accent = false`). Returns the clicked index, if any.
pub fn segmented(ui: &Ui, rect: Rect, id: Id, items: &[&str], selected: usize, accent: bool) -> Option<usize> {
    let painter = ui.painter();
    let p = theme::pal();
    painter.rect_filled(rect, 7.0, p.recess);
    let inner = rect.shrink(2.0);
    let w = inner.width() / items.len().max(1) as f32;
    let mut clicked = None;
    for (i, label) in items.iter().enumerate() {
        let seg = Rect::from_min_size(Pos2::new(inner.min.x + w * i as f32, inner.min.y), Vec2::new(w, inner.height()));
        let r = clickable(ui, seg, id.with(i));
        let on = i == selected;
        let (fill, fg, font) = if on && accent {
            (Some(theme::accent()), p.on_accent, theme::ui_semibold(11.5))
        } else if on {
            (Some(p.selected), p.text_strong, theme::ui_semibold(11.5))
        } else {
            (r.hovered().then_some(p.hover), p.text_2, theme::ui(11.5))
        };
        if let Some(fill) = fill {
            painter.rect_filled(seg, 5.0, fill);
        }
        text_in(painter, seg, Align2::CENTER_CENTER, label, font, fg);
        if r.clicked() {
            clicked = Some(i);
        }
    }
    clicked
}

/// The look of a text button.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ButtonKind {
    /// Accent color, `on_accent` text, 600.
    Primary,
    /// White border 14%, white background 4%.
    Secondary,
    /// Only the text (`text_2`); the `hover` background shows up on hover.
    Ghost,
}

/// Text button (5 px corners). Returns the response; the caller handles the click.
pub fn button(ui: &Ui, rect: Rect, id: Id, label: &str, kind: ButtonKind) -> Response {
    let r = clickable(ui, rect, id);
    let painter = ui.painter();
    let p = theme::pal();
    let hot = r.hovered();
    match kind {
        ButtonKind::Primary => {
            let fill = if hot { theme::brighten(theme::accent(), 1.1) } else { theme::accent() };
            painter.rect_filled(rect, 5.0, fill);
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), p.on_accent);
        }
        ButtonKind::Secondary => {
            let fill = theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 });
            frame(painter, rect, 5.0, fill, theme::with_alpha(p.text_strong, 0.14));
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui(11.5), p.text);
        }
        ButtonKind::Ghost => {
            if hot {
                painter.rect_filled(rect, 5.0, p.hover);
            }
            text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui(11.5), if hot { p.text } else { p.text_2 });
        }
    }
    r
}

/// Destructive button (`danger` background, `on_accent` text, 5 px corners).
pub fn button_danger(ui: &Ui, rect: Rect, id: Id, label: &str) -> Response {
    let r = clickable(ui, rect, id);
    let painter = ui.painter();
    let p = theme::pal();
    let fill = if r.hovered() { theme::brighten(p.danger, 1.1) } else { p.danger };
    painter.rect_filled(rect, 5.0, fill);
    text_in(painter, rect, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), p.on_accent);
    r
}

/// Width of a text button with `label` (10 px on each side).
pub fn button_width(painter: &Painter, label: &str, kind: ButtonKind) -> f32 {
    let font = if kind == ButtonKind::Primary { theme::ui_semibold(11.5) } else { theme::ui(11.5) };
    text_w(painter, label, font) + 20.0
}

/// How a row of pill tabs fits in `avail` px: the inner space of each tab (summed on both sides) and the
/// maximum width of each label. `texts` are the label widths; `fixed`, what each tab has besides the label and the
/// inner space (the badge); `gap`, the space between the tabs. First the inner space shrinks from `pad_max` down to `pad_min`;
/// if it still does not fit, the longest labels are shortened (with an ellipsis) by the same amount, except the one of the `keep` tab.
pub fn fit_pills(texts: &[f32], fixed: &[f32], avail: f32, pad: (f32, f32), gap: f32, keep: Option<usize>) -> (f32, Vec<f32>) {
    let n = texts.len() as f32;
    let (pad_min, pad_max) = pad;
    if texts.is_empty() {
        return (pad_max, Vec::new());
    }
    let base = fixed.iter().sum::<f32>() + gap * (n - 1.0);
    let text_sum: f32 = texts.iter().sum();
    let pad = ((avail - base - text_sum) / n).clamp(pad_min, pad_max);
    let room = avail - base - pad * n;
    if text_sum <= room {
        return (pad, texts.to_vec());
    }
    // Common cap of the shortenable labels: the largest `c` with sum(min(t, c)) <= what is left for them.
    let kept = keep.map_or(0.0, |k| texts[k]);
    let mut others: Vec<f32> = texts.iter().enumerate().filter(|(i, _)| Some(*i) != keep).map(|(_, &t)| t).collect();
    others.sort_by(f32::total_cmp);
    let mut left = (room - kept).max(0.0);
    let mut cap = 0.0;
    for (i, &t) in others.iter().enumerate() {
        let share = left / (others.len() - i) as f32;
        if t <= share {
            left -= t;
            cap = t;
        } else {
            cap = share;
            break;
        }
    }
    let widths = texts.iter().enumerate().map(|(i, &t)| if Some(i) == keep { t } else { t.min(cap) }).collect();
    (pad, widths)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_pills_shrink_the_padding_and_then_the_longest_labels() {
        // It fits with room to spare: maximum inner space and whole labels.
        assert_eq!(fit_pills(&[40.0, 50.0], &[0.0, 0.0], 300.0, (6.0, 16.0), 2.0, None), (16.0, vec![40.0, 50.0]));
        // Tight: the inner space shrinks first.
        let (pad, w) = fit_pills(&[40.0, 50.0], &[0.0, 0.0], 112.0, (6.0, 16.0), 2.0, None);
        assert_eq!((pad, w), (10.0, vec![40.0, 50.0]));
        // It does not fit even at the minimum: shortens the longest one, never the active tab's.
        let (pad, w) = fit_pills(&[40.0, 50.0, 60.0], &[10.0, 0.0, 0.0], 150.0, (6.0, 16.0), 2.0, Some(2));
        assert_eq!(pad, 6.0);
        assert_eq!(w[2], 60.0);
        let total = w.iter().sum::<f32>() + 10.0 + 3.0 * 6.0 + 2.0 * 2.0;
        assert!((total - 150.0).abs() < 0.01, "{total}");
        assert!(w[0] <= 40.0 && w[1] <= 40.0 && w[0] == w[1]);
        assert_eq!(fit_pills(&[], &[], 100.0, (6.0, 16.0), 2.0, None), (16.0, vec![]));
    }

    #[test]
    fn the_right_panel_keeps_room_for_the_terminal() {
        // Wide window: the saved width counts (respecting the minimum and the maximum).
        assert_eq!(right_width(380.0, 1280.0, 221.0), (380.0, 640.0));
        assert_eq!(right_width(100.0, 1280.0, 221.0).0, theme::RIGHT_MIN_W);
        assert_eq!(right_width(900.0, 1280.0, 221.0).0, 640.0);
        // Narrow window: the terminal keeps at least TERM_MIN_W, and the bar at least 260.
        let (w, max) = right_width(380.0, 820.0, 221.0);
        assert_eq!((w, max), (260.0, 260.0));
        assert!(820.0 - 221.0 - w >= 300.0);
        let (w, _) = right_width(380.0, 1000.0, 0.0);
        assert_eq!(w, 380.0);
    }

    fn rect(x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(Pos2::new(x0, y0), Pos2::new(x1, y1))
    }

    fn area_of(rects: &[Rect]) -> f32 {
        rects.iter().map(|r| r.width() * r.height()).sum()
    }

    #[test]
    fn the_free_space_is_the_area_minus_the_islands() {
        let area = rect(0.0, 0.0, 100.0, 50.0);
        let islands = [rect(6.0, 0.0, 40.0, 44.0), rect(46.0, 0.0, 94.0, 44.0)];
        let free = free_rects(area, &islands);
        assert_eq!(area_of(&free), 100.0 * 50.0 - 34.0 * 44.0 - 48.0 * 44.0);
        // Nothing is left inside an island.
        for f in &free {
            assert!(islands.iter().all(|i| i.intersect(*f).width() <= 0.0 || i.intersect(*f).height() <= 0.0));
        }
        assert_eq!(free_rects(area, &[]), vec![area]);
        assert!(free_rects(area, &[area]).is_empty());
    }

    #[test]
    fn the_splitter_wire_runs_along_the_gap() {
        let vertical = splitter_wire(rect(100.0, 0.0, 106.0, 400.0));
        assert_eq!((vertical.width(), vertical.height()), (2.0, 400.0));
        assert_eq!(vertical.center(), Pos2::new(103.0, 200.0));
        let horizontal = splitter_wire(rect(0.0, 50.0, 300.0, 56.0));
        assert_eq!((horizontal.width(), horizontal.height()), (300.0, 2.0));
    }
}

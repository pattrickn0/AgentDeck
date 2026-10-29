//! `#sidebar`: the Agents | Documents switch, the projects and agents, the window organization and the
//! VS Code-style file explorer, plus the resizable divider.

use std::collections::HashMap;

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, galley_trunc, paint_splitter, place, section_label, segmented, separator, text_in, tip};
use crate::deck::{Deck, TreeRow};
use crate::icons::Icon;
use crate::theme;
use holt_core::git::{EntryKind, GitEntry};
use holt_core::t;
use holt_core::Organization;

/// Height of an explorer row.
const FILE_ROW_H: f32 = 20.0;
/// Side margin of the rows (the selection does not touch the island edge) and indent per folder level.
const SIDE_MARGIN: f32 = 4.0;
const FILE_INDENT: f32 = 12.0;
/// Height of the ORGANIZATION switch track (20 px segments and 2 px on each side).
const ORG_H: f32 = 24.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    let ctx = ui.ctx().clone();

    // Keeps the workspace root logically expanded so that `deck.tree_rows()` lists the files.
    deck.tree.expanded.insert(String::new());

    let dragging = ctx.dragged_id().is_some() || deck.pane_drag.is_some() || deck.drop_hover;

    // ---------- Agents | Documents switch, with the island width (6 px margin) ----------
    let switch = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, rect.min.y + 6.0), Vec2::new(rect.width() - 12.0, super::docs::SWITCH_H));
    super::docs::switch(deck, ui, switch, "side-center");
    let mut top_y = switch.max.y + 6.0;
    separator(&painter, Pos2::new(rect.min.x, top_y + 0.5), Pos2::new(rect.max.x, top_y + 0.5));
    top_y += 1.0;

    // ---------- PROJECTS section ----------
    super::projects::draw(deck, ui, &mut top_y, rect, dragging);

    // ---------- ORGANIZATION section (how the windows are arranged) ----------
    let org_head = Rect::from_min_size(Pos2::new(rect.min.x, top_y + 4.0), Vec2::new(rect.width(), super::SECTION_LABEL_H));
    section_label(&painter, org_head, t!("ORGANIZATION"));
    let org = Rect::from_min_size(Pos2::new(rect.min.x + 10.0, org_head.max.y), Vec2::new(rect.width() - 20.0, ORG_H));
    let modes = [Organization::Tabs, Organization::Split, Organization::Highlight];
    let mode = modes.iter().position(|m| *m == deck.organization).unwrap_or(0);
    let segments = [t!("Tabs"), t!("Split"), t!("Highlight")];
    if let Some(picked) = segmented(ui, org, Id::new("org-mode"), &segments, mode, false) {
        deck.set_organization(modes[picked]);
    }
    top_y = org.max.y + 8.0;
    separator(&painter, Pos2::new(rect.min.x, top_y + 0.5), Pos2::new(rect.max.x, top_y + 0.5));
    top_y += 1.0;

    // ---------- FILES section with quick actions ----------
    let head_rect = Rect::from_min_size(Pos2::new(rect.min.x, top_y + 4.0), Vec2::new(rect.width(), super::SECTION_LABEL_H));
    top_y = head_rect.max.y;
    section_label(&painter, head_rect, t!("FILES"));

    // Refresh button (Icon::Refresh)
    let refresh_r = Rect::from_min_size(Pos2::new(head_rect.max.x - 24.0, head_rect.min.y + 1.0), Vec2::new(20.0, 20.0));
    let refresh_resp = clickable(ui, refresh_r, Id::new("tree-refresh-btn"));
    let r_hov = refresh_resp.hovered() && !dragging;
    if r_hov {
        painter.rect_filled(refresh_r, 4.0, theme::pal().hover);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let r_color = if r_hov { theme::pal().text } else { theme::pal().text_2 };
    deck.icons.paint(&painter, Icon::Refresh, refresh_r.center(), 13.0, r_color, 0.0);
    if tip(refresh_resp, t!("Refresh explorer")).clicked() {
        deck.refresh_tree();
        deck.tree.expanded.insert(String::new());
    }

    // Collapse all button (VS Code icon drawn on the painter)
    let collapse_r = Rect::from_min_size(Pos2::new(refresh_r.min.x - 22.0, head_rect.min.y + 1.0), Vec2::new(20.0, 20.0));
    let collapse_resp = clickable(ui, collapse_r, Id::new("tree-collapse-btn"));
    let c_hov = collapse_resp.hovered() && !dragging;
    if c_hov {
        painter.rect_filled(collapse_r, 4.0, theme::pal().hover);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let c_color = if c_hov { theme::pal().text } else { theme::pal().text_2 };
    paint_collapse_all(&painter, collapse_r.center(), c_color);
    if tip(collapse_resp, t!("Collapse all folders")).clicked() {
        deck.tree.expanded.retain(|p| p.is_empty());
        deck.tree.expanded.insert(String::new());
    }
    let list_top = top_y + 2.0;

    // ---------- File list of the tree ----------
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, list_top), rect.max);
    let rows = deck.tree_rows();
    let marks = git_marks(deck);
    let mut ui_list = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    ui_list.set_clip_rect(list_rect);
    let mut clicked_dir: Option<String> = None;
    let mut clicked_file: Option<String> = None;
    let mut dbl_file: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("file-tree").auto_shrink([false, false]).show_rows(
        &mut ui_list,
        FILE_ROW_H,
        rows.len(),
        |ui, range| {
            let width = ui.available_width();
            for i in range {
                let (row_rect, resp) = ui.allocate_exact_size(Vec2::new(width, FILE_ROW_H), Sense::click());
                let row = &rows[i];
                paint_row(deck, ui, row_rect, row, &resp, &marks);
                let resp = tip(resp, row.entry.rel_path.clone());
                if row.entry.is_dir {
                    if resp.clicked() {
                        clicked_dir = Some(row.entry.rel_path.clone());
                    }
                } else if resp.double_clicked() {
                    dbl_file = Some(row.entry.rel_path.clone());
                } else if resp.clicked() {
                    clicked_file = Some(row.entry.rel_path.clone());
                }
            }
        },
    );
    if let Some(d) = clicked_dir {
        deck.toggle_dir(&d);
    }
    if let Some(f) = dbl_file {
        deck.tree_file_double_click(&f);
    } else if let Some(f) = clicked_file {
        deck.tree_file_click(&f);
    }
}

/// The Git letter of a changed file (`M`, `A`, `D`, `R`, `U`, `?`).
fn git_mark(e: &GitEntry) -> char {
    match e.kind {
        EntryKind::Untracked => '?',
        EntryKind::Unmerged => 'U',
        EntryKind::Renamed | EntryKind::Copied => 'R',
        EntryKind::Ordinary => [e.index, e.worktree].into_iter().find(|c| matches!(c, 'A' | 'D')).unwrap_or('M'),
    }
}

/// The Git letter of every changed file of the project, by the path relative to the workspace (the `status`
/// comes with paths relative to the repository root, which may be a folder above the workspace).
fn git_marks(deck: &Deck) -> HashMap<String, char> {
    let Some(st) = deck.git.status() else { return HashMap::new() };
    st.entries
        .iter()
        .filter_map(|e| e.path.strip_prefix(st.prefix.as_str()).map(|rel| (rel.trim_end_matches('/').to_owned(), git_mark(e))))
        .collect()
}

/// Draws the VS Code "Collapse all folders" icon on the painter: a square with "−"
/// and, behind it, the edge of another square (above and to the right).
fn paint_collapse_all(painter: &egui::Painter, center: Pos2, color: Color32) {
    let stroke = Stroke::new(1.0, color);
    let front = Rect::from_center_size(center + Vec2::new(-1.0, 1.0), Vec2::splat(9.0));
    painter.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Inside);
    painter.line_segment([Pos2::new(front.min.x + 2.5, front.center().y), Pos2::new(front.max.x - 2.5, front.center().y)], stroke);
    let (x0, y0, x1, y1) = (front.min.x + 2.5, front.min.y - 2.5, front.max.x + 2.5, front.max.y - 2.5);
    painter.line_segment([Pos2::new(x0, front.min.y), Pos2::new(x0, y0)], stroke);
    painter.line_segment([Pos2::new(x0, y0), Pos2::new(x1, y0)], stroke);
    painter.line_segment([Pos2::new(x1, y0), Pos2::new(x1, y1)], stroke);
    painter.line_segment([Pos2::new(x1, y1), Pos2::new(front.max.x, y1)], stroke);
}

fn paint_row(deck: &mut Deck, ui: &Ui, rect: Rect, row: &TreeRow, resp: &egui::Response, marks: &HashMap<String, char>) {
    let painter = ui.painter();
    let ctx = ui.ctx();
    let p = theme::pal();
    let dragging = ctx.dragged_id().is_some() || deck.pane_drag.is_some() || deck.drop_hover;
    let hovered = resp.hovered() && !dragging;
    if hovered {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let is_active = !row.entry.is_dir && deck.tree.active_file.as_deref() == Some(row.entry.rel_path.as_str());
    let mark = marks.get(row.entry.rel_path.as_str()).copied();
    let (bg, fg) = if is_active {
        (Some(p.selected_soft), p.text_strong)
    } else if hovered {
        (Some(p.hover), p.text_strong)
    } else if mark.is_some() {
        (None, p.text)
    } else {
        (None, p.text_2)
    };
    let inner = rect.shrink2(Vec2::new(SIDE_MARGIN, 0.0));
    if let Some(bg) = bg {
        painter.rect_filled(inner, 4.0, bg);
    }
    // The first level of the root has depth 1 in TreeRow: it becomes indent 0.
    let depth = row.depth.saturating_sub(1);
    let x0 = inner.min.x + 8.0 + depth as f32 * FILE_INDENT;
    let cy = rect.center().y;
    // 14 px box: the folder arrow (it turns when opening) or the file type sign, always in gray.
    let box_c = Pos2::new(x0 + 7.0, cy);
    if row.entry.is_dir {
        let t = ctx.animate_bool_with_time(Id::new(("tree-chev", &row.entry.rel_path)), row.expanded, 0.12);
        deck.icons.paint(painter, Icon::TreeChevron, box_c, 10.0, p.text_3, t * std::f32::consts::FRAC_PI_2);
    } else {
        file_icon(deck, painter, Rect::from_center_size(box_c, Vec2::splat(14.0)), &row.entry.name);
    }
    // The Git letter on the right (IBM Plex Mono 10 px).
    let right = inner.max.x - 8.0;
    let mut name_right = right;
    if let Some(m) = mark {
        let g = super::galley(painter, &m.to_string(), theme::meta(10.0), p.text_2);
        let w = g.size().x;
        place(
            painter,
            Rect::from_min_max(Pos2::new(right - w, rect.min.y), Pos2::new(right, rect.max.y)),
            Align2::LEFT_CENTER,
            g,
            p.text_2,
        );
        name_right = right - w - 6.0;
    }
    // Name with ellipsis.
    let name_x = x0 + 14.0 + 5.0;
    let g = galley_trunc(painter, &row.entry.name, theme::ui(11.5), fg, (name_right - name_x).max(1.0));
    place(painter, Rect::from_min_max(Pos2::new(name_x, rect.min.y), Pos2::new(name_right, rect.max.y)), Align2::LEFT_CENTER, g, fg);
}

/// The sign of the file type in the 14 px box, in gray (the user asked for less color).
fn file_icon(deck: &mut Deck, painter: &egui::Painter, rect: Rect, name: &str) {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let ext = if name.contains('.') { ext } else { String::new() };
    match badge_text(&ext) {
        Some(text) => {
            text_in(painter, rect, Align2::CENTER_CENTER, text, theme::mono_bold(8.5), theme::pal().text_3);
        }
        None => deck.icons.paint(painter, Icon::FileGeneric, rect.center(), 12.0, theme::pal().text_3, 0.0),
    }
}

/// The text of the sign of an extension (`None`: generic file icon).
fn badge_text(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "js" | "mjs" | "cjs" => "JS",
        "json" => "{ }",
        "md" | "markdown" => "M↓",
        "ts" | "tsx" => "TS",
        "html" | "htm" => "<>",
        "css" => "#",
        "py" => "Py",
        "rs" => "rs",
        "toml" => "⚙",
        _ => return None,
    })
}

/// `#sidebar-splitter`: dragging resizes between 150 and 480 px.
pub fn splitter(deck: &mut Deck, ui: &mut Ui, line: Rect, full: Rect) {
    let hit = line;
    let r = ui.interact(hit, Id::new("sidebar-splitter"), Sense::drag());
    if r.hovered() || r.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if r.drag_started() {
        if let Some(p) = r.interact_pointer_pos() {
            deck.sidebar_dragging = Some((p.x, deck.sidebar_w));
        }
    }
    if r.dragged() {
        if let (Some((x0, w0)), Some(p)) = (deck.sidebar_dragging, ui.input(|i| i.pointer.latest_pos())) {
            deck.sidebar_w = (w0 + p.x - x0).clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W);
        }
    }
    if r.drag_stopped() {
        deck.sidebar_dragging = None;
        deck.persist_layout();
    }
    let hot = r.hovered() || deck.sidebar_dragging.is_some();
    let painter = ui.painter_at(full);
    paint_splitter(&painter, line, hot, deck.accent());
    tip(r, t!("Drag to resize the sidebar"));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(kind: EntryKind, index: char, worktree: char) -> GitEntry {
        GitEntry { path: "x".into(), orig_path: None, index, worktree, kind }
    }

    #[test]
    fn the_git_letter_is_the_most_telling_one() {
        assert_eq!(git_mark(&entry(EntryKind::Untracked, '?', '?')), '?');
        assert_eq!(git_mark(&entry(EntryKind::Unmerged, 'U', 'U')), 'U');
        assert_eq!(git_mark(&entry(EntryKind::Renamed, 'R', '.')), 'R');
        assert_eq!(git_mark(&entry(EntryKind::Ordinary, 'A', '.')), 'A');
        assert_eq!(git_mark(&entry(EntryKind::Ordinary, '.', 'D')), 'D');
        assert_eq!(git_mark(&entry(EntryKind::Ordinary, '.', 'M')), 'M');
        // Added and then edited still stays "A".
        assert_eq!(git_mark(&entry(EntryKind::Ordinary, 'A', 'M')), 'A');
    }

    #[test]
    fn the_badge_text_covers_the_usual_extensions() {
        assert_eq!(badge_text("rs"), Some("rs"));
        assert_eq!(badge_text("md"), Some("M↓"));
        assert_eq!(badge_text("tsx"), Some("TS"));
        assert_eq!(badge_text("zzz"), None);
        assert_eq!(badge_text(""), None);
    }
}

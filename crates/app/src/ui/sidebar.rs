//! `#sidebar`: explorador de arquivos estilo VS Code + divisor redimensionável.

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Ui, Vec2};

use super::{galley_trunc, paint_splitter, place, text_in, tip};
use crate::deck::{Deck, TreeRow};
use crate::icons::Icon;
use crate::theme;
use crate::util;

const ROW_H: f32 = 22.0;
const ROOT_H: f32 = 26.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);
    let ctx = ui.ctx().clone();

    let mut top_y = rect.min.y;

    // ---------- Seção PROJETOS ----------
    let proj_head = Rect::from_min_size(Pos2::new(rect.min.x, top_y), Vec2::new(rect.width(), ROOT_H));
    top_y = proj_head.max.y;
    let proj_expanded = deck.projects_expanded;
    let r_head = ui.interact(proj_head, Id::new("proj-head"), Sense::click());
    if r_head.hovered() {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    painter.rect_filled(proj_head, 0.0, if r_head.hovered() { theme::PANEL_2 } else { theme::BG });
    painter.rect_filled(Rect::from_min_max(Pos2::new(proj_head.min.x, proj_head.max.y - 1.0), proj_head.max), 0.0, theme::BORDER);

    let chev_center = Pos2::new(proj_head.min.x + 6.0 + 7.0, proj_head.center().y - 0.5);
    let t = ctx.animate_bool_with_time(Id::new("proj-chev"), proj_expanded, 0.12);
    deck.icons.paint(&painter, Icon::TreeChevron, chev_center, 10.0, theme::TREE_ICON, t * std::f32::consts::FRAC_PI_2);

    let title_x = proj_head.min.x + 6.0 + 14.0 + 5.0;
    let title_g = galley_trunc(&painter, "PROJETOS", theme::ui_semibold(11.0), theme::MUTED, proj_head.width() - 50.0);
    place(&painter, Rect::from_min_max(Pos2::new(title_x, proj_head.min.y), Pos2::new(proj_head.max.x - 30.0, proj_head.max.y - 1.0)), Align2::LEFT_CENTER, title_g, theme::MUTED);

    // Botão "+" para adicionar projeto
    let add_r = Rect::from_min_size(Pos2::new(proj_head.max.x - 24.0, proj_head.min.y + 3.0), Vec2::new(20.0, 20.0));
    let add_resp = super::clickable(ui, add_r, Id::new("proj-add-btn"));
    if add_resp.hovered() {
        painter.rect_filled(add_r, 4.0, theme::BORDER);
    }
    text_in(&painter, add_r, Align2::CENTER_CENTER, "+", theme::ui(14.0), if add_resp.hovered() { theme::TEXT } else { theme::MUTED });
    let mut add_clicked = false;
    if tip(add_resp, "Adicionar projeto…").clicked() {
        add_clicked = true;
    }

    if r_head.clicked() && !add_clicked {
        deck.projects_expanded = !deck.projects_expanded;
    }

    let mut switch_to = None;
    let mut remove_idx = None;
    if deck.projects_expanded {
        let projects = deck.projects.clone();
        for (i, p) in projects.iter().enumerate() {
            let row = Rect::from_min_size(Pos2::new(rect.min.x, top_y), Vec2::new(rect.width(), ROW_H));
            top_y += ROW_H;
            let is_active = *p == deck.workspace;
            let row_resp = ui.interact(row, Id::new(("proj-row", i)), Sense::click());
            let hovered = row_resp.hovered();
            if is_active || hovered {
                painter.rect_filled(row, 0.0, if is_active { theme::PANEL_2 } else { theme::BG });
            }
            if hovered {
                ctx.set_cursor_icon(CursorIcon::PointingHand);
            }
            // Indicador de projeto ativo
            let dot_color = if is_active { deck.active_color() } else { theme::MUTED };
            super::dot(&painter, Pos2::new(row.min.x + 16.0, row.center().y), dot_color);

            let pname = util::folder_name(&p.to_string_lossy());
            let font = if is_active { theme::ui_semibold(12.5) } else { theme::ui(12.5) };
            let color = if is_active { theme::TEXT } else { theme::MUTED };
            let g = galley_trunc(&painter, &pname, font, color, row.width() - 48.0);
            place(&painter, Rect::from_min_max(Pos2::new(row.min.x + 28.0, row.min.y), Pos2::new(row.max.x - 24.0, row.max.y)), Align2::LEFT_CENTER, g, color);

            // Botão "×" para remover da lista (se mais de 1 projeto)
            if projects.len() > 1 && hovered {
                let del_r = Rect::from_min_size(Pos2::new(row.max.x - 20.0, row.min.y + 2.0), Vec2::new(16.0, 18.0));
                let del_resp = ui.interact(del_r, Id::new(("proj-del", i)), Sense::click());
                if del_resp.hovered() {
                    painter.rect_filled(del_r, 3.0, theme::BORDER);
                }
                text_in(&painter, del_r, Align2::CENTER_CENTER, "×", theme::ui(12.0), theme::MUTED);
                if tip(del_resp, "Remover projeto da lista").clicked() {
                    remove_idx = Some(i);
                }
            }

            if tip(row_resp, format!("{}\nClique para ativar este projeto", p.display())).clicked() {
                switch_to = Some(p.clone());
            }
        }
        painter.rect_filled(Rect::from_min_max(Pos2::new(rect.min.x, top_y - 1.0), Pos2::new(rect.max.x, top_y)), 0.0, theme::BORDER);
    }

    if add_clicked {
        deck.pick_project();
    }
    if let Some(p) = switch_to {
        deck.switch_workspace(p);
    } else if let Some(i) = remove_idx {
        deck.remove_project(i);
    }

    // ---------- Linha raiz do workspace ativo (26px, negrito, borda inferior) ----------
    let root = Rect::from_min_size(Pos2::new(rect.min.x, top_y), Vec2::new(rect.width(), ROOT_H));
    let root_expanded = deck.tree.expanded.contains("");
    let r = ui.interact(root, Id::new("tree-root"), Sense::click());
    if r.hovered() {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    painter.rect_filled(root, 0.0, if r.hovered() { theme::PANEL_2 } else { theme::BG });
    painter.rect_filled(Rect::from_min_max(Pos2::new(root.min.x, root.max.y - 1.0), root.max), 0.0, theme::BORDER);
    let chev_center = Pos2::new(root.min.x + 6.0 + 7.0, root.center().y - 0.5);
    let t = ctx.animate_bool_with_time(Id::new("tree-root-chev"), root_expanded, 0.12);
    deck.icons.paint(&painter, Icon::TreeChevron, chev_center, 10.0, theme::TREE_ICON, t * std::f32::consts::FRAC_PI_2);
    let name = util::folder_name(&deck.workspace.to_string_lossy());
    let name_x = root.min.x + 6.0 + 14.0 + 5.0;
    let g = galley_trunc(&painter, &name, theme::ui_semibold(13.0), theme::TEXT, root.max.x - 8.0 - name_x);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(name_x, root.min.y), Pos2::new(root.max.x - 8.0, root.max.y - 1.0)),
        Align2::LEFT_CENTER,
        g,
        theme::TEXT,
    );
    if r.clicked() {
        deck.toggle_dir("");
    }

    // ---------- Itens ----------
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, root.max.y), rect.max);
    let rows = deck.tree_rows();
    let mut ui_list = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    ui_list.set_clip_rect(list_rect);
    let mut clicked_dir: Option<String> = None;
    let mut clicked_file: Option<String> = None;
    let mut dbl_file: Option<String> = None;
    egui::ScrollArea::vertical().id_salt("file-tree").auto_shrink([false, false]).show_rows(
        &mut ui_list,
        ROW_H,
        rows.len(),
        |ui, range| {
            let width = ui.available_width();
            for i in range {
                let (row_rect, resp) = ui.allocate_exact_size(Vec2::new(width, ROW_H), Sense::click());
                let row = &rows[i];
                paint_row(deck, ui, row_rect, row, &resp);
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

fn paint_row(deck: &mut Deck, ui: &Ui, rect: Rect, row: &TreeRow, resp: &egui::Response) {
    let painter = ui.painter();
    let ctx = ui.ctx();
    let hovered = resp.hovered();
    if hovered {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let is_active = !row.entry.is_dir && deck.tree.active_file.as_deref() == Some(row.entry.rel_path.as_str());
    let (bg, fg) = if is_active {
        (Some(theme::BORDER), theme::WHITE)
    } else if hovered {
        (Some(theme::PANEL_2), theme::WHITE)
    } else {
        (None, theme::TREE_TEXT)
    };
    if let Some(bg) = bg {
        painter.rect_filled(rect, 0.0, bg);
    }
    let x0 = rect.min.x + 6.0 + row.depth as f32 * 12.0;
    let cy = rect.center().y;
    // Chevron (só pastas), caixa de 14px.
    if row.entry.is_dir {
        let t = ctx.animate_bool_with_time(Id::new(("tree-chev", &row.entry.rel_path)), row.expanded, 0.12);
        deck.icons.paint(painter, Icon::TreeChevron, Pos2::new(x0 + 7.0, cy), 10.0, theme::TREE_ICON, t * std::f32::consts::FRAC_PI_2);
    }
    // Ícone (16px) após gap de 5px.
    let icon_rect = Rect::from_center_size(Pos2::new(x0 + 14.0 + 5.0 + 8.0, cy), Vec2::splat(16.0));
    if row.entry.is_dir {
        let icon = if row.expanded { Icon::FolderOpen } else { Icon::FolderClosed };
        deck.icons.paint(painter, icon, icon_rect.center(), 14.0, theme::TREE_ICON, 0.0);
    } else {
        file_icon(deck, painter, icon_rect, &row.entry.name);
    }
    // Nome com reticências.
    let name_x = icon_rect.max.x + 5.0;
    let max_w = rect.max.x - 8.0 - name_x;
    let g = galley_trunc(painter, &row.entry.name, theme::ui(13.0), fg, max_w);
    place(painter, Rect::from_min_max(Pos2::new(name_x, rect.min.y), Pos2::new(rect.max.x - 8.0, rect.max.y)), Align2::LEFT_CENTER, g, fg);
}

/// Badges de extensão (`getFileIconHtml`).
fn file_icon(deck: &mut Deck, painter: &egui::Painter, rect: Rect, name: &str) {
    let ext = name.rsplit('.').next().unwrap_or("").to_lowercase();
    let ext = if name.contains('.') { ext } else { String::new() };
    let badge: Option<(&str, Color32, f32)> = match ext.as_str() {
        "js" | "mjs" | "cjs" => Some(("JS", theme::BADGE_JS, 9.5)),
        "json" => Some(("{ }", theme::BADGE_JSON, 9.5)),
        "md" | "markdown" => Some(("M↓", theme::BADGE_MD, 9.0)),
        "ts" => Some(("TS", theme::BADGE_TS, 9.0)),
        "html" | "htm" => Some(("<>", theme::BADGE_HTML, 9.0)),
        "css" => Some(("#", theme::BADGE_CSS, 9.0)),
        "py" => Some(("Py", theme::BADGE_PY, 9.0)),
        _ => None,
    };
    match badge {
        Some((text, color, size)) => {
            // font-weight: 800 → fonte mono em negrito (com reforço de meio pixel).
            let font = theme::mono_bold(size);
            text_in(painter, rect, Align2::CENTER_CENTER, text, font.clone(), color);
            text_in(painter, rect.translate(Vec2::new(0.35, 0.0)), Align2::CENTER_CENTER, text, font, color);
        }
        None => deck.icons.paint(painter, Icon::FileGeneric, rect.center(), 13.0, theme::TREE_ICON, 0.0),
    }
}

/// `#sidebar-splitter`: arrastar redimensiona entre 150 e 480 px.
pub fn splitter(deck: &mut Deck, ui: &mut Ui, line: Rect, full: Rect) {
    let hit = line.expand2(Vec2::new(3.0, 0.0));
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
    paint_splitter(&painter, line, hot, deck.active_color());
    tip(r, "Arraste para redimensionar barra lateral");
}

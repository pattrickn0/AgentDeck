//! `#sidebar`: explorador de arquivos estilo VS Code + divisor redimensionável.

use egui::{Align2, Color32, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, dot, frame, galley_trunc, menus, paint_splitter, place, shadow, text_in, tip};
use crate::deck::{Deck, TreeRow};
use crate::icons::Icon;
use crate::theme;
use crate::util;

const ROW_H: f32 = 22.0;
/// Altura do título de cada seção (ORGANIZAÇÃO, EXPLORADOR).
const SECTION_H: f32 = 24.0;

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 0.0, theme::BG);
    let ctx = ui.ctx().clone();

    // Mantém a raiz do workspace logicamente expandida para que `deck.tree_rows()` liste os arquivos.
    deck.tree.expanded.insert(String::new());

    let dragging = ctx.dragged_id().is_some() || deck.pane_drag.is_some() || deck.drop_hover;
    let menu_id = Id::new("ws-dropdown-open");
    let mut menu_open = if deck.sidebar_collapsed {
        ctx.data_mut(|d| d.insert_temp(menu_id, false));
        false
    } else {
        ctx.data_mut(|d| d.get_temp::<bool>(menu_id).unwrap_or(false))
    };

    let mut top_y = rect.min.y;

    // ---------- Seletor de Workspace no topo (38 px = theme::TOPBAR_H) ----------
    let sel_h = theme::TOPBAR_H;
    let sel_rect = Rect::from_min_size(Pos2::new(rect.min.x, top_y), Vec2::new(rect.width(), sel_h));
    top_y = sel_rect.max.y;

    let r_sel = ui.interact(sel_rect, Id::new("ws-selector"), Sense::click());
    let sel_hovered = r_sel.hovered() && !dragging;
    if sel_hovered {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }

    // Fundo em PANEL (repouso) ou PANEL_2 (hover / dropdown aberto)
    painter.rect_filled(sel_rect, 0.0, if menu_open || sel_hovered { theme::PANEL_2 } else { theme::PANEL });
    painter.rect_filled(Rect::from_min_max(Pos2::new(sel_rect.min.x, sel_rect.max.y - 1.0), sel_rect.max), 0.0, theme::BORDER);

    // Ícone de pasta à esquerda (14 px)
    let folder_icon = if menu_open { Icon::FolderOpen } else { Icon::FolderClosed };
    let folder_color = if menu_open || sel_hovered { theme::TEXT } else { theme::TREE_ICON };
    let folder_center = Pos2::new(sel_rect.min.x + 14.0, sel_rect.center().y);
    deck.icons.paint(&painter, folder_icon, folder_center, 14.0, folder_color, 0.0);

    // Chevron à direita girado (0 fechado, π/2 aberto)
    let chev_center = Pos2::new(sel_rect.max.x - 14.0, sel_rect.center().y);
    let t_chev = ctx.animate_bool_with_time(Id::new("ws-chev"), menu_open, 0.12);
    let chev_color = if menu_open || sel_hovered { theme::TEXT } else { theme::TREE_ICON };
    deck.icons.paint(&painter, Icon::TreeChevron, chev_center, 9.0, chev_color, t_chev * std::f32::consts::FRAC_PI_2);

    // Textos em duas linhas: Nome da pasta em ui_semibold(13) e caminho em ui(10.5)
    let text_x = sel_rect.min.x + 28.0;
    let text_max_x = sel_rect.max.x - 24.0;
    let max_text_w = (text_max_x - text_x).max(1.0);

    let ws_name = util::folder_name(&deck.workspace.to_string_lossy());
    let name_g = galley_trunc(&painter, &ws_name, theme::ui_semibold(13.0), theme::TEXT, max_text_w);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(text_x, sel_rect.min.y + 3.0), Pos2::new(text_max_x, sel_rect.min.y + 19.0)),
        Align2::LEFT_CENTER,
        name_g,
        theme::TEXT,
    );

    let ws_path = deck.workspace.to_string_lossy();
    let path_g = galley_trunc(&painter, &ws_path, theme::ui(10.5), theme::MUTED, max_text_w);
    place(
        &painter,
        Rect::from_min_max(Pos2::new(text_x, sel_rect.min.y + 20.0), Pos2::new(text_max_x, sel_rect.min.y + 34.0)),
        Align2::LEFT_CENTER,
        path_g,
        theme::MUTED,
    );

    let r_sel = tip(r_sel, format!("{}\nClique para trocar de projeto", deck.workspace.display()));
    if r_sel.clicked() {
        menu_open = !menu_open;
        ctx.data_mut(|d| d.insert_temp(menu_id, menu_open));
    }

    // ---------- Menu suspenso de projetos (Dropdown) ----------
    let mut switch_to = None;
    let mut remove_idx = None;
    let mut pick_folder = false;

    if menu_open {
        let projects = deck.projects.clone();
        let item_h = 26.0;
        let sep_h = 9.0;
        let h = 6.0 + (projects.len() as f32 * item_h) + sep_h + item_h + 6.0;
        let w = rect.width().max(200.0);
        let screen = ctx.content_rect();
        let base_pos = Pos2::new(sel_rect.min.x, sel_rect.max.y);
        let pos = menus::keep_on_screen(&ctx, base_pos, Vec2::new(w, h));
        let pos_y = if pos.y + h > screen.max.y - 8.0 { (screen.max.y - h - 8.0).max(screen.min.y + 4.0) } else { pos.y };
        let menu_rect = Rect::from_min_size(Pos2::new(pos.x, pos_y), Vec2::new(w, h));

        egui::Area::new(Id::new("ws-dropdown-area")).order(egui::Order::Foreground).fixed_pos(menu_rect.min).constrain(false).show(
            &ctx,
            |ui| {
                let (panel_rect, _) = ui.allocate_exact_size(menu_rect.size(), Sense::hover());
                let p = ui.painter();
                shadow(p, panel_rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.60));
                frame(p, panel_rect, 6.0, theme::PANEL_2, theme::BORDER);

                let mut y = panel_rect.min.y + 5.0;

                // Lista de projetos
                for (i, proj) in projects.iter().enumerate() {
                    let item_r = Rect::from_min_size(Pos2::new(panel_rect.min.x + 5.0, y), Vec2::new(w - 10.0, item_h));
                    y += item_h;

                    let is_active = *proj == deck.workspace;
                    let item_resp = ui.interact(item_r, Id::new(("ws-item", i)), Sense::click());
                    let item_hovered = ui.rect_contains_pointer(item_r) && !dragging;

                    if item_hovered {
                        p.rect_filled(item_r, 4.0, theme::BORDER);
                        ctx.set_cursor_icon(CursorIcon::PointingHand);
                    }

                    // Indicador de projeto ativo
                    if is_active {
                        dot(p, Pos2::new(item_r.min.x + 12.0, item_r.center().y), deck.active_color());
                    }

                    let pname = util::folder_name(&proj.to_string_lossy());
                    let font = if is_active { theme::ui_semibold(12.5) } else { theme::ui(12.5) };
                    let color = if is_active { theme::TEXT } else { theme::MUTED };

                    let max_name_w = (item_r.width() - 44.0).max(1.0);
                    let g = galley_trunc(p, &pname, font, color, max_name_w);
                    place(
                        p,
                        Rect::from_min_max(Pos2::new(item_r.min.x + 22.0, item_r.min.y), Pos2::new(item_r.max.x - 22.0, item_r.max.y)),
                        Align2::LEFT_CENTER,
                        g,
                        color,
                    );

                    // Botão "×" para remover da lista (se mais de 1 projeto)
                    if projects.len() > 1 && item_hovered {
                        let del_r = Rect::from_min_size(Pos2::new(item_r.max.x - 20.0, item_r.min.y + 3.0), Vec2::new(16.0, 20.0));
                        let del_resp = ui.interact(del_r, Id::new(("ws-del", i)), Sense::click());
                        if del_resp.hovered() {
                            p.rect_filled(del_r, 3.0, theme::BORDER);
                        }
                        text_in(
                            p,
                            del_r,
                            Align2::CENTER_CENTER,
                            "×",
                            theme::ui(13.0),
                            if del_resp.hovered() { theme::RED_TEXT } else { theme::MUTED },
                        );
                        if tip(del_resp, "Remover projeto da lista").clicked() {
                            remove_idx = Some(i);
                        }
                    }

                    if tip(item_resp, format!("{}\nClique para ativar este projeto", proj.display())).clicked() && remove_idx.is_none() {
                        switch_to = Some(proj.clone());
                    }
                }

                // Separador
                let sep_y = y + 4.0;
                p.rect_filled(
                    Rect::from_min_max(Pos2::new(panel_rect.min.x + 8.0, sep_y), Pos2::new(panel_rect.max.x - 8.0, sep_y + 1.0)),
                    0.0,
                    theme::BORDER,
                );
                y += sep_h;

                // Ação "Abrir pasta…"
                let open_r = Rect::from_min_size(Pos2::new(panel_rect.min.x + 5.0, y), Vec2::new(w - 10.0, item_h));
                let open_resp = ui.interact(open_r, Id::new("ws-open-folder"), Sense::click());
                let open_hovered = ui.rect_contains_pointer(open_r) && !dragging;
                if open_hovered {
                    p.rect_filled(open_r, 4.0, theme::BORDER);
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }
                deck.icons.paint(
                    p,
                    Icon::FolderOpen,
                    Pos2::new(open_r.min.x + 12.0, open_r.center().y),
                    13.0,
                    if open_hovered { theme::TEXT } else { theme::TREE_ICON },
                    0.0,
                );
                let open_g = galley_trunc(
                    p,
                    "Abrir pasta…",
                    theme::ui(12.5),
                    if open_hovered { theme::TEXT } else { theme::MUTED },
                    open_r.width() - 32.0,
                );
                place(
                    p,
                    Rect::from_min_max(Pos2::new(open_r.min.x + 24.0, open_r.min.y), open_r.max),
                    Align2::LEFT_CENTER,
                    open_g,
                    theme::TEXT,
                );
                if tip(open_resp, "Abrir outra pasta como projeto…").clicked() {
                    pick_folder = true;
                }
            },
        );

        // Fechar com ESC, clique fora ou escolha
        if let Some(i) = remove_idx {
            deck.remove_project(i);
        } else if let Some(p) = switch_to {
            deck.switch_workspace(p);
            ctx.data_mut(|d| d.insert_temp(menu_id, false));
        } else if pick_folder {
            deck.pick_workspace();
            ctx.data_mut(|d| d.insert_temp(menu_id, false));
        } else if ctx.input(|i| i.key_pressed(egui::Key::Escape)) || menus::closes_on_outside(&ctx, menu_rect, sel_rect) {
            ctx.data_mut(|d| d.insert_temp(menu_id, false));
        }
    }

    // ---------- Seção ORGANIZAÇÃO (como as janelas se arrumam) ----------
    let org_head = Rect::from_min_size(Pos2::new(rect.min.x, top_y + 6.0), Vec2::new(rect.width(), SECTION_H));
    section_title(&painter, org_head, "ORGANIZAÇÃO");
    let split_row = Rect::from_min_size(Pos2::new(rect.min.x, org_head.max.y), Vec2::new(rect.width(), ROW_H + 4.0));
    let split_resp = ui.interact(split_row, Id::new("org-split"), Sense::click());
    let split_hov = split_resp.hovered() && !dragging;
    if split_hov {
        painter.rect_filled(split_row, 0.0, theme::PANEL_2);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let on = deck.split_enabled;
    let label_color = if split_hov { theme::WHITE } else { theme::TREE_TEXT };
    deck.icons.paint(
        &painter,
        Icon::Split,
        Pos2::new(split_row.min.x + 19.0, split_row.center().y),
        14.0,
        if on { deck.active_color() } else { theme::TREE_ICON },
        0.0,
    );
    let switch_r = Rect::from_center_size(Pos2::new(split_row.max.x - 12.0 - 13.0, split_row.center().y), Vec2::new(26.0, 14.0));
    let label_g =
        galley_trunc(&painter, "Tela dividida", theme::ui(13.0), label_color, (switch_r.min.x - 8.0 - (split_row.min.x + 32.0)).max(1.0));
    place(
        &painter,
        Rect::from_min_max(Pos2::new(split_row.min.x + 32.0, split_row.min.y), Pos2::new(switch_r.min.x - 8.0, split_row.max.y)),
        Align2::LEFT_CENTER,
        label_g,
        label_color,
    );
    let t = ctx.animate_bool_with_time(Id::new("org-split-anim"), on, 0.12);
    paint_switch(&painter, switch_r, t, deck.active_color());
    let tip_text = if on {
        "Tela dividida: ligada
Cada tipo de agente numa janela · clique para ver uma aba por vez"
    } else {
        "Tela dividida: desligada
Clique para abrir uma janela por tipo de agente"
    };
    if tip(split_resp, tip_text).clicked() {
        deck.toggle_split();
    }
    top_y = split_row.max.y + 6.0;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.min.x + 8.0, top_y), Pos2::new(rect.max.x - 8.0, top_y + 1.0)),
        0.0,
        theme::BORDER,
    );

    // ---------- Seção EXPLORADOR com ações rápidas ----------
    let head_rect = Rect::from_min_size(Pos2::new(rect.min.x, top_y + 6.0), Vec2::new(rect.width(), SECTION_H));
    top_y = head_rect.max.y;
    section_title(&painter, head_rect, "EXPLORADOR");

    // Botão Atualizar (Icon::Refresh)
    let refresh_r = Rect::from_min_size(Pos2::new(head_rect.max.x - 24.0, head_rect.min.y + 2.0), Vec2::new(20.0, 20.0));
    let refresh_resp = clickable(ui, refresh_r, Id::new("tree-refresh-btn"));
    let r_hov = refresh_resp.hovered() && !dragging;
    if r_hov {
        painter.rect_filled(refresh_r, 4.0, theme::PANEL_2);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let r_color = if r_hov { theme::TEXT } else { theme::MUTED };
    deck.icons.paint(&painter, Icon::Refresh, refresh_r.center(), 13.0, r_color, 0.0);
    if tip(refresh_resp, "Atualizar explorador").clicked() {
        deck.refresh_tree();
        deck.tree.expanded.insert(String::new());
    }

    // Botão Recolher tudo (ícone VS Code no painter)
    let collapse_r = Rect::from_min_size(Pos2::new(refresh_r.min.x - 22.0, head_rect.min.y + 2.0), Vec2::new(20.0, 20.0));
    let collapse_resp = clickable(ui, collapse_r, Id::new("tree-collapse-btn"));
    let c_hov = collapse_resp.hovered() && !dragging;
    if c_hov {
        painter.rect_filled(collapse_r, 4.0, theme::PANEL_2);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let c_color = if c_hov { theme::TEXT } else { theme::MUTED };
    paint_collapse_all(&painter, collapse_r.center(), c_color);
    if tip(collapse_resp, "Recolher todas as pastas").clicked() {
        deck.tree.expanded.retain(|p| p.is_empty());
        deck.tree.expanded.insert(String::new());
    }

    // Separador sutil com margens laterais
    let sep_y = top_y + 4.0;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.min.x + 8.0, sep_y), Pos2::new(rect.max.x - 8.0, sep_y + 1.0)),
        0.0,
        theme::BORDER,
    );
    let list_top = sep_y + 4.0;

    // ---------- Lista de arquivos da árvore ----------
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, list_top), rect.max);
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

/// Título de seção da barra lateral (MAIÚSCULAS, 11 px, MUTED).
fn section_title(painter: &egui::Painter, head: Rect, title: &str) {
    let g = galley_trunc(painter, title, theme::ui_semibold(11.0), theme::MUTED, head.width() - 60.0);
    place(
        painter,
        Rect::from_min_max(Pos2::new(head.min.x + 12.0, head.min.y), Pos2::new(head.max.x - 52.0, head.max.y)),
        Align2::LEFT_CENTER,
        g,
        theme::MUTED,
    );
}

/// Chave liga/desliga (`t` de 0 a 1, animado): trilho na cor do agente ativo quando ligada.
fn paint_switch(painter: &egui::Painter, r: Rect, t: f32, on_color: Color32) {
    let track = theme::BORDER.lerp_to_gamma(on_color, t);
    painter.rect_filled(r, r.height() / 2.0, track);
    let knob_r = r.height() / 2.0 - 2.0;
    let x = egui::lerp((r.min.x + 2.0 + knob_r)..=(r.max.x - 2.0 - knob_r), t);
    painter.circle_filled(Pos2::new(x, r.center().y), knob_r, theme::MUTED.lerp_to_gamma(theme::WHITE, t));
}

/// Desenha o ícone de "Recolher todas as pastas" do VS Code no painter: um quadrado com "−"
/// e, atrás dele, a borda de outro quadrado (acima e à direita).
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

fn paint_row(deck: &mut Deck, ui: &Ui, rect: Rect, row: &TreeRow, resp: &egui::Response) {
    let painter = ui.painter();
    let ctx = ui.ctx();
    let dragging = ctx.dragged_id().is_some() || deck.pane_drag.is_some() || deck.drop_hover;
    let hovered = resp.hovered() && !dragging;
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
    // Deslocamento horizontal: o primeiro nível da raiz tem depth 1 em TreeRow,
    // que convertemos para depth 0 para poupar 12 px na barra lateral.
    let depth = row.depth.saturating_sub(1);
    let x0 = rect.min.x + 6.0 + depth as f32 * 12.0;
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
    let max_w = (rect.max.x - 8.0 - name_x).max(1.0);
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

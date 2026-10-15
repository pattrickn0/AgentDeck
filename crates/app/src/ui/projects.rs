//! Seção PROJETOS da barra lateral: lista de projetos, abas por projeto e ações rápidas.

use std::path::{Path, PathBuf};

use egui::{Align2, Color32, CursorIcon, Id, Order, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::{clickable, dot, frame, galley, galley_trunc, menus, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::theme;

pub const ROW_H: f32 = 22.0;
pub const SECTION_H: f32 = 24.0;
const LOGO_SIZE: f32 = 12.0;
const LOGO_GAP: f32 = 4.0;

/// Mapeia o identificador do modelo para o ícone correspondente.
fn agent_icon(kind: &str) -> Option<Icon> {
    let model = kind.split(':').next().unwrap_or(kind);
    match model {
        "claude" => Some(Icon::ClaudeLogo),
        "antigravity" => Some(Icon::AntigravityLogo),
        "ollama" => Some(Icon::OllamaLogo),
        "codex" => Some(Icon::CodexLogo),
        _ => None,
    }
}

/// Abre uma pasta no explorador de arquivos do sistema operacional.
fn open_project_folder(deck: &Deck, path: &Path) {
    if deck.workspace == path && deck.core.open_path(".").is_ok() {
        return;
    }
    #[cfg(windows)]
    {
        let _ = std::process::Command::new("explorer").arg(path).spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open").arg(path).spawn();
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        let _ = std::process::Command::new("xdg-open").arg(path).spawn();
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

/// Desenha a seção PROJETOS e avança `top_y`.
pub fn draw(deck: &mut Deck, ui: &mut Ui, top_y: &mut f32, rect: Rect, dragging: bool) {
    let ctx = ui.ctx().clone();

    if deck.sidebar_collapsed {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>);
            d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>);
        });
        return;
    }

    let painter = ui.painter_at(rect);

    // ---------- Cabeçalho da seção PROJETOS ----------
    let head_rect = Rect::from_min_size(Pos2::new(rect.min.x, *top_y + 4.0), Vec2::new(rect.width(), SECTION_H));
    *top_y = head_rect.max.y;
    section_title(&painter, head_rect, "PROJETOS");

    // Botão "+" para novo projeto
    let plus_r = Rect::from_min_size(Pos2::new(head_rect.max.x - 24.0, head_rect.min.y + 2.0), Vec2::new(20.0, 20.0));
    let plus_resp = clickable(ui, plus_r, Id::new("proj-plus-btn"));
    let plus_hov = plus_resp.hovered() && !dragging;
    if plus_hov {
        painter.rect_filled(plus_r, 4.0, theme::PANEL_2);
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let plus_color = if plus_hov { theme::TEXT } else { theme::MUTED };
    text_in(&painter, plus_r, Align2::CENTER_CENTER, "+", theme::ui_semibold(14.0), plus_color);
    if tip(plus_resp, "Novo projeto…").clicked() {
        deck.new_project_dialog();
    }

    // ---------- Lista de projetos e abas ativas ----------
    let projects = deck.project_views();
    // Só a altura do conteúdo, até 40 % da barra (aí a lista rola).
    let rows = projects.len() + projects.iter().filter(|p| p.active).map(|p| p.agents.len()).sum::<usize>();
    let content_h = rows as f32 * (ROW_H + ui.spacing().item_spacing.y);
    let max_list_h = content_h.min((rect.height() * 0.40).round());
    let list_top = *top_y + 2.0;
    let list_rect = Rect::from_min_max(Pos2::new(rect.min.x, list_top), Pos2::new(rect.max.x, list_top + max_list_h));

    let mut ui_list = ui.new_child(egui::UiBuilder::new().max_rect(list_rect).layout(egui::Layout::top_down(egui::Align::Min)));
    ui_list.set_clip_rect(list_rect);

    let mut switch_to_id = None;
    let mut activate_tab = None;
    let mut open_ctx = None;
    let active_color = deck.active_color();
    let deck_active = deck.active;

    let scroll_output = egui::ScrollArea::vertical().id_salt("proj-list-scroll").max_height(max_list_h).auto_shrink([false, true]).show(
        &mut ui_list,
        |ui| {
            let row_w = ui.available_width();
            let p = ui.painter().clone();

            for (p_idx, proj) in projects.iter().enumerate() {
                let (row_r, resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
                let hovered = resp.hovered() && !dragging;

                // Fundo: ativo = PANEL_2, hover = PANEL_2
                if proj.active {
                    p.rect_filled(row_r, 0.0, theme::PANEL_2);
                } else if hovered {
                    p.rect_filled(row_r, 4.0, theme::PANEL_2);
                }

                if hovered {
                    ctx.set_cursor_icon(CursorIcon::PointingHand);
                }

                // Indicador de projeto ativo: borda esquerda + ponto
                if proj.active {
                    let ind_bar = Rect::from_min_max(Pos2::new(row_r.min.x, row_r.min.y), Pos2::new(row_r.min.x + 2.5, row_r.max.y));
                    p.rect_filled(ind_bar, 0.0, active_color);
                    dot(&p, Pos2::new(row_r.min.x + 10.0, row_r.center().y), active_color);
                }

                // Lado direito: contagem e logos dos agentes
                let count = proj.agents.len();
                let count_font = theme::ui(11.0);
                let count_str = if count > 0 { count.to_string() } else { String::new() };
                let count_w = if count > 0 { text_w(&p, &count_str, count_font.clone()) } else { 0.0 };
                let count_right_x = row_r.max.x - 10.0;

                if count > 0 {
                    text_in(
                        &p,
                        Rect::from_min_max(Pos2::new(count_right_x - count_w, row_r.min.y), Pos2::new(count_right_x, row_r.max.y)),
                        Align2::RIGHT_CENTER,
                        &count_str,
                        count_font,
                        theme::MUTED,
                    );
                }

                // Logos dos modelos para projetos não ativos
                let mut unique_models: Vec<(&str, Color32)> = Vec::new();
                if !proj.active {
                    for ag in &proj.agents {
                        let model = ag.kind.split(':').next().unwrap_or(&ag.kind);
                        if !unique_models.iter().any(|(m, _)| *m == model) {
                            unique_models.push((model, ag.color));
                        }
                    }
                }

                let logos_total_w = if !unique_models.is_empty() {
                    unique_models.len() as f32 * LOGO_SIZE + (unique_models.len().saturating_sub(1) as f32) * LOGO_GAP
                } else {
                    0.0
                };

                let logos_end_x = if count > 0 { count_right_x - count_w - 6.0 } else { count_right_x };
                let logos_start_x = logos_end_x - logos_total_w;

                if !unique_models.is_empty() {
                    let mut cur_logo_x = logos_start_x;
                    for (model, color) in &unique_models {
                        let center = Pos2::new(cur_logo_x + LOGO_SIZE / 2.0, row_r.center().y);
                        if let Some(icon) = agent_icon(model) {
                            deck.icons.paint(&p, icon, center, LOGO_SIZE, *color, 0.0);
                        } else {
                            let ch = model.chars().next().unwrap_or('?').to_uppercase().to_string();
                            p.text(center, Align2::CENTER_CENTER, ch, theme::ui_semibold(9.5), *color);
                        }
                        cur_logo_x += LOGO_SIZE + LOGO_GAP;
                    }
                }

                // Nome do projeto (com truncamento para não sobrepor logos/contagem)
                let name_x = row_r.min.x + 20.0;
                let name_right_limit = if !unique_models.is_empty() {
                    logos_start_x - 6.0
                } else if count > 0 {
                    count_right_x - count_w - 6.0
                } else {
                    row_r.max.x - 10.0
                };
                let max_name_w = (name_right_limit - name_x).max(1.0);
                let name_font = if proj.active { theme::ui_semibold(12.5) } else { theme::ui(12.5) };
                let name_color = if proj.active || hovered { theme::TEXT } else { theme::TREE_TEXT };

                let name_g = galley_trunc(&p, &proj.name, name_font, name_color, max_name_w);
                place(
                    &p,
                    Rect::from_min_max(Pos2::new(name_x, row_r.min.y), Pos2::new(name_right_limit, row_r.max.y)),
                    Align2::LEFT_CENTER,
                    name_g,
                    name_color,
                );

                // Tooltip com caminho completo
                let resp = tip(resp, proj.path.display().to_string());

                // Interações de clique
                if resp.secondary_clicked() {
                    let p_pos = ui.input(|i| i.pointer.interact_pos()).unwrap_or(row_r.center());
                    open_ctx = Some((proj.id.clone(), proj.path.clone(), proj.name.clone(), p_pos));
                } else if resp.clicked() && !proj.active {
                    switch_to_id = Some(proj.id.clone());
                }

                // Se o projeto for o ativo, listar os agentes dele embaixo
                if proj.active {
                    for (ag_idx, ag) in proj.agents.iter().enumerate() {
                        let (tab_r, tab_resp) = ui.allocate_exact_size(Vec2::new(row_w, ROW_H), Sense::click());
                        let tab_hov = tab_resp.hovered() && !dragging;
                        let is_active_tab = ag.tab.is_some() && deck_active == ag.tab;

                        if is_active_tab {
                            p.rect_filled(tab_r, 4.0, theme::BORDER);
                        } else if tab_hov {
                            p.rect_filled(tab_r, 4.0, theme::PANEL_2);
                        }

                        if tab_hov {
                            ctx.set_cursor_icon(CursorIcon::PointingHand);
                        }

                        let dot_pos = Pos2::new(tab_r.min.x + 28.0, tab_r.center().y);
                        if ag.busy {
                            ctx.request_repaint_after(std::time::Duration::from_millis(33));
                            let t = ctx.input(|inp| inp.time);
                            let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
                            let halo_r = 4.0 + 2.8 * wave;
                            let halo_alpha = 0.35 * (1.0 - wave);
                            p.circle_filled(dot_pos, halo_r, theme::with_opacity(ag.color, halo_alpha));
                            p.circle_filled(dot_pos, 3.2 + 1.0 * wave, theme::with_opacity(ag.color, 0.75 + 0.25 * wave));
                        } else if ag.done && !is_active_tab {
                            dot(&p, dot_pos, ag.color);
                            p.circle_stroke(dot_pos, 6.2, Stroke::new(1.2, theme::brighten(ag.color, 1.25)));
                            p.circle_filled(dot_pos, 1.2, theme::with_opacity(Color32::WHITE, 0.75));
                        } else if !ag.alive {
                            dot(&p, dot_pos, theme::with_opacity(ag.color, 0.4));
                        } else {
                            dot(&p, dot_pos, ag.color);
                        }

                        let tab_text_x = tab_r.min.x + 38.0;
                        let max_tab_w = (tab_r.max.x - 10.0 - tab_text_x).max(1.0);
                        let tab_font = if ag.done || is_active_tab { theme::ui_semibold(12.0) } else { theme::ui(12.0) };
                        let tab_color = if !ag.alive {
                            theme::with_opacity(theme::MUTED, 0.6)
                        } else if is_active_tab || ag.done {
                            theme::WHITE
                        } else if tab_hov {
                            theme::TEXT
                        } else {
                            theme::TREE_TEXT
                        };

                        let tab_g = galley_trunc(&p, &ag.label, tab_font, tab_color, max_tab_w);
                        let placed_r = place(
                            &p,
                            Rect::from_min_max(Pos2::new(tab_text_x, tab_r.min.y), Pos2::new(tab_r.max.x - 10.0, tab_r.max.y)),
                            Align2::LEFT_CENTER,
                            tab_g,
                            tab_color,
                        );

                        if !ag.alive {
                            p.hline(placed_r.min.x..=placed_r.max.x, placed_r.center().y + 0.5, Stroke::new(1.0, tab_color));
                        }

                        let tooltip = if ag.busy {
                            format!("{} (trabalhando…)", ag.label)
                        } else if ag.done {
                            format!("{} (concluído)", ag.label)
                        } else if !ag.alive {
                            format!("{} (encerrado)", ag.label)
                        } else {
                            ag.label.clone()
                        };
                        let tab_resp = tip(tab_resp, tooltip);

                        let _ = (p_idx, ag_idx);
                        if tab_resp.clicked() {
                            if let Some(tab_idx) = ag.tab {
                                activate_tab = Some(tab_idx);
                            }
                        }
                    }
                }
            }
        },
    );

    *top_y = list_top + scroll_output.inner_rect.height().min(max_list_h);

    // Separador com margem de 8 px
    *top_y += 6.0;
    painter.rect_filled(
        Rect::from_min_max(Pos2::new(rect.min.x + 8.0, *top_y), Pos2::new(rect.max.x - 8.0, *top_y + 1.0)),
        0.0,
        theme::BORDER,
    );
    *top_y += 1.0;

    // Ações executadas fora das closures
    if let Some(id) = switch_to_id {
        deck.switch_project(&id);
    }
    if let Some(tab_idx) = activate_tab {
        deck.activate(tab_idx);
    }
    if let Some((id, path, name, pos)) = open_ctx {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("proj-ctx-menu"), Some((id, path, name, pos)));
        });
    }

    // Menus e Modais
    show_context_menu(deck, &ctx);
    show_rename_modal(deck, &ctx);
}

/// Menu de contexto do projeto (clique direito).
fn show_context_menu(deck: &mut Deck, ctx: &egui::Context) {
    let ctx_data: Option<(String, PathBuf, String, Pos2)> = ctx.data(|d| d.get_temp(Id::new("proj-ctx-menu")).flatten());
    let Some((id, path, name, pos)) = ctx_data else { return };

    let can_remove = true;
    let w = 170.0;
    let item_h = 26.0;
    let h = item_h * 3.0 + 8.0;
    let pos = menus::keep_on_screen(ctx, pos, Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));

    let mut action_rename = false;
    let mut action_open = false;
    let mut action_remove = false;

    egui::Area::new(Id::new("proj-ctx-menu-area")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(menu_rect.size(), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, theme::rgba(0, 0, 0, 0.55));
        frame(p, rect, 6.0, theme::PANEL_2, theme::BORDER);
        let font = theme::ui(12.5);

        let mut item_y = rect.min.y + 4.0;

        // Renomear
        let r_ren = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        item_y += item_h;
        let resp_ren = clickable(ui, r_ren, Id::new("proj-ctx-ren"));
        if resp_ren.hovered() {
            p.rect_filled(r_ren, 4.0, theme::BORDER);
        }
        text_in(p, r_ren.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Renomear", font.clone(), theme::TEXT);
        if resp_ren.clicked() {
            action_rename = true;
        }

        // Abrir pasta no sistema
        let r_open = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        item_y += item_h;
        let resp_open = clickable(ui, r_open, Id::new("proj-ctx-open"));
        if resp_open.hovered() {
            p.rect_filled(r_open, 4.0, theme::BORDER);
        }
        text_in(p, r_open.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Abrir pasta no sistema", font.clone(), theme::TEXT);
        if resp_open.clicked() {
            action_open = true;
        }

        // Remover
        let r_rem = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, item_y), Vec2::new(w - 8.0, item_h));
        let resp_rem = if can_remove {
            clickable(ui, r_rem, Id::new("proj-ctx-rem"))
        } else {
            ui.interact(r_rem, Id::new("proj-ctx-rem"), Sense::hover())
        };
        if resp_rem.hovered() && can_remove {
            p.rect_filled(r_rem, 4.0, theme::BORDER);
        }
        let rem_color = if !can_remove {
            theme::with_opacity(theme::MUTED, 0.5)
        } else if resp_rem.hovered() {
            theme::RED_TEXT
        } else {
            theme::TEXT
        };
        text_in(p, r_rem.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Remover", font, rem_color);
        if can_remove && resp_rem.clicked() {
            action_remove = true;
        }
    });

    if action_rename {
        ctx.data_mut(|d| {
            d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>);
            d.insert_temp(Id::new("proj-rename-modal"), Some((id, name)));
        });
    } else if action_open {
        open_project_folder(deck, &path);
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
    } else if action_remove {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
        let ok = rfd::MessageDialog::new()
            .set_title("Remover projeto")
            .set_description(format!("Deseja remover \"{name}\" da lista de projetos?"))
            .set_buttons(rfd::MessageButtons::YesNo)
            .show()
            == rfd::MessageDialogResult::Yes;
        if ok {
            deck.remove_project_by_id(&id);
        }
    } else if ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu_rect.contains(p)))
        || ctx.input(|i| i.key_pressed(egui::Key::Escape))
    {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-ctx-menu"), None::<(String, PathBuf, String, Pos2)>));
    }
}

/// Modal para renomear projeto (modal de texto simples).
fn show_rename_modal(deck: &mut Deck, ctx: &egui::Context) {
    let modal_data: Option<(String, String)> = ctx.data(|d| d.get_temp(Id::new("proj-rename-modal")).flatten());
    let Some((id, mut buffer)) = modal_data else { return };

    let screen = ctx.content_rect();
    // Backdrop escuro
    ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("proj-rename-backdrop"))).rect_filled(
        screen,
        0.0,
        theme::rgba(0, 0, 0, 0.5),
    );

    let w = 360.0;
    let h = 145.0;
    let center = screen.center();
    let rect = Rect::from_center_size(center, Vec2::new(w, h));
    let mut close = false;
    let mut apply = false;

    egui::Area::new(Id::new("proj-rename-modal-area")).order(Order::Tooltip).fixed_pos(rect.min).show(ctx, |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let p = ui.painter().clone();
        shadow(&p, r, 0, 8, 30, theme::rgba(0, 0, 0, 0.7));
        frame(&p, r, 10.0, theme::PANEL_2, theme::BORDER);

        let mut y = r.min.y + 16.0;
        // Título
        let title_g = galley(&p, "Renomear Projeto", theme::ui_semibold(15.0), theme::TEXT);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 20.0)), Align2::LEFT_CENTER, title_g, theme::TEXT);
        y += 24.0;

        // Explicação
        let hint_g = galley(&p, "Altera o nome do projeto exibido na barra lateral", theme::ui(11.5), theme::MUTED);
        place(&p, Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 16.0)), Align2::LEFT_CENTER, hint_g, theme::MUTED);
        y += 22.0;

        // Caixa de texto
        let input_r = Rect::from_min_size(Pos2::new(r.min.x + 20.0, y), Vec2::new(w - 40.0, 28.0));
        p.rect_filled(input_r, 6.0, theme::BG);
        p.rect_stroke(input_r, 6.0, Stroke::new(1.0, theme::BORDER), egui::StrokeKind::Inside);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input_r.shrink2(Vec2::new(8.0, 4.0))));
        let input_id = Id::new("proj-rename-input");
        let te = egui::TextEdit::singleline(&mut buffer)
            .id(input_id)
            .frame(egui::Frame::NONE)
            .font(theme::ui(13.0))
            .text_color(theme::TEXT)
            .desired_width(input_r.width() - 16.0);
        let out = te.show(&mut child);
        out.response.request_focus();
        if (out.response.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))) || ui.input(|i| i.key_pressed(egui::Key::Enter)) {
            apply = true;
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            close = true;
        }
        y += 34.0;

        // Botões Cancelar e Salvar
        let btn_w = 80.0;
        let btn_h = 26.0;
        let save_r = Rect::from_min_size(Pos2::new(r.max.x - 20.0 - btn_w, y), Vec2::new(btn_w, btn_h));
        let cancel_r = Rect::from_min_size(Pos2::new(save_r.min.x - 10.0 - btn_w, y), Vec2::new(btn_w, btn_h));

        let cr = clickable(ui, cancel_r, Id::new("proj-rename-cancel-btn"));
        if cr.hovered() {
            p.rect_filled(cancel_r, 6.0, theme::BORDER);
        } else {
            p.rect_filled(cancel_r, 6.0, theme::PANEL);
        }
        text_in(&p, cancel_r, Align2::CENTER_CENTER, "Cancelar", theme::ui(12.5), theme::MUTED);
        if cr.clicked() {
            close = true;
        }

        let sr = clickable(ui, save_r, Id::new("proj-rename-save-btn"));
        if sr.hovered() {
            p.rect_filled(save_r, 6.0, theme::brighten(theme::ACCENT, 1.15));
        } else {
            p.rect_filled(save_r, 6.0, theme::ACCENT);
        }
        text_in(&p, save_r, Align2::CENTER_CENTER, "Salvar", theme::ui_semibold(12.5), theme::WHITE);
        if sr.clicked() {
            apply = true;
        }
    });

    if close {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>));
    } else if apply {
        let trimmed = buffer.trim();
        if !trimmed.is_empty() {
            deck.rename_project(&id, trimmed);
        }
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), None::<(String, String)>));
    } else {
        ctx.data_mut(|d| d.insert_temp(Id::new("proj-rename-modal"), Some((id, buffer))));
    }
}

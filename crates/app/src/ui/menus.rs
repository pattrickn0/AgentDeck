//! Menus suspensos: nova aba (+), configurações (engrenagem) e lista do "Enviar para".

use egui::{Align2, Area, Id, Order, Pos2, Rect, Sense, Stroke, Vec2};

use super::{dot, frame, galley_trunc, place, shadow, text_in, text_w};
use crate::deck::Deck;
use crate::theme;

/// Fecha o menu se houve clique fora dele e da âncora (`document.addEventListener('click')`).
fn closes_on_outside(ctx: &egui::Context, menu: Rect, anchor: Rect) -> bool {
    ctx.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !menu.contains(p) && !anchor.contains(p)))
}

/// `#new-tab-menu`
pub fn new_tab_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.new_tab_menu_open {
        return;
    }
    let items = deck.agents.new_tab_menu.clone();
    let items = if items.is_empty() {
        // Antes da primeira listagem: só o terminal (fallback do renderer).
        vec![crate::agents::MenuItem { kind: "shell".into(), label: crate::agents::shell_label(None), color: theme::SHELL }]
    } else {
        items
    };
    let font = theme::ui(13.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let content_w = items.iter().map(|m| 8.0 + 8.0 + text_w(&painter, &m.label, font.clone()) + 20.0).fold(0.0, f32::max);
    let w = (content_w + 8.0 + 2.0).max(180.0);
    let item_h = 13.0 * theme::LINE_HEIGHT + 16.0;
    let h = items.len() as f32 * item_h + 8.0 + 2.0;
    let pos = Pos2::new(anchor.min.x, anchor.max.y);
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    Area::new(Id::new("new-tab-menu")).order(Order::Foreground).fixed_pos(pos).constrain(true).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, theme::rgba(0, 0, 0, 0x88 as f32 / 255.0));
        frame(p, rect, 8.0, theme::PANEL_2, theme::BORDER);
        let mut y = rect.min.y + 5.0;
        for m in &items {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 5.0, y), Vec2::new(w - 10.0, item_h));
            y += item_h;
            let resp = super::clickable(ui, r, Id::new(("new-tab-item", &m.kind)));
            if resp.hovered() {
                p.rect_filled(r, 6.0, theme::BORDER);
            }
            dot(p, Pos2::new(r.min.x + 10.0 + 4.0, r.center().y), m.color);
            text_in(p, Rect::from_min_max(Pos2::new(r.min.x + 10.0 + 16.0, r.min.y), r.max), Align2::LEFT_CENTER, &m.label, font.clone(), theme::TEXT);
            if resp.clicked() {
                chosen = Some(m.kind.clone());
            }
        }
    });
    if let Some(kind) = chosen {
        deck.new_tab_menu_open = false;
        deck.open_tab(&kind, false);
    } else if closes_on_outside(ctx, menu_rect, anchor) {
        deck.new_tab_menu_open = false;
    }
}

/// `#settings-menu`
pub fn settings_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.settings_open {
        return;
    }
    let w = 220.0;
    let title_h = 11.0 * theme::LINE_HEIGHT + 4.0;
    let item_h = 13.0 * theme::LINE_HEIGHT + 12.0;
    let btn_h = 12.0 * theme::LINE_HEIGHT + 12.0 + 2.0;
    let h = 8.0 + title_h + 8.0 + item_h + 2.0 + item_h + 6.0 + btn_h + 8.0 + 2.0;
    // right: 4px a partir da borda direita do botão (que fica colado à janela).
    let pos = Pos2::new(anchor.max.x - 4.0 - w, anchor.max.y);
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut toggle = false;
    let mut toggle_tabs = false;
    let mut test = false;
    let sound = deck.sound_enabled;
    let auto_tabs = deck.auto_open_agent_tabs;
    Area::new(Id::new("settings-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        let p = ui.painter();
        shadow(p, rect, 8, 8, 24, theme::rgba(0, 0, 0, 0xaa as f32 / 255.0));
        frame(p, rect, 8.0, theme::PANEL_2, theme::BORDER);
        let inner = rect.shrink(9.0);
        // Título: 11px, 700, maiúsculas, letter-spacing .5px
        let title = Rect::from_min_size(Pos2::new(inner.min.x + 4.0, inner.min.y), Vec2::new(inner.width() - 8.0, title_h));
        let mut job = egui::text::LayoutJob::default();
        for ch in "CONFIGURAÇÕES".chars() {
            job.append(&ch.to_string(), 0.0, egui::TextFormat { font_id: theme::ui_bold(11.0), color: theme::MUTED, extra_letter_spacing: 0.5, ..Default::default() });
        }
        let g = p.layout_job(job);
        place(p, title, Align2::LEFT_CENTER, g, theme::MUTED);

        // Item com checkbox
        let item = Rect::from_min_size(Pos2::new(inner.min.x, title.max.y + 8.0), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item, Id::new("setting-sound"));
        if r.hovered() {
            p.rect_filled(item, 6.0, theme::BORDER);
        }
        text_in(p, item.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Sons de notificação", theme::ui(13.0), theme::TEXT);
        let cb = Rect::from_center_size(Pos2::new(item.max.x - 8.0 - 8.0, item.center().y), Vec2::splat(16.0));
        checkbox(p, cb, sound);
        if r.clicked() {
            toggle = true;
        }

        // Item com checkbox: abas dos agentes chamados por `ask-agent`
        let item2 = Rect::from_min_size(Pos2::new(inner.min.x, item.max.y + 2.0), Vec2::new(inner.width(), item_h));
        let r = super::clickable(ui, item2, Id::new("setting-auto-tabs"));
        if r.hovered() {
            p.rect_filled(item2, 6.0, theme::BORDER);
        }
        text_in(p, item2.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, "Abrir abas dos agentes", theme::ui(13.0), theme::TEXT);
        let cb2 = Rect::from_center_size(Pos2::new(item2.max.x - 8.0 - 8.0, item2.center().y), Vec2::splat(16.0));
        checkbox(p, cb2, auto_tabs);
        if r.clicked() {
            toggle_tabs = true;
        }

        // ▶ Testar som
        let btn = Rect::from_min_size(Pos2::new(inner.min.x, item2.max.y + 6.0), Vec2::new(inner.width(), btn_h));
        let r = super::clickable(ui, btn, Id::new("setting-test-sound"));
        let hot = r.hovered();
        frame(p, btn, 6.0, if hot { theme::BORDER } else { theme::PANEL }, theme::BORDER);
        text_in(p, btn, Align2::CENTER_CENTER, "▶ Testar som", theme::ui(12.0), if hot { theme::WHITE } else { theme::TEXT });
        if r.clicked() {
            test = true;
        }
    });
    if toggle {
        deck.sound_enabled = !deck.sound_enabled;
        let v = deck.sound_enabled;
        deck.core.update_config(|c| c.sound_enabled = v);
    }
    if toggle_tabs {
        deck.auto_open_agent_tabs = !deck.auto_open_agent_tabs;
        let v = deck.auto_open_agent_tabs;
        deck.core.update_config(|c| c.auto_open_agent_tabs = v);
    }
    if test {
        deck.play_sound(true);
    }
    if closes_on_outside(ctx, menu_rect, anchor) {
        deck.settings_open = false;
    }
}

/// Checkbox nativo com `accent-color: var(--claude)`.
fn checkbox(p: &egui::Painter, r: Rect, checked: bool) {
    if checked {
        p.rect_filled(r, 3.0, theme::CLAUDE);
        let s = Stroke::new(2.0, theme::WHITE);
        let a = Pos2::new(r.min.x + 3.5, r.center().y + 0.5);
        let b = Pos2::new(r.min.x + 6.8, r.max.y - 4.0);
        let c = Pos2::new(r.max.x - 3.5, r.min.y + 4.0);
        p.line_segment([a, b], s);
        p.line_segment([b, c], s);
    } else {
        p.rect(r, 3.0, theme::PANEL_2, Stroke::new(1.0, theme::MUTED), egui::StrokeKind::Inside);
    }
}

/// Lista de opções do `<select id="target">` com os `<optgroup>`.
pub fn target_menu(deck: &mut Deck, ctx: &egui::Context, anchor: Rect) {
    if !deck.target_menu_open {
        return;
    }
    let groups = deck.agents.targets.clone();
    let opt_font = theme::ui(12.5);
    let group_font = theme::ui_bold(11.0);
    let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("measure")));
    let w = groups
        .iter()
        .flat_map(|g| g.options.iter().map(|o| text_w(&painter, &o.label, opt_font.clone()) + 32.0))
        .fold(anchor.width(), f32::max);
    let row_h = 20.0;
    let n_rows: usize = groups.iter().map(|g| 1 + g.options.len()).sum();
    let h = n_rows as f32 * row_h + 2.0;
    // Abre para cima se não couber abaixo (o seletor fica no rodapé da janela).
    let screen = ctx.content_rect();
    let pos = if anchor.max.y + h > screen.max.y { Pos2::new(anchor.min.x, anchor.min.y - h) } else { Pos2::new(anchor.min.x, anchor.max.y) };
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    let current = deck.target.clone();
    Area::new(Id::new("target-menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        let p = ui.painter();
        shadow(p, rect, 4, 4, 12, theme::rgba(0, 0, 0, 0.5));
        frame(p, rect, 4.0, theme::PANEL_2, theme::BORDER);
        let mut y = rect.min.y + 1.0;
        for g in &groups {
            let gr = Rect::from_min_size(Pos2::new(rect.min.x + 1.0, y), Vec2::new(w - 2.0, row_h));
            p.rect_filled(gr, 0.0, theme::PANEL);
            text_in(p, gr.shrink2(Vec2::new(6.0, 0.0)), Align2::LEFT_CENTER, &g.label, group_font.clone(), theme::MUTED);
            y += row_h;
            for o in &g.options {
                let r = Rect::from_min_size(Pos2::new(rect.min.x + 1.0, y), Vec2::new(w - 2.0, row_h));
                y += row_h;
                let resp = super::clickable(ui, r, Id::new(("target-opt", &o.value)));
                let selected = o.value == current;
                let (bg, fg) = if resp.hovered() {
                    (theme::rgba(79, 140, 247, 0.85), theme::WHITE)
                } else if selected {
                    (theme::BORDER, theme::TEXT)
                } else {
                    (theme::PANEL_2, theme::SELECT_TEXT)
                };
                p.rect_filled(r, 0.0, bg);
                let g = galley_trunc(p, &o.label, opt_font.clone(), fg, r.width() - 24.0);
                place(p, Rect::from_min_max(Pos2::new(r.min.x + 18.0, r.min.y), r.max), Align2::LEFT_CENTER, g, fg);
                if resp.clicked() {
                    chosen = Some(o.value.clone());
                }
            }
        }
    });
    if let Some(v) = chosen {
        deck.set_target(v);
        deck.target_menu_open = false;
    } else if closes_on_outside(ctx, menu_rect, anchor) {
        deck.target_menu_open = false;
    }
}

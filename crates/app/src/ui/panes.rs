//! Área dos terminais: a tela dividida por tipo de agente (uma janela por grupo, cada uma com o
//! seu cabeçalho de abas), os divisores arrastáveis e a troca de lugar pela alça.

use std::time::Duration;

use egui::{Align2, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::tabs::{self, StripClick};
use super::{clickable, empty_state, galley, paint_splitter, place, tip};
use crate::deck::{Deck, PanePlan};
use crate::split;
use crate::terminal::view::{self, Metrics};
use crate::theme;

/// Altura do cabeçalho de abas de cada janela.
pub const HEAD_H: f32 = 32.0;
const GRIP_W: f32 = 18.0;
const PLUS_W: f32 = 30.0;

/// Área do terminal dentro de uma janela (abaixo do cabeçalho).
pub fn pane_body(rect: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(rect.min.x, (rect.min.y + HEAD_H).min(rect.max.y)), rect.max)
}

/// O que um clique numa janela pede (aplicado depois de pintar todas, porque fechar uma aba muda
/// os índices das outras).
enum Click {
    Focus(usize),
    Close(usize),
    NewTab(String),
    Swap(usize, usize),
    Mic(u64),
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, area: Rect) {
    deck.terminal_rect = Some(pane_body(area));
    if deck.visible().is_empty() {
        deck.panes.clear();
        if deck.initialized {
            deck.pane_drag = None;
            empty_state::draw(deck, ui, area);
        }
        return;
    }
    let Some(metrics) = deck.metrics.clone() else { return };
    let (shape, plans) = deck.plan_panes(area);
    let multi = plans.len() > 1;
    let focused_pane = deck.active.and_then(|a| plans.iter().position(|p| p.tabs.contains(&a)));
    // O fundo aparece só nas frestas entre as janelas (os divisores).
    ui.painter().rect_filled(area, 0.0, theme::BORDER);

    let mut clicks = Vec::new();
    for (slot, plan) in plans.iter().enumerate() {
        draw_pane(deck, ui, plan, slot, focused_pane == Some(slot), multi, &metrics, &mut clicks);
    }
    if multi {
        dividers(deck, ui, area, &shape, &plans);
        drag_overlay(deck, ui);
    }
    for c in clicks {
        match c {
            Click::Focus(i) => focus(deck, ui, i),
            Click::Close(i) => deck.close_tab(i),
            Click::NewTab(kind) => deck.open_tab(&kind, false),
            Click::Swap(a, b) => deck.swap_panes(a, b),
            Click::Mic(uid) => deck.toggle_tab_mic(uid),
        }
    }
}

/// Foco no terminal de uma aba (clique no terminal, no cabeçalho ou numa aba).
fn focus(deck: &mut Deck, ui: &Ui, idx: usize) {
    ui.ctx().memory_mut(|m| {
        if let Some(f) = m.focused() {
            m.surrender_focus(f);
        }
    });
    if deck.active == Some(idx) {
        deck.set_term_focus(true);
    } else {
        deck.activate(idx);
    }
    deck.new_tab_menu_open = false;
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(deck: &mut Deck, ui: &mut Ui, plan: &PanePlan, slot: usize, focused: bool, multi: bool, m: &Metrics, clicks: &mut Vec<Click>) {
    let ctx = ui.ctx().clone();
    let head = Rect::from_min_size(plan.rect.min, Vec2::new(plan.rect.width(), HEAD_H));
    let body = pane_body(plan.rect);
    let painter = ui.painter_at(plan.rect);
    painter.rect_filled(head, 0.0, theme::PANEL);
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::BORDER);
    let group = plan.groups.first().cloned().unwrap_or_default();

    // Clique no espaço vazio do cabeçalho foca a janela (as abas, por cima, têm a própria ação).
    if ui.interact(head, Id::new(("pane-head", &group)), Sense::click()).clicked() {
        clicks.push(Click::Focus(plan.shown));
    }

    // ---------- Alça (arrastar sobre outra janela troca as duas de lugar) ----------
    let mut x0 = head.min.x;
    if multi {
        let grip = Rect::from_min_size(head.min, Vec2::new(GRIP_W, HEAD_H - 1.0));
        let r = ui.interact(grip, Id::new(("pane-grip", &group)), Sense::drag());
        let dragging = deck.pane_drag == Some(slot);
        if r.hovered() || dragging {
            ctx.set_cursor_icon(if dragging { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
        let c = if r.hovered() || dragging { theme::TEXT } else { theme::MUTED };
        for (dx, dy) in [(-2.5, -4.0), (2.5, -4.0), (-2.5, 0.0), (2.5, 0.0), (-2.5, 4.0), (2.5, 4.0)] {
            painter.circle_filled(grip.center() + Vec2::new(dx, dy), 1.2, c);
        }
        if r.drag_started() {
            deck.pane_drag = Some(slot);
        }
        if r.drag_stopped() {
            if let Some(to) = drop_target(deck, ui, slot) {
                clicks.push(Click::Swap(slot, to));
            }
            deck.pane_drag = None;
        }
        tip(r, "Arraste sobre outra janela para trocar de lugar");
        x0 = grip.max.x;
    }

    // ---------- Abas ----------
    let plus = Rect::from_min_max(Pos2::new(head.max.x - PLUS_W, head.min.y), Pos2::new(head.max.x, head.max.y - 1.0));
    let strip_rect = Rect::from_min_max(Pos2::new(x0, head.min.y), Pos2::new(plus.min.x.max(x0), head.max.y - 1.0));
    match tabs::strip(deck, ui, strip_rect, &plan.tabs, plan.shown, focused || !multi, Id::new(("pane-tabs", &group))) {
        Some(StripClick::Activate(i)) => clicks.push(Click::Focus(i)),
        Some(StripClick::Close(i)) => clicks.push(Click::Close(i)),
        Some(StripClick::Rename(i)) => deck.start_rename_tab(i),
        Some(StripClick::ContextMenu(i, pos)) => deck.tab_context_menu = Some((i, pos)),
        Some(StripClick::Mic(uid)) => clicks.push(Click::Mic(uid)),
        None => {}
    }

    // ---------- "+": outra aba do mesmo tipo da visível ----------
    let kind = deck.tabs[plan.shown].kind.clone();
    let r = clickable(ui, plus, Id::new(("pane-plus", &group)));
    if r.hovered() {
        painter.rect_filled(plus, 0.0, theme::PANEL_2);
    }
    let pg = galley(&painter, "+", theme::ui(17.0), theme::MUTED);
    place(&painter, plus, Align2::CENTER_CENTER, pg, if r.hovered() { theme::TEXT } else { theme::MUTED });
    if tip(r, format!("Nova aba de {}", deck.agents.name(&kind))).clicked() {
        clicks.push(Click::NewTab(kind));
    }

    // ---------- Terminal ----------
    let idx = plan.shown;
    deck.fit_tab(idx, body);
    let has_focus = focused && deck.term_focus && deck.window_focused;
    let elapsed = deck.blink_epoch.elapsed().as_millis();
    let blink_on = (elapsed / 600).is_multiple_of(2);
    let preedit = if has_focus { deck.ime_preedit.clone() } else { String::new() };
    let tab = &mut deck.tabs[idx];
    let color = tab.color;
    let id = Id::new(("terminal", tab.uid));
    let resp = view::show(ui, body, id, &tab.session, &mut tab.view, has_focus, blink_on, &preedit, m);
    // IME do sistema ancorado no cursor do terminal focado (candidatos e seletor de emoji).
    if let (true, Some(cell)) = (has_focus, resp.cursor_rect) {
        ctx.output_mut(|o| {
            o.ime = Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Terminal,
                rect: cell,
                cursor_rect: cell,
                should_interrupt_composition: false,
            });
        });
    }
    if resp.focus {
        clicks.push(Click::Focus(idx));
    }
    if !resp.to_pty.is_empty() {
        deck.terminal_mouse_bytes(idx, &resp.to_pty);
    }
    if has_focus {
        let next = 600 - (elapsed % 600) as u64;
        ctx.request_repaint_after(Duration::from_millis(next));
    }

    // Janela focada: contorno na cor do agente.
    if multi && focused {
        painter.rect_stroke(plan.rect, 0.0, Stroke::new(1.0, theme::with_opacity(color, 0.85)), egui::StrokeKind::Inside);
    }
}

/// Janela sob o ponteiro (outra que não `from`) enquanto uma alça é arrastada.
fn drop_target(deck: &Deck, ui: &Ui, from: usize) -> Option<usize> {
    if deck.debug_pane_drop.is_some() {
        return deck.debug_pane_drop;
    }
    let p = ui.input(|i| i.pointer.latest_pos())?;
    deck.panes.iter().position(|s| s.rect.contains(p)).filter(|&t| t != from)
}

/// Destaque da janela de origem e do alvo enquanto uma alça é arrastada.
fn drag_overlay(deck: &Deck, ui: &Ui) {
    let Some(from) = deck.pane_drag else { return };
    let painter = ui.painter();
    let accent = deck.active_color();
    if let Some(src) = deck.panes.get(from) {
        painter.rect_filled(src.rect, 0.0, theme::rgba(0, 0, 0, 0.35));
    }
    if let Some(to) = drop_target(deck, ui, from) {
        let r = deck.panes[to].rect;
        painter.rect_filled(r, 0.0, theme::with_opacity(accent, 0.12));
        painter.rect_stroke(r.shrink(1.0), 0.0, Stroke::new(2.0, accent), egui::StrokeKind::Inside);
        super::text_in(painter, r, Align2::CENTER_CENTER, "Soltar para trocar de lugar", theme::ui_semibold(13.0), theme::TEXT);
    }
}

/// Divisores entre as linhas e entre as janelas de cada linha: arrastar redimensiona, duplo clique
/// iguala.
fn dividers(deck: &mut Deck, ui: &mut Ui, area: Rect, shape: &[usize], plans: &[PanePlan]) {
    let painter = ui.painter_at(area);
    let color = deck.active_color();
    let rects: Vec<Rect> = plans.iter().map(|p| p.rect).collect();
    let mut rows: Vec<&[Rect]> = Vec::new();
    let mut first = 0;
    for &c in shape {
        let end = (first + c).min(rects.len());
        rows.push(&rects[first..end]);
        first = end;
    }
    let mut changed = false;
    let mut stopped = false;
    // As salvas só se adaptam ao formato atual quando o usuário mexe num divisor.
    let touched = |deck: &mut Deck| deck.split_ratios.conform(shape);
    // Entre linhas.
    for (r, row) in rows.iter().enumerate().take(rows.len().saturating_sub(1)) {
        let Some(above) = row.first() else { continue };
        let line = Rect::from_min_size(Pos2::new(area.min.x, above.max.y), Vec2::new(area.width(), split::GAP));
        let resp = ui.interact(line.expand2(Vec2::new(0.0, 3.0)), Id::new(("split-row", r)), Sense::click_and_drag());
        if resp.hovered() || resp.dragged() {
            ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
        }
        if resp.dragged() || resp.double_clicked() {
            touched(deck);
        }
        if resp.dragged() {
            split::drag(&mut deck.split_ratios.rows, r, resp.drag_delta().y, area.height(), split::MIN_H);
            changed = true;
        }
        if resp.double_clicked() {
            deck.split_ratios.rows = split::equal(shape.len());
            stopped = true;
        }
        stopped |= resp.drag_stopped();
        paint_splitter(&painter, line, resp.hovered() || resp.dragged(), color);
        tip(resp, "Arraste para redimensionar · duplo clique iguala");
    }
    // Dentro de cada linha.
    for (r, row) in rows.iter().enumerate() {
        for c in 0..row.len().saturating_sub(1) {
            let left = row[c];
            let line = Rect::from_min_max(Pos2::new(left.max.x, left.min.y), Pos2::new(left.max.x + split::GAP, left.max.y));
            let resp = ui.interact(line.expand2(Vec2::new(3.0, 0.0)), Id::new(("split-col", r, c)), Sense::click_and_drag());
            if resp.hovered() || resp.dragged() {
                ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
            }
            if resp.dragged() || resp.double_clicked() {
                touched(deck);
            }
            if let Some(cols) = deck.split_ratios.cols.get_mut(r) {
                if resp.dragged() {
                    split::drag(cols, c, resp.drag_delta().x, area.width(), split::MIN_W);
                    changed = true;
                }
                if resp.double_clicked() {
                    *cols = split::equal(cols.len());
                    stopped = true;
                }
            }
            stopped |= resp.drag_stopped();
            paint_splitter(&painter, line, resp.hovered() || resp.dragged(), color);
            tip(resp, "Arraste para redimensionar · duplo clique iguala");
        }
    }
    if changed {
        ui.ctx().request_repaint();
    }
    if stopped {
        deck.persist_layout();
    }
}

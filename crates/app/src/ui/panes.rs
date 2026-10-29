//! Terminal area: the split screen by agent type (one window per group, each with its own tab
//! header), the draggable dividers and the swapping by the grip.

use std::time::Duration;

use egui::{Align2, CursorIcon, Id, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::tabs::{self, StripClick};
use super::{clickable, empty_state, paint_splitter, text_in, tip};
use crate::audio;
use crate::deck::{Deck, PanePlan};
use crate::dictation::{self, ButtonVisual, MicTarget};
use crate::icons::Icon;
use crate::split;
use crate::terminal::view::{self, Metrics};
use crate::theme;
use agentdeck_core::t;
use agentdeck_core::Organization;

/// Height of the tab header of each window.
pub const HEAD_H: f32 = 30.0;
const GRIP_W: f32 = 18.0;
const ICON_BTN: f32 = 20.0;

/// Terminal area inside a window (below the header).
pub fn pane_body(rect: Rect) -> Rect {
    Rect::from_min_max(Pos2::new(rect.min.x, (rect.min.y + HEAD_H).min(rect.max.y)), rect.max)
}

/// What a click on a window asks for (applied after painting all of them, because closing a tab changes
/// the indexes of the others).
enum Click {
    Focus(usize),
    Close(usize),
    NewTab(String),
    Swap(usize, usize),
    Mic(u64),
    Attach(u64),
}

/// The islands of the center: one per window (or the whole area, in the empty state).
pub fn island_rects(deck: &mut Deck, area: Rect) -> Vec<Rect> {
    if deck.visible().is_empty() {
        return vec![area];
    }
    deck.plan_panes(area).1.iter().map(|p| p.rect).collect()
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, area: Rect) {
    deck.terminal_rect = Some(pane_body(area));
    if deck.visible().is_empty() {
        deck.panes.clear();
        super::island(ui, area, super::IslandKind::Well);
        if deck.initialized {
            deck.pane_drag = None;
            empty_state::draw(deck, ui, area);
        }
        super::island_edge(ui, area, super::IslandKind::Well);
        return;
    }
    let Some(metrics) = deck.metrics.clone() else { return };
    let (shape, plans) = deck.plan_panes(area);
    let multi = plans.len() > 1;
    // In Highlight, window 0 is the big one; the others are the small ones, with the bring button.
    let highlight = multi && deck.organization == Organization::Highlight;
    let focused_pane = deck.active.and_then(|a| plans.iter().position(|p| p.tabs.contains(&a)));

    let mut clicks = Vec::new();
    for (slot, plan) in plans.iter().enumerate() {
        draw_pane(deck, ui, plan, slot, focused_pane == Some(slot), Mode { multi, highlight }, &metrics, &mut clicks);
    }
    if highlight {
        highlight_divider(deck, ui, area, &plans);
    } else if multi {
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
            Click::Attach(uid) => deck.pick_tab_attachments(uid),
        }
    }
}

/// Focus on the terminal of a tab (click on the terminal, on the header or on a tab).
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

/// How the windows of this frame are arranged.
#[derive(Clone, Copy)]
struct Mode {
    /// There is more than one window (the highlight and the grip only exist then).
    multi: bool,
    /// Highlight: window 0 is the big one and the others have the "Bring to highlight" button.
    highlight: bool,
}

#[allow(clippy::too_many_arguments)]
fn draw_pane(deck: &mut Deck, ui: &mut Ui, plan: &PanePlan, slot: usize, focused: bool, mode: Mode, m: &Metrics, clicks: &mut Vec<Click>) {
    let Mode { multi, highlight } = mode;
    let ctx = ui.ctx().clone();
    let head = Rect::from_min_size(plan.rect.min, Vec2::new(plan.rect.width(), HEAD_H));
    let body = pane_body(plan.rect);
    super::island(ui, plan.rect, super::IslandKind::Well);
    let painter = ui.painter_at(plan.rect);
    painter.rect_filled(head, 0.0, theme::pal().strip);
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);
    let group = plan.groups.first().cloned().unwrap_or_default();

    // ---------- Grip (dragging over another window swaps the two) ----------
    let mut x0 = head.min.x;
    if multi && !highlight {
        let grip = Rect::from_min_size(head.min, Vec2::new(GRIP_W, HEAD_H - 1.0));
        let r = ui.interact(grip, Id::new(("pane-grip", &group)), Sense::drag());
        let dragging = deck.pane_drag == Some(slot);
        if r.hovered() || dragging {
            ctx.set_cursor_icon(if dragging { CursorIcon::Grabbing } else { CursorIcon::Grab });
        }
        let c = if r.hovered() || dragging { theme::pal().text } else { theme::pal().text_2 };
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
        tip(r, t!("Drag over another window to swap places"));
        x0 = grip.max.x;
    }

    // ---------- Action buttons to the right of the header (20×20, corners 4) ----------
    let cy = head.min.y + (HEAD_H - 1.0) / 2.0;
    let plus = Rect::from_min_size(Pos2::new(head.max.x - 6.0 - ICON_BTN, cy - ICON_BTN / 2.0), Vec2::splat(ICON_BTN));
    let attach = Rect::from_min_size(Pos2::new(plus.min.x - 4.0 - ICON_BTN, cy - ICON_BTN / 2.0), Vec2::splat(ICON_BTN));
    let mic = Rect::from_min_size(Pos2::new(attach.min.x - 4.0 - ICON_BTN, cy - ICON_BTN / 2.0), Vec2::splat(ICON_BTN));
    // Small window of the Highlight: "Bring to highlight" to the left of the other buttons.
    let bring = (highlight && slot > 0)
        .then(|| Rect::from_min_size(Pos2::new(mic.min.x - 4.0 - ICON_BTN, cy - ICON_BTN / 2.0), Vec2::splat(ICON_BTN)));

    // ---------- Tabs ----------
    let strip_end = bring.unwrap_or(mic).min.x - 4.0;
    let strip_rect = Rect::from_min_max(Pos2::new(x0, head.min.y), Pos2::new(strip_end.max(x0), head.max.y - 1.0));
    // A click on the empty space of the tab strip focuses the window (the tabs, on top, have their own action).
    if ui.interact(strip_rect, Id::new(("pane-head", &group)), Sense::click()).clicked() {
        clicks.push(Click::Focus(plan.shown));
    }
    match tabs::strip(deck, ui, strip_rect, &plan.tabs, plan.shown, focused || !multi, Id::new(("pane-tabs", &group))) {
        Some(StripClick::Activate(i)) => clicks.push(Click::Focus(i)),
        Some(StripClick::Close(i)) => clicks.push(Click::Close(i)),
        Some(StripClick::Rename(i)) => deck.start_rename_tab(i),
        Some(StripClick::ContextMenu(i, pos)) => deck.tab_context_menu = Some((i, pos)),
        None => {}
    }

    if let Some(bring) = bring {
        let r = clickable(ui, bring, Id::new(("pane-bring", &group)));
        if r.hovered() {
            painter.rect_filled(bring, 4.0, theme::pal().hover);
        }
        let fg = if r.hovered() { theme::pal().text } else { theme::pal().text_2 };
        deck.icons.paint(&painter, Icon::Expand, bring.center(), 12.0, fg, 0.0);
        if tip(r, t!("Bring to highlight")).clicked() {
            clicks.push(Click::Focus(plan.shown));
        }
    }

    // ---------- Microphone and attachment of the visible tab (`plan.shown`) + "+" ----------
    let shown_uid = deck.tabs[plan.shown].uid;
    let shown_alive = deck.tabs[plan.shown].alive;
    let shown_label = deck.tabs[plan.shown].label.clone();
    let kind = deck.tabs[plan.shown].kind.clone();

    let visual = dictation::button_visual(deck.mic_phase, MicTarget::Tab(shown_uid));
    let mic_busy = visual.is_busy() || !shown_alive;
    let mic_resp = ui.interact(mic, Id::new(("pane-mic", &group)), if mic_busy { Sense::hover() } else { Sense::click() });
    let mic_hot = mic_resp.hovered() && !mic_busy;
    if mic_hot {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let mut mic_opacity = 1.0;
    if visual == ButtonVisual::Recording {
        mic_opacity = super::right_panel::blink_alpha(ctx.input(|inp| inp.time), 1.2);
        ctx.request_repaint_after(Duration::from_millis(50));
        painter.rect_filled(mic, 4.0, theme::with_opacity(theme::pal().danger, mic_opacity));
    } else if mic_hot {
        painter.rect_filled(mic, 4.0, theme::pal().hover);
    }
    if mic_busy || !audio::AVAILABLE || visual == ButtonVisual::Opening {
        mic_opacity = 0.45;
    }
    let mic_fg = if visual == ButtonVisual::Recording {
        theme::pal().text_strong
    } else if mic_hot {
        theme::pal().text
    } else {
        theme::pal().text_2
    };
    deck.icons.paint(&painter, Icon::Mic, mic.center(), 14.0, theme::with_opacity(mic_fg, mic_opacity), 0.0);
    let mic_tip = if !shown_alive {
        t!("Dictation unavailable (process ended)").to_owned()
    } else {
        dictation::mic_tooltip(visual, audio::AVAILABLE, deck.dictation_setup(), &deck.engine_label, Some("Ctrl+Shift+M"))
    };
    if tip(mic_resp, mic_tip).clicked() && !mic_busy {
        clicks.push(Click::Mic(shown_uid));
    }

    let att_resp = ui.interact(attach, Id::new(("pane-attach", &group)), if shown_alive { Sense::click() } else { Sense::hover() });
    let att_hot = att_resp.hovered() && shown_alive;
    if att_hot {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
        painter.rect_filled(attach, 4.0, theme::pal().hover);
    }
    let att_fg = if att_hot {
        theme::pal().text
    } else if shown_alive {
        theme::pal().text_2
    } else {
        theme::with_opacity(theme::pal().text_2, 0.45)
    };
    deck.icons.paint(&painter, Icon::Attach, attach.center(), 14.0, att_fg, 0.0);
    let att_tip = if shown_alive {
        t!("Attach files to {label}", label = shown_label)
    } else {
        t!("Attachments unavailable (process ended)").to_owned()
    };
    if tip(att_resp, att_tip).clicked() && shown_alive {
        clicks.push(Click::Attach(shown_uid));
    }

    let r = clickable(ui, plus, Id::new(("pane-plus", &group)));
    if r.hovered() {
        painter.rect_filled(plus, 4.0, theme::pal().hover);
    }
    text_in(&painter, plus, Align2::CENTER_CENTER, "+", theme::ui(16.0), if r.hovered() { theme::pal().text } else { theme::pal().text_2 });
    if tip(r, t!("New {agent} tab", agent = deck.agents.name(&kind))).clicked() {
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
    // System IME anchored on the focused terminal's cursor (candidates and emoji picker).
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

    // Highlight (Split and Highlight): the other windows sit at 62% and go back to 100% with the mouse over them.
    if multi {
        let dim = !focused && !ui.rect_contains_pointer(plan.rect);
        let fade = ctx.animate_bool_with_time(Id::new(("pane-dim", &group)), dim, 0.12);
        if fade > 0.0 {
            painter.rect_filled(
                plan.rect,
                theme::ISLAND_RADIUS as f32,
                theme::with_opacity(theme::pal().bg, (1.0 - theme::PANE_DIM_OPACITY) * fade),
            );
        }
    }
    super::island_edge(ui, plan.rect, super::IslandKind::Well);
    // The selected agent's window: a 1 px border and a 3 px halo in its color.
    if multi && focused {
        let radius = theme::ISLAND_RADIUS as f32;
        ui.painter().rect_stroke(plan.rect, radius, Stroke::new(theme::PANE_HALO_W, theme::accent_soft(color)), egui::StrokeKind::Outside);
        painter.rect_stroke(plan.rect, radius, Stroke::new(1.0, color), egui::StrokeKind::Inside);
    }
}

/// Window under the pointer (other than `from`) while a grip is being dragged.
fn drop_target(deck: &Deck, ui: &Ui, from: usize) -> Option<usize> {
    if deck.debug_pane_drop.is_some() {
        return deck.debug_pane_drop;
    }
    let p = ui.input(|i| i.pointer.latest_pos())?;
    deck.panes.iter().position(|s| s.rect.contains(p)).filter(|&t| t != from)
}

/// Highlight of the source window and of the target while a grip is being dragged.
fn drag_overlay(deck: &Deck, ui: &Ui) {
    let Some(from) = deck.pane_drag else { return };
    let painter = ui.painter();
    let accent = deck.accent();
    if let Some(src) = deck.panes.get(from) {
        painter.rect_filled(src.rect, theme::ISLAND_RADIUS as f32, theme::with_opacity(theme::pal().scrim, 0.7));
    }
    if let Some(to) = drop_target(deck, ui, from) {
        let r = deck.panes[to].rect;
        painter.rect_filled(r, theme::ISLAND_RADIUS as f32, theme::accent_wash(accent));
        painter.rect_stroke(r.shrink(1.0), theme::ISLAND_RADIUS as f32 - 1.0, Stroke::new(2.0, accent), egui::StrokeKind::Inside);
        super::text_in(painter, r, Align2::CENTER_CENTER, t!("Drop to swap places"), theme::ui_semibold(13.0), theme::pal().text);
    }
}

/// Dividers between the rows and between the windows of each row: dragging resizes, a double click
/// equalizes.
fn dividers(deck: &mut Deck, ui: &mut Ui, area: Rect, shape: &[usize], plans: &[PanePlan]) {
    let painter = ui.painter_at(area);
    let color = deck.accent();
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
    // The saved ones only adapt to the current shape when the user moves a divider.
    let touched = |deck: &mut Deck| deck.split_ratios.conform(shape);
    // Between rows.
    for (r, row) in rows.iter().enumerate().take(rows.len().saturating_sub(1)) {
        let Some(above) = row.first() else { continue };
        let line = Rect::from_min_size(Pos2::new(area.min.x, above.max.y), Vec2::new(area.width(), split::GAP));
        let resp = ui.interact(line, Id::new(("split-row", r)), Sense::click_and_drag());
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
        tip(resp, t!("Drag to resize · double click to equalize"));
    }
    // Inside each row.
    for (r, row) in rows.iter().enumerate() {
        for c in 0..row.len().saturating_sub(1) {
            let left = row[c];
            let line = Rect::from_min_max(Pos2::new(left.max.x, left.min.y), Pos2::new(left.max.x + split::GAP, left.max.y));
            let resp = ui.interact(line, Id::new(("split-col", r, c)), Sense::click_and_drag());
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
            tip(resp, t!("Drag to resize · double click to equalize"));
        }
    }
    if changed {
        ui.ctx().request_repaint();
    }
    if stopped {
        deck.persist_layout();
    }
}

/// Divider between the big window and the Highlight column: dragging changes the ratio, a double click
/// goes back to 2/3.
fn highlight_divider(deck: &mut Deck, ui: &mut Ui, area: Rect, plans: &[PanePlan]) {
    let Some(big) = plans.first() else { return };
    let line = Rect::from_min_max(Pos2::new(big.rect.max.x, area.min.y), Pos2::new(big.rect.max.x + split::GAP, area.max.y));
    let resp = ui.interact(line, Id::new("split-highlight"), Sense::click_and_drag());
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.dragged() {
        split::drag_highlight(&mut deck.highlight_ratio, resp.drag_delta().x, area.width());
        ui.ctx().request_repaint();
    }
    if resp.double_clicked() {
        deck.highlight_ratio = split::HIGHLIGHT_RATIO;
    }
    if resp.drag_stopped() || resp.double_clicked() {
        deck.persist_layout();
    }
    paint_splitter(&ui.painter_at(area), line, resp.hovered() || resp.dragged(), deck.accent());
    tip(resp, t!("Drag to resize · double click to reset"));
}

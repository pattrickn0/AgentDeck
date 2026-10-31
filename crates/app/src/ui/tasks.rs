//! The right bar's "Tasks" tab, as a queue: the tasks grouped by who has the next move (you, an agent, nobody yet, done),
//! one row each, with the two actions of what needs you on the row itself; the detail of a task (the lifecycle track,
//! the explanation of its state, the report / changes / timeline / request panes and the actions pinned at the bottom)
//! and the new task form. The state and the calls into the core live in `tasks_ui.rs`; here we only paint and *collect*
//! what was clicked ([`Act`]), applied at the end of the frame with the whole `Deck`.

use egui::{Align2, Area, Color32, Id, Key, Margin, Modifiers, Order, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2};

use holt_core::capability::Level;
use holt_core::models::{self, Effort};
use holt_core::usage::ModelUsage;
use holt_core::{t, FeedKind, MergeBlock, Task, TaskStatus};

use super::{
    button as ui_button, button_danger, button_width, clickable, frame, galley, galley_spaced, galley_trunc, place, segmented,
    separator as line_sep, shadow, text_in, text_w, tip, ButtonKind,
};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::tasks_ui::{
    self, actions_for, age_short, can_show_diff, clip_text, lanes, lifecycle, mark_of, parse_diff_stat, state_sentence, state_word,
    status_age, status_name, task_date_ddmm, waiting_since, Confirm, DetailTab, FormMenu, HistoryFilter, Lane, Mark, Moment, StopState,
    TaskAct, TasksView,
};
use crate::theme;
use crate::util;

const HEAD_H: f32 = theme::TOPBAR_H;
const BTN_H: f32 = 28.0;
/// Buttons on a row of the list.
const ROW_BTN_H: f32 = 24.0;
const CHIP_H: f32 = 21.0;
const PAD: f32 = 12.0;
const LIST_PAD: f32 = 8.0;
const FOOTER_H: f32 = 45.0;
/// The glyph column of a row and the gap after it.
const GLYPH_W: f32 = 18.0;
/// Rows of the "Finished" lane.
const DONE_ROW_H: f32 = 24.0;
/// How many characters of the request and the report go to the screen.
const TEXT_MAX: usize = 20_000;
/// Lines of `git diff --stat` shown in the detail.
const STAT_MAX_LINES: usize = 40;
/// Files named in the "touched outside the scope" notice.
const OUTSIDE_SHOWN: usize = 8;
/// Timeline items shown at once (events and messages, newest first).
const MOMENTS_MAX: usize = 60;

/// What was clicked in this frame.
enum Act {
    New,
    Back,
    Open(String),
    ToggleHistoryMenu,
    CloseHistoryMenu,
    /// Cancels the automatic merge of a task whose conflict is with the agent.
    CancelAutoMerge(String),
    SetHistoryFilter(HistoryFilter),
    SetTab(DetailTab),
    /// Opens or collapses a timeline message (task id, message time).
    ToggleMessage(String, u64),
    Task(String, TaskAct),
    /// "View diff" / "What changed": opens the diff in the center.
    OpenDiff(String),
    /// "Open in Documents" on the request (`false`) or on the report (`true`).
    OpenText(String, bool),
    DismissNotice,
    /// "Stash my changes and merge" in the refusal notice.
    MergeStash(String),
    /// "Ask the agent to commit" / "…resolve the conflict".
    AskAgent(String),
    /// "See what is uncommitted" / "Hide the list".
    ToggleNoticeFiles,
    ConfirmYes,
    ConfirmNo,
    SendReturn(String),
    CancelReturn,
    /// The detail's "⋯" menu.
    ToggleMore,
    CloseMore,
    ToggleMenu(FormMenu),
    CloseMenu,
    SetKind(String),
    SetModel(Option<String>),
    SetEffort(Option<Effort>),
    SetLevel(Level),
    /// Step of the model menu: the provider whose models are on show (`None` = list of providers).
    SetProvider(Option<String>),
    ToggleStartNow,
    ToggleFormMore,
    PickAttachments,
    RemoveAttachment(usize),
    Submit,
}

/// The detail's "⋯" menu is open (ephemeral: closes with the detail).
fn more_id() -> Id {
    Id::new("tasks-more-open")
}

pub fn draw(deck: &mut Deck, ui: &mut Ui, rect: Rect) {
    let mut acts = Vec::new();
    let now = holt_core::util::now_ms();
    match deck.tasks.view.clone() {
        TasksView::List => list_view(deck, ui, rect, now, &mut acts),
        TasksView::Detail(id) => match deck.tasks.task(&id).cloned() {
            Some(task) => detail_view(deck, ui, rect, &task, now, &mut acts),
            None => list_view(deck, ui, rect, now, &mut acts),
        },
        TasksView::New => form_view(deck, ui, rect, &mut acts),
    }
    if !matches!(deck.tasks.view, TasksView::Detail(_)) {
        ui.ctx().data_mut(|d| d.remove::<bool>(more_id()));
    }
    confirm_dialog(deck, ui.ctx(), rect, &mut acts);
    // The times ("5 min") advance by themselves.
    ui.ctx().request_repaint_after(std::time::Duration::from_secs(20));
    for act in acts {
        apply(deck, ui.ctx(), act);
    }
}

fn apply(deck: &mut Deck, ctx: &egui::Context, act: Act) {
    match act {
        Act::New => deck.tasks_open_new(),
        Act::Back => deck.tasks_back(),
        Act::Open(id) => deck.tasks_open_detail(&id),
        Act::ToggleHistoryMenu => deck.tasks.history_menu = !deck.tasks.history_menu,
        Act::CloseHistoryMenu => deck.tasks.history_menu = false,
        Act::SetHistoryFilter(f) => {
            deck.tasks.history_filter = f;
            deck.tasks.history_menu = false;
        }
        Act::SetTab(tab) => deck.tasks.detail_tab = Some(tab),
        Act::ToggleMessage(id, at_ms) => {
            if !deck.tasks.open_messages.remove(&(id.clone(), at_ms)) {
                deck.tasks.open_messages.insert((id, at_ms));
            }
        }
        Act::Task(id, act) => deck.tasks_act(&id, act),
        Act::OpenDiff(id) => deck.docs_open_task_diff(&id),
        Act::OpenText(id, report) => deck.docs_open_task_text(&id, report),
        Act::DismissNotice => deck.tasks_clear_notice(),
        Act::MergeStash(id) => deck.tasks_merge_stash(&id),
        Act::AskAgent(id) => deck.tasks_ask_agent(&id),
        Act::CancelAutoMerge(id) => deck.tasks_cancel_auto_merge(&id),
        Act::ToggleNoticeFiles => deck.tasks.notice_files = !deck.tasks.notice_files,
        Act::ConfirmYes => deck.tasks_confirm_accepted(),
        Act::ConfirmNo => deck.tasks.confirm = None,
        Act::SendReturn(id) => deck.tasks_send_return(&id),
        Act::CancelReturn => {
            deck.tasks.return_open = false;
            deck.tasks.feedback.clear();
        }
        Act::ToggleMore => {
            let open = ctx.data(|d| d.get_temp::<bool>(more_id())).unwrap_or(false);
            ctx.data_mut(|d| d.insert_temp(more_id(), !open));
        }
        Act::CloseMore => {
            ctx.data_mut(|d| d.insert_temp(more_id(), false));
        }
        Act::ToggleMenu(m) => {
            let open = deck.tasks.form.menu != Some(m);
            deck.tasks.form.menu = open.then_some(m);
            if open && m == FormMenu::Model {
                // The menu opens on the provider of the chosen model; with no model, on the list of providers.
                deck.tasks.form.provider = deck.tasks.form.model.as_deref().and_then(models::provider).map(str::to_owned);
            }
        }
        Act::CloseMenu => {
            deck.tasks.form.menu = None;
            deck.tasks.form.provider = None;
        }
        Act::SetKind(k) => deck.tasks_form_set_kind(k),
        Act::SetLevel(l) => deck.tasks_form_set_level(l),
        Act::SetProvider(p) => deck.tasks.form.provider = p,
        Act::SetModel(m) => {
            deck.tasks.form.provider = None;
            deck.tasks.form.model = m;
            deck.tasks.form.menu = None;
            deck.tasks_form_refresh();
        }
        Act::SetEffort(e) => {
            deck.tasks.form.effort = e;
            deck.tasks_form_refresh();
        }
        Act::ToggleStartNow => deck.tasks.form.start_now = !deck.tasks.form.start_now,
        Act::ToggleFormMore => deck.tasks.form.more_open = !deck.tasks.form.more_open,
        Act::PickAttachments => deck.tasks_pick_attachments(),
        Act::RemoveAttachment(i) => deck.tasks_remove_attachment(i),
        Act::Submit => deck.tasks_submit_form(),
    }
}

// ---------------------------------------------------------------------------
// Common pieces
// ---------------------------------------------------------------------------

/// The colour of a state (the word on the row, the detail's sentence and the track): the accent for what waits for
/// you, amber for a conflict or a stopped merge, red for a failure, grey for the rest.
fn mark_color(mark: Mark) -> Color32 {
    let p = theme::pal();
    match mark {
        Mark::Review | Mark::Validate => theme::accent(),
        Mark::Stopped | Mark::Conflict => p.warning,
        Mark::Failed => p.danger_text,
        Mark::Running | Mark::Queued | Mark::Draft => p.text_2,
        Mark::Done => p.success,
        Mark::Discarded => p.text_3,
    }
}

/// The 16 px state glyph centered on `c`: filled ring (review), ring with a check (test it), amber or red "!" (merge
/// stopped, failed), pulsing dot in the agent's colour (working) or amber (resolving a conflict), dashed ring (queued),
/// hollow ring (draft), green check (completed), faint ring (discarded).
fn paint_mark(ui: &Ui, c: Pos2, mark: Mark, agent: Color32) {
    let p = theme::pal();
    let painter = ui.painter();
    let accent = theme::accent();
    let check = |color: Color32, s: f32| {
        let pts = vec![c + Vec2::new(-2.6 * s, 0.2 * s), c + Vec2::new(-0.7 * s, 2.0 * s), c + Vec2::new(2.8 * s, -2.0 * s)];
        painter.add(Shape::line(pts, Stroke::new(1.6, color)));
    };
    let bang = |fill: Color32, fg: Color32| {
        painter.circle_filled(c, 7.0, fill);
        painter.line_segment([c + Vec2::new(0.0, -3.4), c + Vec2::new(0.0, 1.0)], Stroke::new(1.7, fg));
        painter.circle_filled(c + Vec2::new(0.0, 3.4), 1.0, fg);
    };
    let pulse = |color: Color32| {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        let wave = (ui.ctx().input(|i| i.time) * 4.8).sin() as f32 * 0.5 + 0.5;
        painter.circle_filled(c, 6.0 + 2.5 * wave, theme::with_opacity(color, 0.30 * (1.0 - wave)));
        painter.circle_filled(c, 4.0, color);
    };
    match mark {
        Mark::Review => {
            painter.circle_stroke(c, 7.0, Stroke::new(1.5, accent));
            painter.circle_filled(c, 3.6, accent);
        }
        Mark::Validate => {
            painter.circle_stroke(c, 7.0, Stroke::new(1.5, accent));
            check(accent, 1.0);
        }
        Mark::Stopped => bang(p.warning, p.on_accent),
        Mark::Failed => bang(p.danger, p.text_strong),
        Mark::Running => pulse(agent),
        Mark::Conflict => pulse(p.warning),
        Mark::Queued => {
            // Dashed ring: 10 arcs with gaps.
            for i in 0..10 {
                let a0 = i as f32 / 10.0 * std::f32::consts::TAU;
                let a1 = a0 + std::f32::consts::TAU / 20.0;
                let pt = |a: f32| c + Vec2::new(a.cos(), a.sin()) * 6.4;
                painter.line_segment([pt(a0), pt(a1)], Stroke::new(1.5, p.text_3));
            }
        }
        Mark::Draft => {
            painter.circle_stroke(c, 5.8, Stroke::new(1.5, p.text_faint));
        }
        Mark::Done => check(p.success, 1.2),
        Mark::Discarded => {
            painter.circle_stroke(c, 5.0, Stroke::new(1.3, p.text_faint));
            painter.line_segment([c + Vec2::new(-4.0, 4.0), c + Vec2::new(4.0, -4.0)], Stroke::new(1.3, p.text_faint));
        }
    }
}

/// The agent's mark (model logo, or the dot) at 12 px.
fn paint_agent(deck: &mut Deck, painter: &egui::Painter, c: Pos2, kind: &str) {
    let color = deck.agents.tab_color(kind);
    match super::projects::agent_icon(kind) {
        Some(icon) => deck.icons.paint(painter, icon, c, 12.0, color, 0.0),
        None => {
            painter.circle_filled(c, 3.0, color);
        }
    }
}

/// `+120 −8` in green and red (Plex Mono 10.5), right-aligned in `rect`. Returns the left edge.
fn paint_diff_counts(painter: &egui::Painter, rect: Rect, plus: u32, minus: u32) -> f32 {
    let p = theme::pal();
    let mut x = rect.max.x;
    let font = theme::meta(10.5);
    for (text, color) in [(format!("−{minus}"), p.danger_text), (format!("+{plus}"), p.success)] {
        if text.ends_with('0') && text.len() == 2 {
            continue;
        }
        let g = galley(painter, &text, font.clone(), color);
        let w = g.size().x;
        place(painter, Rect::from_min_max(Pos2::new(x - w, rect.min.y), Pos2::new(x, rect.max.y)), Align2::LEFT_CENTER, g, color);
        x -= w + 5.0;
    }
    x + 5.0
}

/// Top strip of the detail and of the form: bottom border, "←", a quiet prefix (the task id) and the title; `more`
/// reserves the "⋯" button on the right and returns its rectangle.
fn head_bar(ui: &Ui, rect: Rect, prefix: Option<&str>, title: &str, more: bool, acts: &mut Vec<Act>) -> (Rect, Option<Rect>) {
    let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEAD_H));
    let p = ui.painter_at(head);
    let pal = theme::pal();
    line_sep(&p, Pos2::new(head.min.x, head.max.y - 0.5), Pos2::new(head.max.x, head.max.y - 0.5));
    let b = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.center().y - 11.0), Vec2::new(24.0, 22.0));
    let r = clickable(ui, b, Id::new("tasks-back"));
    if r.hovered() {
        p.rect_filled(b, 5.0, pal.hover);
    }
    text_in(&p, b, Align2::CENTER_CENTER, "←", theme::ui(14.0), if r.hovered() { pal.text } else { pal.text_2 });
    if tip(r, t!("Back to the task list")).clicked() {
        acts.push(Act::Back);
    }
    let mut x = b.max.x + 6.0;
    let more_rect = more.then(|| Rect::from_min_size(Pos2::new(head.max.x - 6.0 - 24.0, head.center().y - 11.0), Vec2::new(24.0, 22.0)));
    let right = more_rect.map_or(head.max.x - PAD, |m| m.min.x - 6.0);
    if let Some(prefix) = prefix {
        let g = galley(&p, prefix, theme::meta(11.0), pal.text_3);
        x = place(&p, Rect::from_min_max(Pos2::new(x, head.min.y), Pos2::new(right, head.max.y - 1.0)), Align2::LEFT_CENTER, g, pal.text_3)
            .max
            .x
            + 7.0;
    }
    let g = galley_trunc(&p, title, theme::ui_semibold(12.5), pal.text_strong, (right - x).max(20.0));
    place(&p, Rect::from_min_max(Pos2::new(x, head.min.y), Pos2::new(right, head.max.y - 1.0)), Align2::LEFT_CENTER, g, pal.text_strong);
    (head, more_rect)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum BtnKind {
    Primary,
    Secondary,
    /// Text only, `hover` behind it: a third, quiet action.
    Ghost,
}

fn btn_width(painter: &egui::Painter, label: &str, kind: BtnKind) -> f32 {
    let font = if kind == BtnKind::Primary { theme::ui_semibold(11.5) } else { theme::ui(11.5) };
    galley(painter, label, font, theme::pal().text).size().x + 22.0
}

/// Button with text (corners 5). When off, it is dimmed and does not react. Returns whether it was clicked.
fn button(ui: &Ui, rect: Rect, id: Id, label: &str, kind: BtnKind, enabled: bool, tooltip: &str) -> bool {
    let r = if enabled { clickable(ui, rect, id) } else { ui.interact(rect, id, Sense::hover()) };
    let hot = enabled && r.hovered();
    let p = theme::pal();
    let painter = ui.painter();
    if !enabled {
        frame(painter, rect, 5.0, theme::with_alpha(p.text_strong, 0.02), p.separator);
        let g = galley_trunc(painter, label, theme::ui(11.5), p.text_3, rect.width() - 12.0);
        place(painter, rect, Align2::CENTER_CENTER, g, p.text_3);
    } else {
        match kind {
            BtnKind::Primary => {
                let fill = if hot { theme::brighten(theme::accent(), 1.1) } else { theme::accent() };
                painter.rect_filled(rect, 5.0, fill);
                let g = galley_trunc(painter, label, theme::ui_semibold(11.5), p.on_accent, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, p.on_accent);
            }
            BtnKind::Secondary => {
                let fill = theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 });
                frame(painter, rect, 5.0, fill, theme::with_alpha(p.text_strong, 0.14));
                let g = galley_trunc(painter, label, theme::ui(11.5), p.text, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, p.text);
            }
            BtnKind::Ghost => {
                if hot {
                    painter.rect_filled(rect, 5.0, p.hover);
                }
                let color = if hot { p.text } else { p.text_2 };
                let g = galley_trunc(painter, label, theme::ui(11.5), color, rect.width() - 12.0);
                place(painter, rect, Align2::CENTER_CENTER, g, color);
            }
        }
    }
    let r = if tooltip.is_empty() { r } else { tip(r, tooltip) };
    enabled && r.clicked()
}

/// Buttons in a row that wrap to the next row when they do not fit. Returns the index of the clicked one.
fn button_flow(ui: &mut Ui, buttons: &[(String, BtnKind, bool, &str)], id_salt: &str) -> Option<usize> {
    let avail = ui.available_width();
    let max_w = (avail - 2.0 * PAD).max(60.0);
    let widths: Vec<f32> = buttons.iter().map(|(l, k, ..)| btn_width(ui.painter(), l, *k).min(max_w)).collect();
    let mut clicked = None;
    let mut i = 0;
    while i < buttons.len() {
        // How many fit on this row.
        let mut end = i;
        let mut w = 0.0;
        while end < buttons.len() {
            let next = w + widths[end] + if end > i { 6.0 } else { 0.0 };
            if end > i && next > max_w {
                break;
            }
            w = next;
            end += 1;
        }
        let (row, _) = ui.allocate_exact_size(Vec2::new(avail, BTN_H + 6.0), Sense::hover());
        let mut x = row.min.x + PAD;
        for (j, (label, kind, enabled, tooltip)) in buttons.iter().enumerate().take(end).skip(i) {
            let r = Rect::from_min_size(Pos2::new(x, row.min.y), Vec2::new(widths[j], BTN_H));
            x = r.max.x + 6.0;
            if button(ui, r, Id::new(("tasks-btn", id_salt, j)), label, *kind, *enabled, tooltip) {
                clicked = Some(j);
            }
        }
        i = end;
    }
    clicked
}

/// Paragraph with line wrapping, indented `PAD` on both sides. Returns the rectangle it occupies.
fn paragraph(ui: &mut Ui, text: &str, font: egui::FontId, color: Color32) -> Rect {
    let w = (ui.available_width() - 2.0 * PAD).max(40.0);
    let g = ui.painter().layout(text.to_owned(), font, color, w);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 6.0), Sense::hover());
    ui.painter().galley(Pos2::new(r.min.x + PAD, r.min.y + 2.0), g, color);
    r
}

/// Short, quiet message (empty states inside a section).
fn muted_line(ui: &mut Ui, text: &str) {
    paragraph(ui, text, theme::ui(12.0), theme::pal().text_2);
}

/// Red strip with the error (and the optional "×"). Returns whether the "×" was clicked.
fn error_box(ui: &mut Ui, text: &str, dismiss: bool) -> bool {
    notice_box(ui, text, dismiss, true)
}

/// Notice strip: red (`danger`) for an error, amber (`warning`) for what worked but the user needs to know.
fn notice_box(ui: &mut Ui, text: &str, dismiss: bool, danger: bool) -> bool {
    let (tone, text_color) =
        if danger { (theme::pal().danger, theme::pal().danger_text) } else { (theme::pal().warning, theme::pal().text) };
    let w = ui.available_width() - 2.0 * PAD;
    let g = ui.painter().layout(text.to_owned(), theme::ui(12.0), text_color, (w - 22.0 - if dismiss { 18.0 } else { 0.0 }).max(40.0));
    let h = g.size().y + 16.0;
    let (outer, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), h + 12.0), Sense::hover());
    let r = Rect::from_min_size(Pos2::new(outer.min.x + PAD, outer.min.y + 3.0), Vec2::new(w, h));
    ui.painter().rect_filled(r, 7.0, theme::with_alpha(tone, 0.12));
    ui.painter().rect_stroke(r, 7.0, Stroke::new(1.0, theme::with_alpha(tone, 0.45)), egui::StrokeKind::Inside);
    ui.painter().galley(Pos2::new(r.min.x + 10.0, r.min.y + 8.0), g, text_color);
    let mut closed = false;
    if dismiss {
        let x = Rect::from_min_size(Pos2::new(r.max.x - 24.0, r.min.y + 2.0), Vec2::splat(22.0));
        let xr = clickable(ui, x, Id::new(("tasks-error-x", text.len())));
        text_in(
            ui.painter(),
            x,
            Align2::CENTER_CENTER,
            "×",
            theme::ui(15.0),
            if xr.hovered() { theme::pal().text } else { theme::pal().text_2 },
        );
        closed = tip(xr, t!("Close the notice")).clicked();
    }
    closed
}

/// Text field in a 1 px frame (`recess` background, `separator` border and `accent` on focus); the frame grows with the text.
fn text_field(ui: &mut Ui, id: Id, text: &mut String, hint: &str, rows: Option<usize>) -> egui::Response {
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    let p = theme::pal();
    egui::Frame::new()
        .fill(p.recess)
        .stroke(Stroke::new(1.0, if focused { theme::accent() } else { p.separator }))
        .corner_radius(6)
        .inner_margin(Margin { left: 9, right: 9, top: 6, bottom: 6 })
        .show(ui, |ui| {
            let te = match rows {
                Some(n) => egui::TextEdit::multiline(text).desired_rows(n),
                None => egui::TextEdit::singleline(text),
            };
            te.id(id)
                .frame(egui::Frame::NONE)
                .font(theme::ui(12.0))
                .text_color(p.text)
                .hint_text(egui::RichText::new(hint).font(theme::ui(12.0)).color(p.text_3))
                .desired_width(ui.available_width())
                .show(ui);
        })
        .response
}

/// Runs `f` in a column indented `PAD` on both sides and advances the `ui` cursor by what it occupied.
fn padded<R>(ui: &mut Ui, f: impl FnOnce(&mut Ui) -> R) -> R {
    let avail = ui.available_rect_before_wrap();
    let inner = Rect::from_min_max(Pos2::new(avail.min.x + PAD, avail.min.y), Pos2::new(avail.max.x - PAD, avail.max.y));
    let mut child = ui.new_child(UiBuilder::new().max_rect(inner).layout(egui::Layout::top_down(egui::Align::Min)));
    child.spacing_mut().item_spacing = Vec2::ZERO;
    let out = f(&mut child);
    let used = child.min_rect();
    ui.allocate_rect(Rect::from_min_max(Pos2::new(avail.min.x, used.min.y), Pos2::new(avail.max.x, used.max.y)), Sense::hover());
    out
}

/// The lane's header: its name as the app's section label, the count beside it and, for "Finished", the filter on the
/// right (returns its rectangle).
fn lane_header(deck: &Deck, ui: &mut Ui, lane: Lane, count: usize, acts: &mut Vec<Act>) -> Option<Rect> {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), Sense::hover());
    let p = theme::pal();
    let painter = ui.painter().clone();
    let row = Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y + 10.0), Pos2::new(r.max.x - LIST_PAD, r.max.y - 2.0));
    let g = galley_spaced(&painter, &lane.title().to_uppercase(), theme::label(10.0), p.text_3, 10.0 * theme::LABEL_SPACING_EM);
    let t = place(&painter, row, Align2::LEFT_CENTER, g, p.text_3);
    text_in(
        &painter,
        Rect::from_min_max(Pos2::new(t.max.x + 8.0, row.min.y), row.max),
        Align2::LEFT_CENTER,
        &count.to_string(),
        theme::meta(10.0),
        p.text_faint,
    );
    if lane != Lane::Finished {
        return None;
    }
    let filter_text = format!("{} ▾", deck.tasks.history_filter.label());
    let fw = text_w(&painter, &filter_text, theme::ui(11.0)) + 12.0;
    let f = Rect::from_min_size(Pos2::new(row.max.x - fw, row.center().y - 10.0), Vec2::new(fw, 20.0));
    let resp = clickable(ui, f, Id::new("tasks-history-filter"));
    let hot = resp.hovered() || deck.tasks.history_menu;
    if hot {
        painter.rect_filled(f, 4.0, p.hover);
    }
    text_in(&painter, f, Align2::CENTER_CENTER, &filter_text, theme::ui(11.0), if hot { p.text } else { p.text_2 });
    if tip(resp, t!("Filter the finished tasks")).clicked() {
        acts.push(Act::ToggleHistoryMenu);
    }
    Some(f)
}

// ---------------------------------------------------------------------------
// List
// ---------------------------------------------------------------------------

fn list_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, now: u64, acts: &mut Vec<Act>) {
    // Fixed footer at the bottom: separator + primary "+ New task" button, full width.
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, (rect.max.y - FOOTER_H).max(rect.min.y)), rect.max);
    let body = Rect::from_min_max(rect.min, Pos2::new(rect.max.x, footer.min.y));

    if deck.tasks.tasks.is_empty() {
        // With no open project nobody reads the list: it does not stay on "Reading the tasks…" forever.
        empty_state(ui, body, deck.tasks.loaded || deck.active_project.is_empty());
        draw_footer(ui, footer, acts);
        return;
    }

    let tasks = deck.tasks.tasks.clone();
    let filter = deck.tasks.history_filter;
    let lanes = lanes(&tasks, filter);
    // The oldest task waiting for you gets the accent frame: it is the next one to handle.
    let next_up = lanes.iter().find(|(l, _)| *l == Lane::NeedsYou).and_then(|(_, list)| list.first()).map(|t| t.id.clone());
    let mut filter_anchor = None;

    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("tasks-list").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(2.0);
        for (lane, list) in &lanes {
            let count = list.len();
            if let Some(f) = lane_header(deck, ui, *lane, count, acts) {
                filter_anchor = Some(f);
            }
            if *lane == Lane::Finished {
                if list.is_empty() {
                    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), DONE_ROW_H), Sense::hover());
                    text_in(
                        ui.painter(),
                        Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y), r.max),
                        Align2::LEFT_CENTER,
                        t!("No tasks in this filter."),
                        theme::ui(11.5),
                        theme::pal().text_3,
                    );
                }
                for task in list {
                    done_row(deck, ui, task, acts);
                }
            } else {
                for (i, task) in list.iter().enumerate() {
                    if i > 0 {
                        ui.add_space(2.0);
                    }
                    task_row(deck, ui, task, next_up.as_deref() == Some(task.id.as_str()), now, acts);
                }
            }
        }
        ui.add_space(LIST_PAD + 2.0);
    });

    draw_footer(ui, footer, acts);

    // Drop-down menu of the "Finished" filter.
    if deck.tasks.history_menu {
        if let Some(anchor) = filter_anchor {
            let items: Vec<MenuItem> = HistoryFilter::ALL
                .iter()
                .map(|&f| MenuItem {
                    label: f.label().to_owned(),
                    dot: None,
                    selected: f == filter,
                    enabled: true,
                    detail: None,
                    submenu: false,
                })
                .collect();
            let menu_w = 186.0_f32.max(anchor.width());
            let menu_anchor = Rect::from_min_size(Pos2::new(anchor.max.x - menu_w, anchor.min.y), Vec2::new(menu_w, anchor.height()));
            if let Some(i) = dropdown_menu(ui.ctx(), "tasks-history-filter-menu", menu_anchor, &items, acts, true) {
                acts.push(Act::SetHistoryFilter(HistoryFilter::ALL[i]));
            }
        }
    }
}

fn draw_footer(ui: &Ui, footer: Rect, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(footer);
    line_sep(&painter, Pos2::new(footer.min.x, footer.min.y + 0.5), Pos2::new(footer.max.x, footer.min.y + 0.5));
    let btn_rect =
        Rect::from_min_size(Pos2::new(footer.min.x + LIST_PAD, footer.min.y + 8.5), Vec2::new(footer.width() - 2.0 * LIST_PAD, BTN_H));
    if tip(ui_button(ui, btn_rect, Id::new("tasks-new"), t!("+ New task"), ButtonKind::Primary), t!("Create a task for an agent")).clicked()
    {
        acts.push(Act::New);
    }
}

fn empty_state(ui: &mut Ui, body: Rect, loaded: bool) {
    let painter = ui.painter_at(body);
    let w = (body.width() - 2.0 * PAD - 8.0).max(80.0);
    let mut y = body.min.y + 24.0;
    let x = body.min.x + PAD + 4.0;
    if !loaded {
        text_in(
            &painter,
            Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 20.0)),
            Align2::LEFT_CENTER,
            t!("Reading the tasks…"),
            theme::ui(12.0),
            theme::pal().text_2,
        );
        return;
    }
    let title = painter.layout(t!("No tasks yet").to_owned(), theme::ui_semibold(13.0), theme::pal().text_strong, w);
    painter.galley(Pos2::new(x, y), title.clone(), theme::pal().text_strong);
    y += title.size().y + 6.0;
    let text = t!(
        "A task is a request that an agent carries out on an isolated copy of the project (a branch with its own folder). \
When it finishes, you review the report and the diff, merge, test and validate. If something fails, you send it back or reject it with the reason."
    );
    let g = painter.layout(text.to_owned(), theme::ui(12.0), theme::pal().text_2, w);
    painter.galley(Pos2::new(x, y), g, theme::pal().text_2);
}

/// The note under a row and its colour: why a merge stopped, the failure, a conflict being resolved.
fn row_note(task: &Task) -> Option<(String, Color32)> {
    let p = theme::pal();
    match mark_of(task) {
        Mark::Stopped => task.merge_stopped.as_ref().map(|s| (s.reason.clone(), p.warning)),
        Mark::Failed => task.error.clone().map(|e| (e, p.danger_text)),
        Mark::Conflict => Some((t!("Merges into {base} by itself when the agent delivers.", base = task.base), p.text_2)),
        _ => None,
    }
}

/// The actions on a row that needs you: the main move and, at most, one quiet look (the diff). What needs typing (Return,
/// Reject) or is rarely wanted from the list (Discard) lives in the detail, so a row never becomes a wall of buttons.
fn row_actions(task: &Task) -> Vec<(String, Act, ButtonKind, &'static str)> {
    let id = || task.id.clone();
    let diff = |label: &str| (label.to_owned(), Act::OpenDiff(id()), ButtonKind::Ghost, tasks_ui::diff_button_tip(task));
    let mut v = match mark_of(task) {
        Mark::Review => vec![(TaskAct::Merge.label(task), Act::Task(id(), TaskAct::Merge), ButtonKind::Primary, TaskAct::Merge.tip())],
        Mark::Validate => {
            vec![(t!("Validate").to_owned(), Act::Task(id(), TaskAct::Validate), ButtonKind::Primary, TaskAct::Validate.tip())]
        }
        Mark::Stopped if task.merge_stopped.as_ref().is_some_and(|s| s.conflict_files.is_some()) => vec![(
            t!("Ask agent to resolve").to_owned(),
            Act::AskAgent(id()),
            ButtonKind::Primary,
            t!("Sends the task back to the agent asking it to merge the base into its branch (git merge) and resolve the conflicting files"),
        )],
        Mark::Failed => vec![(TaskAct::Retry.label(task), Act::Task(id(), TaskAct::Retry), ButtonKind::Primary, TaskAct::Retry.tip())],
        _ => Vec::new(),
    };
    if matches!(mark_of(task), Mark::Review | Mark::Validate) && can_show_diff(task) {
        v.push(diff(if task.status == TaskStatus::Validating { t!("What changed") } else { t!("Diff") }));
    }
    v
}

/// A row of the queue:
/// - line 1: the state glyph, the title (12.5 px 600, up to 2 lines) and how long it waits (10.5 px, `text_3`);
/// - line 2: the state word in its colour, the agent's logo with the model (and effort), the id, and the diff on the right;
/// - optional note (why the merge stopped, the failure, the conflict being resolved, or the agent's last move);
/// - for what needs you, its actions (24 px).
fn task_row(deck: &mut Deck, ui: &mut Ui, task: &Task, next_up: bool, now: u64, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let mark = mark_of(task);
    let needs_you = Lane::of(task.status) == Lane::NeedsYou;
    let busy = deck.tasks.busy.as_ref().filter(|b| b.id == task.id).map(|b| b.label);
    let working = deck.tasks.busy.is_some();
    let avail_w = ui.available_width();
    let row_w = (avail_w - 2.0 * LIST_PAD).max(120.0);
    let x_text = LIST_PAD + 6.0 + GLYPH_W + 8.0;
    let inner_w = (avail_w - x_text - LIST_PAD - 8.0).max(60.0);
    let painter = ui.painter().clone();

    let age = age_short(now, waiting_since(task));
    let age_g = galley(&painter, &age, theme::ui(10.5), p.text_3);
    let mut title_job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping {
            max_width: (inner_w - age_g.size().x - 8.0).max(40.0),
            max_rows: 2,
            break_anywhere: false,
            overflow_character: Some('…'),
        },
        ..Default::default()
    };
    title_job.append(
        &task.title,
        0.0,
        egui::TextFormat { font_id: theme::ui_semibold(12.5), color: p.text, line_height: Some(16.0), ..Default::default() },
    );
    let title_g = painter.layout_job(title_job);
    let title_h = title_g.size().y.max(16.0);

    let note = row_note(task).or_else(|| {
        (mark == Mark::Running)
            .then(|| tasks_ui::last_move(task))
            .flatten()
            .map(|(text, at)| (t!("Last: {what} · {age}", what = text, age = age_short(now, at)), p.text_3))
    });
    let note_g = note.as_ref().map(|(text, color)| {
        let mut job = egui::text::LayoutJob {
            wrap: egui::text::TextWrapping { max_width: inner_w, max_rows: 2, break_anywhere: false, overflow_character: Some('…') },
            ..Default::default()
        };
        job.append(text, 0.0, egui::TextFormat { font_id: theme::ui(11.5), color: *color, ..Default::default() });
        painter.layout_job(job)
    });
    let conflict_link = mark == Mark::Conflict;
    let actions = if needs_you { row_actions(task) } else { Vec::new() };

    let meta_h = 15.0;
    let note_h = note_g.as_ref().map_or(0.0, |g| 5.0 + g.size().y) + if conflict_link { 17.0 } else { 0.0 };
    let acts_h = if actions.is_empty() { 0.0 } else { 8.0 + ROW_BTN_H };
    let h = 8.0 + title_h + 3.0 + meta_h + note_h + acts_h + 8.0;

    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail_w, h), Sense::hover());
    let rect = Rect::from_min_size(Pos2::new(outer.min.x + LIST_PAD, outer.min.y), Vec2::new(row_w, h));
    let resp = clickable(ui, rect, Id::new(("task-row", &task.id)));
    let hot = resp.hovered();
    if next_up {
        frame(&painter, rect, 7.0, theme::accent_wash(theme::accent()), theme::accent_edge(theme::accent()));
    } else if hot {
        painter.rect_filled(rect, 7.0, p.hover);
    } else if needs_you {
        painter.rect_filled(rect, 7.0, theme::with_alpha(p.text_strong, 0.035));
    }

    let x0 = outer.min.x + x_text;
    let right = rect.max.x - 8.0;
    let y1 = rect.min.y + 8.0;
    paint_mark(ui, Pos2::new(rect.min.x + 6.0 + GLYPH_W / 2.0, y1 + 8.0), mark, deck.agents.tab_color(&task.kind));
    painter.galley(Pos2::new(x0, y1), title_g, p.text);
    place(&painter, Rect::from_min_max(Pos2::new(x0, y1), Pos2::new(right, y1 + 16.0)), Align2::RIGHT_CENTER, age_g, p.text_3);

    // Line 2: state · logo model · id ……… diff
    let y2 = y1 + title_h + 3.0;
    let line = Rect::from_min_max(Pos2::new(x0, y2), Pos2::new(right, y2 + meta_h));
    let mut diff_left = right;
    if let Some(sum) = task.diff_stat.as_deref().and_then(parse_diff_stat).filter(|s| s.files > 0) {
        if mark != Mark::Validate {
            diff_left = paint_diff_counts(&painter, line, sum.insertions, sum.deletions) - 8.0;
        }
    }
    if mark == Mark::Validate {
        if let Some(h) = tasks_ui::merge_short(task) {
            let text = t!("in {base} · {hash}", base = task.base, hash = h);
            let g = galley(&painter, &text, theme::meta(10.5), p.text_3);
            diff_left = place(&painter, line, Align2::RIGHT_CENTER, g, p.text_3).min.x - 8.0;
        }
    }
    let state = busy.unwrap_or_else(|| state_word(task));
    let state_color = if busy.is_some() { theme::accent() } else { mark_color(mark) };
    let g = galley_trunc(&painter, state, theme::ui_semibold(11.0), state_color, (diff_left - x0).max(10.0));
    let mut x = place(&painter, Rect::from_min_max(line.min, Pos2::new(diff_left, line.max.y)), Align2::LEFT_CENTER, g, state_color).max.x;
    if let (TaskStatus::Queued, Some(holder)) = (task.status, task.waiting_for.as_deref()) {
        let text = t!("after {holder}", holder);
        let g = galley_trunc(&painter, &text, theme::ui(11.0), p.text_3, (diff_left - x - 6.0).max(0.0));
        x = place(
            &painter,
            Rect::from_min_max(Pos2::new(x + 6.0, line.min.y), Pos2::new(diff_left, line.max.y)),
            Align2::LEFT_CENTER,
            g,
            p.text_3,
        )
        .max
        .x;
    }
    if diff_left - x > 40.0 {
        paint_agent(deck, &painter, Pos2::new(x + 8.0 + 6.0, line.center().y), &task.kind);
        x += 8.0 + 12.0 + 5.0;
        let model = task.model.as_deref().map(super::projects::model_tag).unwrap_or_else(|| t!("default model").to_owned());
        let who = match tasks_ui::effort_text(task) {
            Some(e) => format!("{model} · {e}"),
            None => model,
        };
        let g = galley_trunc(&painter, &who, theme::ui(11.0), p.text_2, (diff_left - x).max(0.0));
        x = place(
            &painter,
            Rect::from_min_max(Pos2::new(x, line.min.y), Pos2::new(diff_left, line.max.y)),
            Align2::LEFT_CENTER,
            g,
            p.text_2,
        )
        .max
        .x;
    }
    if diff_left - x > 24.0 {
        let g = galley(&painter, &task.id, theme::meta(10.5), p.text_3);
        if g.size().x + 6.0 <= diff_left - x {
            place(
                &painter,
                Rect::from_min_max(Pos2::new(x + 6.0, line.min.y), Pos2::new(diff_left, line.max.y)),
                Align2::LEFT_CENTER,
                g,
                p.text_3,
            );
        }
    }

    // Note and the conflict's link.
    let mut y = y2 + meta_h;
    let mut clicked_inside = false;
    if let (Some(g), Some((_, color))) = (note_g, note.as_ref()) {
        y += 5.0;
        let gh = g.size().y;
        painter.galley(Pos2::new(x0, y), g, *color);
        y += gh;
    }
    if conflict_link {
        let label = t!("Cancel automatic merge");
        let w = text_w(&painter, label, theme::ui_medium(11.5));
        let link = Rect::from_min_size(Pos2::new(x0, y + 2.0), Vec2::new(w, 15.0));
        let lr = if working {
            ui.interact(link, Id::new(("row-cancel-auto", &task.id)), Sense::hover())
        } else {
            clickable(ui, link, Id::new(("row-cancel-auto", &task.id)))
        };
        let color = if lr.hovered() && !working { p.text_strong } else { p.text };
        text_in(&painter, link, Align2::LEFT_CENTER, label, theme::ui_medium(11.5), color);
        painter.line_segment(
            [Pos2::new(link.min.x, link.max.y - 1.0), Pos2::new(link.max.x, link.max.y - 1.0)],
            Stroke::new(1.0, theme::with_alpha(color, 0.35)),
        );
        if tip(lr, t!("The task will wait in review when the agent delivers, and you merge it yourself")).clicked() && !working {
            acts.push(Act::CancelAutoMerge(task.id.clone()));
            clicked_inside = true;
        }
        y += 17.0;
    }

    // Actions.
    if !actions.is_empty() {
        let by = y + 8.0;
        let mut bx = x0;
        for (i, (label, act, kind, tooltip)) in actions.into_iter().enumerate() {
            let w = button_width(&painter, &label, kind);
            if i > 0 && bx + w > right + 2.0 {
                break;
            }
            let r = Rect::from_min_size(Pos2::new(bx, by), Vec2::new(w, ROW_BTN_H));
            bx = r.max.x + 6.0;
            let id = Id::new(("row-act", &task.id, i));
            if working {
                // While an operation runs the row's actions are off (dimmed, no click).
                ui.interact(r, id, Sense::hover());
                frame(&painter, r, 5.0, theme::with_alpha(p.text_strong, 0.02), p.separator);
                text_in(&painter, r, Align2::CENTER_CENTER, &label, theme::ui(11.5), p.text_3);
                continue;
            }
            if tip(ui_button(ui, r, id, &label, kind), tooltip).clicked() {
                acts.push(act);
                clicked_inside = true;
            }
        }
    }

    let tooltip = format!("{} · {}\n{} → {}", task.id, task.title, task.branch, task.base);
    if !clicked_inside && tip(resp, tooltip).clicked() {
        acts.push(Act::Open(task.id.clone()));
    }
}

/// A 24 px row of "Finished": the glyph (check or faint ring), the id, the title (struck through when discarded) and
/// the `dd/mm` date.
fn done_row(deck: &mut Deck, ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let avail_w = ui.available_width();
    let (outer, _) = ui.allocate_exact_size(Vec2::new(avail_w, DONE_ROW_H), Sense::hover());
    let row = outer.shrink2(Vec2::new(LIST_PAD, 0.0));
    let resp = clickable(ui, row, Id::new(("task-hist", &task.id)));
    if resp.hovered() {
        ui.painter().rect_filled(row, 5.0, p.hover);
    }
    let painter = ui.painter().clone();
    paint_mark(ui, Pos2::new(row.min.x + 6.0 + GLYPH_W / 2.0, row.center().y), mark_of(task), deck.agents.tab_color(&task.kind));
    let x_text = outer.min.x + LIST_PAD + 6.0 + GLYPH_W + 8.0;
    let id_g = galley(&painter, &task.id, theme::meta(10.5), p.text_3);
    let id_r = place(&painter, Rect::from_min_max(Pos2::new(x_text, row.min.y), row.max), Align2::LEFT_CENTER, id_g, p.text_3);
    let date_g = galley(&painter, &task_date_ddmm(task), theme::meta(10.0), p.text_3);
    let date_r =
        place(&painter, Rect::from_min_max(row.min, Pos2::new(row.max.x - 8.0, row.max.y)), Align2::RIGHT_CENTER, date_g, p.text_3);
    let title_x0 = id_r.max.x.max(x_text + 24.0) + 6.0;
    let title_w = (date_r.min.x - 8.0 - title_x0).max(20.0);
    let discarded = task.status == TaskStatus::Discarded;
    let title_color = if discarded { p.text_3 } else { p.text };
    let mut job = egui::text::LayoutJob {
        wrap: egui::text::TextWrapping { max_width: title_w, max_rows: 1, break_anywhere: true, overflow_character: Some('…') },
        ..Default::default()
    };
    job.append(
        &task.title,
        0.0,
        egui::TextFormat {
            font_id: theme::ui(11.5),
            color: title_color,
            strikethrough: if discarded { Stroke::new(1.0, p.text_3) } else { Stroke::NONE },
            ..Default::default()
        },
    );
    let title_g = painter.layout_job(job);
    place(
        &painter,
        Rect::from_min_size(Pos2::new(title_x0, row.min.y), Vec2::new(title_w, DONE_ROW_H)),
        Align2::LEFT_CENTER,
        title_g,
        title_color,
    );
    let tooltip = format!("{} · {}\n{}", task.id, task.title, status_name(task.status));
    if tip(resp, tooltip).clicked() {
        acts.push(Act::Open(task.id.clone()));
    }
}

// ---------------------------------------------------------------------------
// Detail
// ---------------------------------------------------------------------------

/// The pinned footer's buttons for the task's state: the main move first. Discard and the tab go to the "⋯" menu.
fn footer_actions(task: &Task, tab_open: bool) -> Vec<(String, Act, BtnKind, &'static str)> {
    let id = || task.id.clone();
    let tab = if tab_open { TaskAct::GoToTab } else { TaskAct::ReopenTab };
    let act = |a: TaskAct, kind: BtnKind| (a.label(task), Act::Task(id(), a), kind, a.tip());
    let diff = || (tasks_ui::diff_button_label(task).to_owned(), Act::OpenDiff(id()), BtnKind::Ghost, tasks_ui::diff_button_tip(task));
    let mut v = match mark_of(task) {
        Mark::Draft => vec![act(TaskAct::Start, BtnKind::Primary)],
        Mark::Queued => Vec::new(),
        Mark::Running | Mark::Conflict => vec![act(tab, BtnKind::Secondary)],
        Mark::Review => vec![
            act(TaskAct::Merge, BtnKind::Primary),
            (t!("Return…").to_owned(), Act::Task(id(), TaskAct::Return), BtnKind::Secondary, TaskAct::Return.tip()),
        ],
        Mark::Stopped => vec![(t!("Return…").to_owned(), Act::Task(id(), TaskAct::Return), BtnKind::Secondary, TaskAct::Return.tip())],
        Mark::Validate => vec![
            act(TaskAct::Validate, BtnKind::Primary),
            (t!("Reject…").to_owned(), Act::Task(id(), TaskAct::Reject), BtnKind::Secondary, TaskAct::Reject.tip()),
        ],
        Mark::Failed if task.report.is_some() => {
            vec![
                act(TaskAct::Retry, BtnKind::Primary),
                (t!("Return…").to_owned(), Act::Task(id(), TaskAct::Return), BtnKind::Secondary, TaskAct::Return.tip()),
            ]
        }
        Mark::Failed => vec![act(TaskAct::Retry, BtnKind::Primary)],
        Mark::Done | Mark::Discarded => Vec::new(),
    };
    if can_show_diff(task) {
        v.push(diff());
    }
    v
}

/// The items of the "⋯" menu: (label, act, danger).
fn more_items(task: &Task, tab_open: bool) -> Vec<(String, Act, bool)> {
    let id = || task.id.clone();
    let all = actions_for(task, tab_open);
    let mut v = Vec::new();
    for a in [TaskAct::GoToTab, TaskAct::ReopenTab] {
        // Working, the tab is already a footer button.
        if all.contains(&a) && !matches!(task.status, TaskStatus::Running) {
            v.push((a.label(task), Act::Task(id(), a), false));
        }
    }
    if !task.brief.trim().is_empty() {
        v.push((t!("Open the request in Documents").to_owned(), Act::OpenText(id(), false), false));
    }
    if task.report.as_deref().is_some_and(|r| !r.trim().is_empty()) {
        v.push((t!("Open the report in Documents").to_owned(), Act::OpenText(id(), true), false));
    }
    if all.contains(&TaskAct::Discard) {
        v.push((t!("Discard…").to_owned(), Act::Task(id(), TaskAct::Discard), true));
    }
    v
}

fn detail_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, task: &Task, now: u64, acts: &mut Vec<Act>) {
    let tab_open = deck.tasks_tab_open(task);
    let more = more_items(task, tab_open);
    let (head, more_rect) = head_bar(ui, rect, Some(&task.id), &task.title, !more.is_empty(), acts);
    let more_open = ui.ctx().data(|d| d.get_temp::<bool>(more_id())).unwrap_or(false);
    if let Some(m) = more_rect {
        let r = clickable(ui, m, Id::new("tasks-more"));
        let p = theme::pal();
        if r.hovered() || more_open {
            ui.painter().rect_filled(m, 5.0, if more_open { p.selected } else { p.hover });
        }
        for dx in [-4.5, 0.0, 4.5] {
            ui.painter().circle_filled(m.center() + Vec2::new(dx, 0.0), 1.4, if r.hovered() || more_open { p.text } else { p.text_2 });
        }
        if tip(r, t!("More actions")).clicked() {
            acts.push(Act::ToggleMore);
        }
    }
    let busy = deck.tasks.busy.clone();
    let working = busy.is_some();
    let busy_here = busy.as_ref().filter(|b| b.id == task.id).map(|b| b.label);
    if busy_here.is_some() {
        super::git::busy_bar(ui, Rect::from_min_max(Pos2::new(rect.min.x, head.max.y - 2.0), Pos2::new(rect.max.x, head.max.y)));
    }
    let return_open = deck.tasks.return_open && matches!(task.status, TaskStatus::Review | TaskStatus::Validating | TaskStatus::Failed);
    let buttons = footer_actions(task, tab_open);

    // The pinned footer: the return/reject field and its buttons, or the state's actions (with the busy line).
    let footer_h = if return_open {
        return_form_height(ui, task)
    } else if buttons.is_empty() && busy_here.is_none() {
        0.0
    } else {
        FOOTER_H + if busy_here.is_some() { 22.0 } else { 0.0 }
    };
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - footer_h), rect.max);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), Pos2::new(rect.max.x, footer.min.y));

    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt(("tasks-detail", &task.id)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        detail_top(deck, ui, task, now);
        track(ui, task, now);
        ui.add_space(2.0);
        callouts(deck, ui, task, working, acts);
        ui.add_space(12.0);
        let tab = deck.tasks.detail_tab.unwrap_or_else(|| DetailTab::default_for(task));
        detail_tabs(ui, task, tab, acts);
        ui.add_space(10.0);
        match tab {
            DetailTab::Report => {
                report_pane(ui, task, acts);
                ui.add_space(14.0);
                pane_label(ui, t!("Changes"));
                changes_pane(ui, task, acts);
            }
            DetailTab::Timeline => timeline_pane(deck, ui, task, acts),
            DetailTab::Request => request_pane(ui, task, acts),
        }
        ui.add_space(14.0);
    });

    if footer_h > 0.0 {
        let painter = ui.painter_at(footer);
        line_sep(&painter, Pos2::new(footer.min.x, footer.min.y + 0.5), Pos2::new(footer.max.x, footer.min.y + 0.5));
        let mut fui =
            ui.new_child(UiBuilder::new().max_rect(footer.shrink2(Vec2::new(0.0, 8.0))).layout(egui::Layout::top_down(egui::Align::Min)));
        fui.spacing_mut().item_spacing = Vec2::ZERO;
        if return_open {
            return_form(deck, &mut fui, task, working, acts);
        } else {
            if let Some(label) = busy_here {
                let (r, _) = fui.allocate_exact_size(Vec2::new(fui.available_width(), 22.0), Sense::hover());
                egui::Spinner::new()
                    .size(14.0)
                    .color(theme::pal().text_2)
                    .paint_at(&fui, Rect::from_center_size(Pos2::new(r.min.x + PAD + 7.0, r.center().y), Vec2::splat(14.0)));
                text_in(
                    fui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + PAD + 22.0, r.min.y), r.max),
                    Align2::LEFT_CENTER,
                    label,
                    theme::ui(12.0),
                    theme::pal().text_2,
                );
            }
            let row: Vec<(String, BtnKind, bool, &str)> = buttons
                .iter()
                .map(|(label, act, kind, tooltip)| (label.clone(), *kind, !working || matches!(act, Act::OpenDiff(_)), *tooltip))
                .collect();
            if let Some(i) = button_flow(&mut fui, &row, "detail-footer") {
                if let Some((_, act, ..)) = buttons.into_iter().nth(i) {
                    acts.push(act);
                }
            }
        }
    }

    if more_open {
        if let Some(anchor) = more_rect {
            more_menu(ui.ctx(), anchor, &more, working, acts);
        }
    }
}

/// Title, the state sentence, the agent and the branch.
fn detail_top(deck: &mut Deck, ui: &mut Ui, task: &Task, now: u64) {
    let p = theme::pal();
    let mark = mark_of(task);
    ui.add_space(12.0);
    let w = ui.available_width() - 2.0 * PAD;
    let g = ui.painter().layout(task.title.clone(), theme::ui_semibold(15.0), p.text_strong, w);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 8.0), Sense::hover());
    ui.painter().galley(Pos2::new(r.min.x + PAD, r.min.y), g, p.text_strong);

    // ◉ Waiting for your review · ready 6 min ago
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    paint_mark(ui, Pos2::new(r.min.x + PAD + 8.0, r.center().y), mark, deck.agents.tab_color(&task.kind));
    let color = mark_color(mark);
    let g = galley(ui.painter(), state_sentence(task), theme::ui_semibold(12.0), color);
    let s = place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x + PAD + 24.0, r.min.y), r.max), Align2::LEFT_CENTER, g, color);
    let when = format!("· {}", status_age(task, now));
    let g = galley_trunc(ui.painter(), &when, theme::ui(12.0), p.text_2, (r.max.x - PAD - s.max.x - 6.0).max(10.0));
    place(
        ui.painter(),
        Rect::from_min_max(Pos2::new(s.max.x + 6.0, r.min.y), Pos2::new(r.max.x - PAD, r.max.y)),
        Align2::LEFT_CENTER,
        g,
        p.text_2,
    );
    ui.add_space(6.0);

    // Agent · model · effort, then the branch (mono) and the scope.
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    let painter = ui.painter().clone();
    paint_agent(deck, &painter, Pos2::new(r.min.x + PAD + 6.0, r.center().y), &task.kind);
    let mut who = deck.agents.name(&task.kind);
    if let Some(m) = tasks_ui::model_effort_text(task) {
        who.push_str(" · ");
        who.push_str(&m);
    }
    let g = galley_trunc(&painter, &who, theme::ui(11.5), p.text_2, (r.width() - 2.0 * PAD - 18.0).max(10.0));
    place(&painter, Rect::from_min_max(Pos2::new(r.min.x + PAD + 18.0, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text_2);
    let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
    deck.icons.paint(&painter, Icon::GitBranch, Pos2::new(r.min.x + PAD + 6.0, r.center().y), 12.0, p.text_3, 0.0);
    let g = galley_trunc(&painter, &tasks_ui::branch_line(task), theme::mono(11.0), p.text_2, (r.width() - 2.0 * PAD - 18.0).max(10.0));
    place(&painter, Rect::from_min_max(Pos2::new(r.min.x + PAD + 18.0, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text_2);
    tip(
        resp,
        format!(
            "{}\n{}",
            tasks_ui::branch_line(task),
            t!("Task branch → branch it will be merged into (and the merge commit, if there was one)")
        ),
    );
    if !task.scope.is_empty() {
        let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
        let text = t!("Scope: {scope}", scope = task.scope.join(", "));
        let g = galley_trunc(&painter, &text, theme::mono(11.0), p.text_3, (r.width() - 2.0 * PAD - 18.0).max(10.0));
        place(&painter, Rect::from_min_max(Pos2::new(r.min.x + PAD + 18.0, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text_3);
        tip(resp, task.scope.join("\n"));
    }
}

/// The lifecycle track: five stops on a line, the reached ones filled, the current one in its colour with a halo,
/// the label and the time under each.
fn track(ui: &mut Ui, task: &Task, now: u64) {
    let p = theme::pal();
    let stops = lifecycle(task, now);
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 52.0), Sense::hover());
    let painter = ui.painter();
    let inner = Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y + 12.0), Pos2::new(r.max.x - PAD, r.max.y));
    let col = inner.width() / 5.0;
    let cx = |i: usize| inner.min.x + col * (i as f32 + 0.5);
    let y = inner.min.y + 6.0;
    let last_reached = stops.iter().rposition(|s| s.state != StopState::Later).unwrap_or(0);
    painter.line_segment([Pos2::new(cx(0), y), Pos2::new(cx(4), y)], Stroke::new(2.0, p.separator));
    if last_reached > 0 {
        painter.line_segment([Pos2::new(cx(0), y), Pos2::new(cx(last_reached), y)], Stroke::new(2.0, theme::with_alpha(p.text_2, 0.7)));
    }
    for (i, s) in stops.iter().enumerate() {
        let c = Pos2::new(cx(i), y);
        let (fill, ring, halo) = match s.state {
            StopState::Done => (p.text_2, p.text_2, None),
            StopState::Now => (theme::accent(), theme::accent(), Some(theme::accent_soft(theme::accent()))),
            StopState::Warn => (p.warning, p.warning, Some(theme::with_alpha(p.warning, 0.22))),
            StopState::Fail => (p.danger, p.danger, Some(theme::with_alpha(p.danger, 0.22))),
            StopState::Later => (p.bg, p.separator, None),
        };
        if let Some(h) = halo {
            painter.circle_filled(c, 9.0, h);
        }
        painter.circle_filled(c, 6.0, ring);
        painter.circle_filled(c, 4.0, fill);
        let current = !matches!(s.state, StopState::Done | StopState::Later);
        let (font, color) = if current { (theme::ui_semibold(10.5), p.text) } else { (theme::ui(10.5), p.text_3) };
        let g = galley_trunc(painter, s.label, font, color, col - 2.0);
        place(painter, Rect::from_center_size(Pos2::new(c.x, y + 17.0), Vec2::new(col, 14.0)), Align2::CENTER_CENTER, g, color);
        let when = if s.when.is_empty() { "—" } else { s.when.as_str() };
        let g = galley_trunc(painter, when, theme::meta(10.0), p.text_faint, col - 2.0);
        place(painter, Rect::from_center_size(Pos2::new(c.x, y + 31.0), Vec2::new(col, 13.0)), Align2::CENTER_CENTER, g, p.text_faint);
    }
}

/// The explanations of the state, each where it belongs: the queue, the files outside the scope, the conflict being
/// resolved (with "Cancel automatic merge"), the merge that stopped (with its way out), the failure and the last
/// action's notice.
fn callouts(deck: &mut Deck, ui: &mut Ui, task: &Task, working: bool, acts: &mut Vec<Act>) {
    let notice = deck.tasks.notice.clone();
    let notice_info = deck.tasks.notice_info;
    let notice_block = deck.tasks.notice_block.clone();
    let notice_stash = deck.tasks.notice_stash.clone();
    let notice_files = deck.tasks.notice_files;
    if let (TaskStatus::Queued, Some(holder)) = (task.status, task.waiting_for.as_deref()) {
        let text = t!(
            "Queued: the scope overlaps task {holder}'s scope. It starts by itself when that task is merged, discarded or fails, branching from {base} at that moment.",
            holder,
            base = task.base
        );
        notice_box(ui, &text, false, false);
    }
    if task.status == TaskStatus::Review && !task.outside_scope.is_empty() {
        let shown: Vec<&str> = task.outside_scope.iter().take(OUTSIDE_SHOWN).map(String::as_str).collect();
        let more = task.outside_scope.len().saturating_sub(OUTSIDE_SHOWN);
        let files = shown.join(", ");
        let text = if more > 0 {
            t!("Touched outside the scope: {files} and {more} more", files, more)
        } else {
            t!("Touched outside the scope: {files}", files)
        };
        notice_box(ui, &text, false, false);
    }
    if task.status == TaskStatus::Running && task.merge_approved {
        let text = t!(
            "You approved the merge, but {base} moved and conflicts. The agent is resolving it; the task merges by itself when it delivers.",
            base = task.base
        );
        notice_box(ui, &text, false, false);
        let b = [(
            t!("Cancel automatic merge").to_owned(),
            BtnKind::Secondary,
            !working,
            t!("The task will wait in review when the agent delivers, and you merge it yourself"),
        )];
        if button_flow(ui, &b, "cancel-auto-merge").is_some() {
            acts.push(Act::CancelAutoMerge(task.id.clone()));
        }
    }
    if let Some(stop) = task.merge_stopped.as_ref().filter(|_| task.status == TaskStatus::Review && notice.is_none()) {
        let text = t!("The task was not merged by itself: {reason}", reason = &stop.reason);
        error_box(ui, &text, false);
        if let Some(files) = &stop.conflict_files {
            refusal_buttons(ui, task, &MergeBlock::Conflict(files.clone()), false, !working, acts);
        }
    }
    if let Some(err) = task.error.as_deref().filter(|_| task.status == TaskStatus::Failed) {
        error_box(ui, err, false);
    }
    if let Some(n) = &notice {
        // A merge refusal with a way out: the plain-language explanation in place of git's text, and the way-out button.
        // The stash notice goes along: it is the only sentence that says where the user's changes went.
        let shown = match (&notice_block, &notice_stash) {
            (Some(b), Some(note)) => format!("{} {note}", b.explain(task)),
            (Some(b), None) => b.explain(task),
            (None, _) => n.clone(),
        };
        if notice_box(ui, &shown, true, !notice_info) {
            acts.push(Act::DismissNotice);
        }
        if let Some(block) = &notice_block {
            refusal_buttons(ui, task, block, notice_files, !working, acts);
        }
    }
}

/// The count shown on a pane's tab (files, timeline items), when there is one.
fn tab_count(task: &Task, tab: DetailTab) -> Option<usize> {
    match tab {
        DetailTab::Timeline => Some(task.events.len() + task.messages.len()).filter(|n| *n > 0),
        DetailTab::Report | DetailTab::Request => None,
    }
}

/// The pane switch: a 26 px track (`recess`) with four segments, the label and its count.
fn detail_tabs(ui: &mut Ui, task: &Task, current: DetailTab, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
    let track = Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y), Pos2::new(r.max.x - PAD, r.max.y));
    let painter = ui.painter().clone();
    painter.rect_filled(track, 6.0, p.recess);
    let tabs = DetailTab::ALL;
    // Each segment is as wide as its text needs, the rest shared equally (the labels never cut in Portuguese).
    let widths: Vec<f32> = tabs
        .iter()
        .map(|t| {
            let label = text_w(&painter, t.label(), theme::ui_semibold(11.5));
            let count = tab_count(task, *t).map_or(0.0, |n| 4.0 + text_w(&painter, &n.to_string(), theme::meta(10.0)));
            label + count + 12.0
        })
        .collect();
    let spare = ((track.width() - 4.0 - 6.0) - widths.iter().sum::<f32>()).max(0.0) / tabs.len() as f32;
    let mut x = track.min.x + 2.0;
    for (i, tab) in tabs.iter().enumerate() {
        let seg = Rect::from_min_size(Pos2::new(x, track.min.y + 2.0), Vec2::new(widths[i] + spare, track.height() - 4.0));
        x = seg.max.x + 2.0;
        let on = *tab == current;
        let resp = clickable(ui, seg, Id::new(("tasks-pane", i)));
        if on {
            painter.rect_filled(seg, 5.0, p.selected);
        } else if resp.hovered() {
            painter.rect_filled(seg, 5.0, p.hover);
        }
        let (font, color) = if on { (theme::ui_semibold(11.5), p.text_strong) } else { (theme::ui(11.5), p.text_2) };
        let label = galley(&painter, tab.label(), font, color);
        let count = tab_count(task, *tab).map(|n| galley(&painter, &n.to_string(), theme::meta(10.0), p.text_3));
        let total = label.size().x + count.as_ref().map_or(0.0, |c| 4.0 + c.size().x);
        let lx = seg.center().x - total / 2.0;
        let lr = place(
            &painter,
            Rect::from_min_max(Pos2::new(lx, seg.min.y), Pos2::new(seg.max.x, seg.max.y)),
            Align2::LEFT_CENTER,
            label,
            color,
        );
        if let Some(c) = count {
            place(
                &painter,
                Rect::from_min_max(Pos2::new(lr.max.x + 4.0, seg.min.y), Pos2::new(seg.max.x, seg.max.y)),
                Align2::LEFT_CENTER,
                c,
                p.text_3,
            );
        }
        if resp.clicked() && !on {
            acts.push(Act::SetTab(*tab));
        }
    }
}

/// A label inside a pane (the app's section label), for the second block of the Report pane.
fn pane_label(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
    let p = theme::pal();
    let g = galley_spaced(ui.painter(), &text.to_uppercase(), theme::label(10.0), p.text_3, 10.0 * theme::LABEL_SPACING_EM);
    place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y), r.max), Align2::LEFT_CENTER, g, p.text_3);
}

/// "Open in Documents" (secondary), under the request and the report.
fn open_text_button(ui: &mut Ui, task: &Task, report: bool, acts: &mut Vec<Act>) {
    let tip = if report {
        t!("Opens the report formatted, with the full width of the center")
    } else {
        t!("Opens the request formatted, with the full width of the center")
    };
    let b = [(t!("Open in Documents").to_owned(), BtnKind::Secondary, true, tip)];
    ui.add_space(4.0);
    if button_flow(ui, &b, if report { "open-report" } else { "open-brief" }).is_some() {
        acts.push(Act::OpenText(task.id.clone(), report));
    }
}

fn report_pane(ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    match task.report.as_deref().filter(|r| !r.trim().is_empty()) {
        Some(report) => {
            paragraph(ui, &clip_text(report.trim(), TEXT_MAX), theme::ui(12.0), p.text);
            open_text_button(ui, task, true, acts);
        }
        None if matches!(task.status, TaskStatus::Running | TaskStatus::Queued | TaskStatus::Draft) => {
            muted_line(ui, t!("The agent has not delivered the report yet."))
        }
        None => muted_line(ui, t!("No report.")),
    }
}

fn request_pane(ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    if task.brief.trim().is_empty() {
        muted_line(ui, t!("No request besides the title: the agent gets only the title."));
    } else {
        paragraph(ui, &clip_text(task.brief.trim(), TEXT_MAX), theme::ui(12.0), p.text);
        open_text_button(ui, task, false, acts);
    }
}

/// The diff stat, one file per row: the path (cut from the start), the +/− split as a 44 px bar and the changed lines.
fn changes_pane(ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let summary = task.diff_stat.as_deref().and_then(parse_diff_stat);
    let lines = task.diff_stat.as_deref().map(tasks_ui::parse_stat_lines).unwrap_or_default();
    if lines.is_empty() {
        let text = match summary {
            Some(s) if s.files == 0 => t!("No changes against the base."),
            _ if can_show_diff(task) => t!("The summary arrives with the report."),
            _ => t!("No changes recorded."),
        };
        muted_line(ui, text);
    } else {
        let painter = ui.painter().clone();
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 18.0), Sense::hover());
        let line = Rect::from_min_max(Pos2::new(r.min.x + PAD + 6.0, r.min.y), Pos2::new(r.max.x - PAD - 6.0, r.max.y));
        let what = if tasks_ui::shows_merged_diff(task) {
            t!("what the merge brought into {base}", base = task.base)
        } else {
            t!("against {base}", base = task.base)
        };
        let left = summary.map_or(line.max.x, |s| paint_diff_counts(&painter, line, s.insertions, s.deletions) - 8.0);
        let g = galley_trunc(&painter, &what, theme::meta(10.5), p.text_3, (left - line.min.x).max(10.0));
        place(&painter, Rect::from_min_max(line.min, Pos2::new(left, line.max.y)), Align2::LEFT_CENTER, g, p.text_3);
        ui.add_space(2.0);
        let shown = lines.len().min(STAT_MAX_LINES);
        for l in lines.iter().take(shown) {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            let row = Rect::from_min_max(Pos2::new(r.min.x + PAD, r.min.y), Pos2::new(r.max.x - PAD, r.max.y));
            if resp.hovered() {
                painter.rect_filled(row, 5.0, p.hover);
            }
            let count = galley(&painter, &l.changes, theme::meta(10.5), p.text_3);
            let cw = count.size().x.max(24.0);
            let cr = place(
                &painter,
                Rect::from_min_max(Pos2::new(row.max.x - 6.0 - cw, row.min.y), Pos2::new(row.max.x - 6.0, row.max.y)),
                Align2::RIGHT_CENTER,
                count,
                p.text_3,
            );
            let bar =
                Rect::from_center_size(Pos2::new(cr.min.x.min(row.max.x - 6.0 - cw) - 8.0 - 22.0, row.center().y), Vec2::new(44.0, 4.0));
            match l.plus {
                Some(share) => {
                    painter.rect_filled(bar, 2.0, p.danger);
                    if share > 0.0 {
                        let plus = Rect::from_min_size(bar.min, Vec2::new(bar.width() * share, bar.height()));
                        painter.rect_filled(plus, 2.0, p.success);
                    }
                }
                None => {
                    painter.rect_filled(bar, 2.0, p.separator);
                }
            }
            let g = tail_galley(&painter, &l.path, theme::mono(11.0), p.text_2, (bar.min.x - 8.0 - row.min.x - 6.0).max(20.0));
            place(
                &painter,
                Rect::from_min_max(Pos2::new(row.min.x + 6.0, row.min.y), Pos2::new(bar.min.x - 8.0, row.max.y)),
                Align2::LEFT_CENTER,
                g,
                p.text_2,
            );
            tip(resp, l.path.clone());
        }
        let extra = lines.len() - shown;
        if extra > 0 {
            let more = if extra == 1 { t!("… and 1 more file").to_owned() } else { t!("… and {extra} more files", extra) };
            muted_line(ui, &more);
        }
    }
    if can_show_diff(task) {
        ui.add_space(6.0);
        let b = [(tasks_ui::diff_button_label(task).to_owned(), BtnKind::Secondary, true, tasks_ui::diff_button_tip(task))];
        if button_flow(ui, &b, "diff").is_some() {
            acts.push(Act::OpenDiff(task.id.clone()));
        }
    }
}

/// Cuts the start of the text (`…ui/tasks.rs`) to fit `max_w`: in a path, the end is what identifies it.
fn tail_galley(painter: &egui::Painter, text: &str, font: egui::FontId, color: Color32, max_w: f32) -> std::sync::Arc<egui::Galley> {
    let full = galley(painter, text, font.clone(), color);
    if full.size().x <= max_w {
        return full;
    }
    let chars: Vec<char> = text.chars().collect();
    let (mut lo, mut hi) = (0, chars.len());
    while lo < hi {
        let mid = (lo + hi) / 2;
        let cut: String = std::iter::once('…').chain(chars[mid..].iter().copied()).collect();
        if galley(painter, &cut, font.clone(), color).size().x <= max_w {
            hi = mid;
        } else {
            lo = mid + 1;
        }
    }
    let cut: String = std::iter::once('…').chain(chars[lo..].iter().copied()).collect();
    galley(painter, &cut, font, color)
}

/// Events and messages in one stream, newest first, along a vertical guide: an event is a hollow dot with its time and
/// text; a message is a dot in the speaker's colour (red for an error), `from → to` and the text in a bubble, collapsed
/// to its first lines (a click opens it whole).
fn timeline_pane(deck: &mut Deck, ui: &mut Ui, task: &Task, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let moments = tasks_ui::timeline(task);
    if moments.is_empty() {
        muted_line(ui, t!("Nothing recorded yet."));
        return;
    }
    let guide_x = ui.max_rect().min.x + PAD + 4.0;
    let text_x = guide_x + 13.0;
    let text_w_max = (ui.max_rect().max.x - PAD - text_x).max(60.0);
    let top = ui.cursor().min.y;
    let mut last_dot = top;
    let painter = ui.painter().clone();
    // The guide goes behind the dots: its slot is reserved first and filled at the end.
    let guide = painter.add(Shape::Noop);
    let time_w = text_w(&painter, "00/00 00:00", theme::meta(10.5));
    for m in moments.iter().take(MOMENTS_MAX) {
        let when = tasks_ui::event_time(m.at_ms());
        match m {
            Moment::Event(e) => {
                let g = painter.layout(
                    clip_text(&e.text, 1_000).into_owned(),
                    theme::ui(11.5),
                    p.text_2,
                    (text_w_max - time_w - 8.0).max(40.0),
                );
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y + 8.0), Sense::hover());
                let first = g.rows.first().map_or(15.0, |row| row.height());
                let c = Pos2::new(guide_x, r.min.y + 2.0 + first / 2.0);
                painter.circle_filled(c, 4.0, theme::pal().bg);
                painter.circle_stroke(c, 3.5, Stroke::new(1.4, p.text_3));
                last_dot = c.y;
                let t = galley(&painter, &when, theme::meta(10.5), p.text_faint);
                place(
                    &painter,
                    Rect::from_min_size(Pos2::new(text_x, r.min.y + 2.0), Vec2::new(time_w, first)),
                    Align2::LEFT_CENTER,
                    t,
                    p.text_faint,
                );
                painter.galley(Pos2::new(text_x + time_w + 8.0, r.min.y + 2.0), g, p.text_2);
            }
            Moment::Message(msg) => {
                let open = deck.tasks.open_messages.contains(&(task.id.clone(), msg.at_ms));
                let (preview, more) = tasks_ui::message_preview(&msg.text);
                let shown = if open { clip_text(msg.text.trim(), TEXT_MAX).into_owned() } else { preview };
                let error = msg.kind == FeedKind::Error;
                let body_color = if error { p.danger_text } else { p.text_2 };
                let body = painter.layout(shown, theme::ui(12.0), body_color, text_w_max - 18.0);
                let head_h = 17.0;
                let foot_h = if more { 15.0 } else { 0.0 };
                let bubble_h = 7.0 + body.size().y + foot_h + 7.0;
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), head_h + 3.0 + bubble_h + 8.0), Sense::click());
                let from = msg.from.as_deref().unwrap_or("…");
                let kind = from.split('@').next().unwrap_or(from);
                let dot_color = if error { p.danger } else { deck.agents.tab_color(kind) };
                let c = Pos2::new(guide_x, r.min.y + head_h / 2.0);
                painter.circle_filled(c, 4.0, dot_color);
                last_dot = c.y;
                // time · from → to · kind
                let t = galley(&painter, &when, theme::meta(10.5), p.text_faint);
                let tr = place(
                    &painter,
                    Rect::from_min_size(Pos2::new(text_x, r.min.y), Vec2::new(time_w, head_h)),
                    Align2::LEFT_CENTER,
                    t,
                    p.text_faint,
                );
                let mut job = egui::text::LayoutJob {
                    wrap: egui::text::TextWrapping {
                        max_width: (r.max.x - PAD - tr.max.x - 8.0).max(20.0),
                        max_rows: 1,
                        break_anywhere: true,
                        overflow_character: Some('…'),
                    },
                    ..Default::default()
                };
                job.append(from, 0.0, egui::TextFormat { font_id: theme::ui_semibold(11.0), color: p.text, ..Default::default() });
                job.append(
                    &format!(" → {}", msg.to.as_deref().unwrap_or("…")),
                    0.0,
                    egui::TextFormat { font_id: theme::ui(11.0), color: p.text_3, ..Default::default() },
                );
                job.append(
                    &format!("  {}", tasks_ui::message_kind_label(msg.kind)),
                    0.0,
                    egui::TextFormat {
                        font_id: theme::ui(11.0),
                        color: if error { p.danger_text } else { p.text_3 },
                        ..Default::default()
                    },
                );
                let hg = painter.layout_job(job);
                place(
                    &painter,
                    Rect::from_min_max(Pos2::new(tr.max.x + 8.0, r.min.y), Pos2::new(r.max.x - PAD, r.min.y + head_h)),
                    Align2::LEFT_CENTER,
                    hg,
                    p.text,
                );
                let bubble = Rect::from_min_size(Pos2::new(text_x, r.min.y + head_h + 3.0), Vec2::new(text_w_max, bubble_h));
                let border = if error { theme::with_alpha(p.danger, 0.3) } else { theme::with_alpha(p.text_strong, 0.06) };
                let fill = if more && resp.hovered() { p.hover } else { theme::with_alpha(p.text_strong, 0.035) };
                frame(&painter, bubble, 7.0, fill, border);
                painter.galley(Pos2::new(bubble.min.x + 9.0, bubble.min.y + 7.0), body.clone(), body_color);
                if more {
                    text_in(
                        &painter,
                        Rect::from_min_size(
                            Pos2::new(bubble.min.x + 9.0, bubble.min.y + 7.0 + body.size().y),
                            Vec2::new(bubble.width() - 18.0, foot_h),
                        ),
                        Align2::LEFT_CENTER,
                        if open { t!("collapse") } else { t!("see all") },
                        theme::meta(10.5),
                        p.text_3,
                    );
                    if resp.hovered() {
                        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                    }
                    if tip(resp.clone(), if open { t!("Collapses the message") } else { t!("Opens the whole message") }).clicked() {
                        acts.push(Act::ToggleMessage(task.id.clone(), msg.at_ms));
                    }
                }
            }
        }
    }
    // The guide, from the first dot to the last, behind them.
    painter.set(guide, Shape::line_segment([Pos2::new(guide_x, top + 6.0), Pos2::new(guide_x, last_dot)], Stroke::new(1.0, p.separator)));
    if moments.len() > MOMENTS_MAX {
        let older = moments.len() - MOMENTS_MAX;
        let text = if older == 1 { t!("… and 1 older item.").to_owned() } else { t!("… and {older} older items.", older) };
        muted_line(ui, &text);
    }
}

/// The "⋯" menu of the detail, under its button.
fn more_menu(ctx: &egui::Context, anchor: Rect, items: &[(String, Act, bool)], working: bool, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let row = 26.0;
    let w = 230.0;
    let h = items.len() as f32 * row + 8.0;
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.max.x - w, anchor.max.y + 2.0), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    Area::new(Id::new("tasks-more-menu")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, p.shadow);
        frame(ui.painter(), rect, 8.0, p.popup, p.glass_border);
        for (i, (label, _, danger)) in items.iter().enumerate() {
            let r = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 4.0 + i as f32 * row), Vec2::new(w - 8.0, row));
            let enabled = !working || !danger;
            let resp = if enabled {
                clickable(ui, r, Id::new(("tasks-more-item", i)))
            } else {
                ui.interact(r, Id::new(("tasks-more-item", i)), Sense::hover())
            };
            if enabled && resp.hovered() {
                ui.painter().rect_filled(r, 5.0, p.hover);
            }
            let color = if !enabled {
                p.text_3
            } else if *danger {
                p.danger_text
            } else if resp.hovered() {
                p.text
            } else {
                p.text_2
            };
            let g = galley_trunc(ui.painter(), label, theme::ui(12.0), color, r.width() - 20.0);
            place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), r.max), Align2::LEFT_CENTER, g, color);
            if enabled && resp.clicked() {
                chosen = Some(i);
            }
        }
    });
    match chosen {
        Some(i) => {
            acts.push(Act::CloseMore);
            if let Some((_, act, _)) = items.get(i) {
                acts.push(match act {
                    Act::Task(id, a) => Act::Task(id.clone(), *a),
                    Act::OpenText(id, r) => Act::OpenText(id.clone(), *r),
                    _ => Act::CloseMore,
                });
            }
        }
        None if ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor) => {
            acts.push(Act::CloseMore)
        }
        None => {}
    }
}

/// The buttons of the way out of the merge refusal (and, for what is uncommitted, the full list of files).
fn refusal_buttons(ui: &mut Ui, task: &Task, block: &MergeBlock, files_open: bool, enabled: bool, acts: &mut Vec<Act>) {
    let mergeable = matches!(task.status, TaskStatus::Review | TaskStatus::Failed);
    if !mergeable {
        return;
    }
    let (buttons, files): (Vec<(String, BtnKind, bool, &str)>, bool) = match block {
        MergeBlock::MainDirty(_) => (
            vec![
                (
                    t!("Stash my changes and merge").to_owned(),
                    BtnKind::Primary,
                    enabled,
                    t!("Stashes your uncommitted changes (git stash), merges the task and gives the changes back; if they clash with the task, they stay stashed and the notice says how to recover them"),
                ),
                (
                    if files_open { t!("Hide the list") } else { t!("See what is uncommitted") }.to_owned(),
                    BtnKind::Secondary,
                    true,
                    t!("Shows all the files with uncommitted changes in the main folder"),
                ),
            ],
            true,
        ),
        MergeBlock::TaskDirty(_) => (
            vec![(
                t!("Ask the agent to commit").to_owned(),
                BtnKind::Primary,
                enabled,
                t!("Sends the task back to the agent with the list of files and a request to commit"),
            )],
            false,
        ),
        MergeBlock::Conflict(_) => (
            vec![(
                t!("Ask the agent to resolve the conflict").to_owned(),
                BtnKind::Primary,
                enabled,
                t!("Sends the task back to the agent asking it to merge the base into its branch (git merge) and resolve the conflicting files"),
            )],
            false,
        ),
    };
    match button_flow(ui, &buttons, "refusal") {
        Some(0) if files => acts.push(Act::MergeStash(task.id.clone())),
        Some(0) => acts.push(Act::AskAgent(task.id.clone())),
        Some(_) => acts.push(Act::ToggleNoticeFiles),
        None => {}
    }
    if files_open && files {
        let list = block.files().join("\n");
        paragraph(ui, &clip_text(&list, TEXT_MAX), theme::mono(11.0), theme::pal().text_2);
    }
}

/// The footer's height with the return/reject field open (the field, its hint and the buttons).
fn return_form_height(ui: &Ui, task: &Task) -> f32 {
    let w = ui.max_rect().width() - 2.0 * PAD;
    let hint = ui.painter().layout(return_hint(task).to_owned(), theme::ui(11.0), theme::pal().text_3, w).size().y;
    8.0 + 92.0 + 5.0 + hint + 8.0 + BTN_H + 6.0 + 8.0
}

/// What the agent gets with the feedback (the hint under the field).
fn return_hint(task: &Task) -> &'static str {
    if task.status == TaskStatus::Validating {
        t!("The agent gets this with the note that the work is already merged. Ctrl+Enter sends.")
    } else {
        t!("The agent gets this as a request for changes. Ctrl+Enter sends.")
    }
}

/// Feedback field of "Return" / "Reject", pinned in the footer, with "Send" and "Cancel".
fn return_form(deck: &mut Deck, ui: &mut Ui, task: &Task, working: bool, acts: &mut Vec<Act>) {
    // In review it is sent back with adjustments; awaiting validation, it is rejected with what did not work in the test.
    let (hint, send, send_tip, cancel_tip) = if task.status == TaskStatus::Validating {
        (
            t!("What did not work in the test?"),
            t!("Reject and send"),
            t!("Sends the task back to the same agent, saying the work was already merged and what failed in the test"),
            t!("Closes without rejecting"),
        )
    } else {
        (
            t!("What does the agent need to change?"),
            t!("Send to the agent"),
            t!("Sends the task back to the agent with this request for changes"),
            t!("Closes without returning"),
        )
    };
    let id = Id::new("tasks-return-feedback");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    if !focused && deck.tasks.feedback.is_empty() {
        ui.ctx().memory_mut(|m| m.request_focus(id));
    }
    let ready = !working && !deck.tasks.feedback.trim().is_empty();
    let ctrl_enter = focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    padded(ui, |ui| {
        text_field(ui, id, &mut deck.tasks.feedback, hint, Some(4));
        ui.add_space(5.0);
        let g = ui.painter().layout(return_hint(task).to_owned(), theme::ui(11.0), theme::pal().text_3, ui.available_width());
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y), Sense::hover());
        ui.painter().galley(r.min, g, theme::pal().text_3);
    });
    let b = [(send.to_owned(), BtnKind::Primary, ready, send_tip), (t!("Cancel").to_owned(), BtnKind::Secondary, true, cancel_tip)];
    ui.add_space(8.0);
    match button_flow(ui, &b, "return") {
        Some(0) => acts.push(Act::SendReturn(task.id.clone())),
        Some(_) => acts.push(Act::CancelReturn),
        None if ctrl_enter && ready => acts.push(Act::SendReturn(task.id.clone())),
        None => {}
    }
}

// ---------------------------------------------------------------------------
// Confirmation
// ---------------------------------------------------------------------------

/// Confirmation modal for "Merge" and "Discard", inside the panel, over the dimmed background.
fn confirm_dialog(deck: &mut Deck, ctx: &egui::Context, panel: Rect, acts: &mut Vec<Act>) {
    let Some(confirm) = deck.tasks.confirm.clone() else { return };
    let (id, merge) = match &confirm {
        Confirm::Merge(id) => (id, true),
        Confirm::Discard(id) => (id, false),
    };
    let Some(task) = deck.tasks.task(id).cloned() else {
        deck.tasks.confirm = None;
        return;
    };
    let (title, detail, yes) = if merge {
        (
            t!("Merge \"{title}\" into {base}?", title = task.title, base = task.base),
            t!(
                "Branch {branch} goes into {base} with a merge commit (git merge --no-ff). The agent, the worktree and the branch stay until you test and validate the task.",
                branch = task.branch,
                base = task.base
            ),
            t!("Merge"),
        )
    } else {
        (
            t!("Discard \"{title}\"?", title = task.title),
            if task.status == TaskStatus::Validating {
                t!(
                    "The agent's tab closes and the task's worktree and branch are deleted. What was already merged stays in {base}: discarding does not undo the merge.",
                    base = task.base
                )
            } else {
                t!("The agent's tab closes and the task's worktree and branch are deleted, along with everything the agent did that was not merged. This cannot be undone.").to_owned()
            },
            t!("Discard"),
        )
    };
    super::modal_backdrop(deck, ctx, "tasks-confirm-backdrop");
    let w = (panel.width() - 24.0).clamp(220.0, 360.0);
    let pos = Pos2::new(panel.center().x - w / 2.0, panel.min.y + 70.0);
    Area::new(Id::new("tasks-confirm")).order(Order::Tooltip).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let p = theme::pal();
        let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), p.text_strong, w - 28.0);
        let g2 = ui.painter().layout(detail.clone(), theme::ui(12.0), p.text_2, w - 28.0);
        let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + BTN_H + 14.0;
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
        shadow(ui.painter(), rect, 10, 8, 24, p.shadow);
        frame(ui.painter(), rect, 10.0, p.popup, p.glass_border);
        let mut y = rect.min.y + 14.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g1.clone(), p.text_strong);
        y += g1.size().y + 6.0;
        ui.painter().galley(Pos2::new(rect.min.x + 14.0, y), g2, p.text_2);
        let by = rect.max.y - 14.0 - BTN_H;
        let dw = button_width(ui.painter(), yes, ButtonKind::Primary);
        let d = Rect::from_min_size(Pos2::new(rect.max.x - 14.0 - dw, by), Vec2::new(dw, BTN_H));
        let cw = button_width(ui.painter(), t!("Cancel"), ButtonKind::Secondary);
        let c = Rect::from_min_size(Pos2::new(d.min.x - 8.0 - cw, by), Vec2::new(cw, BTN_H));
        let dr = if merge {
            ui_button(ui, d, Id::new("tasks-confirm-yes"), yes, ButtonKind::Primary)
        } else {
            button_danger(ui, d, Id::new("tasks-confirm-yes"), yes)
        };
        let cr = ui_button(ui, c, Id::new("tasks-confirm-no"), t!("Cancel"), ButtonKind::Secondary);
        if dr.clicked() {
            acts.push(Act::ConfirmYes);
        } else if cr.clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
            acts.push(Act::ConfirmNo);
        }
    });
}

// ---------------------------------------------------------------------------
// New task
// ---------------------------------------------------------------------------

/// The segments of the effort selector, in the order of [`effort_of`].
fn effort_labels() -> [&'static str; 4] {
    [t!("Default"), t!("Low"), t!("Mid"), t!("High")]
}

/// The segment of an effort (`None` = "Default").
fn effort_index(effort: Option<Effort>) -> usize {
    effort.map_or(0, |e| 1 + Effort::ALL.iter().position(|x| *x == e).unwrap_or(0))
}

/// The effort of a segment.
fn effort_of(index: usize) -> Option<Effort> {
    index.checked_sub(1).and_then(|i| Effort::ALL.get(i).copied())
}

/// Section/field label of the form (IBM Plex Mono Medium 10 px, UPPERCASE, `text_3`, `0.1em`).
fn field_label(ui: &mut Ui, text: &str) {
    let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 20.0), Sense::hover());
    let color = theme::pal().text_3;
    let g = galley_spaced(ui.painter(), &text.to_uppercase(), theme::label(10.0), color, 10.0 * theme::LABEL_SPACING_EM);
    place(ui.painter(), Rect::from_min_max(Pos2::new(r.min.x, r.min.y + 4.0), r.max), Align2::LEFT_CENTER, g, color);
}

fn chip_width(painter: &egui::Painter, reference: &str) -> f32 {
    let t = text_w(painter, &util::chip_label(reference), theme::ui(11.5)).min(220.0);
    10.0 + t + 4.0 + (8.0 + text_w(painter, "×", theme::ui(13.0))) + 4.0 + 2.0
}

/// "Attach files" button and chips of the files attached to the new task's request.
fn form_attachments(deck: &mut Deck, ui: &mut Ui, acts: &mut Vec<Act>) {
    let avail = ui.available_width();
    let attachments = &deck.tasks.form.attachments;
    let busy = deck.tasks.form.busy;
    let p = theme::pal();

    if !attachments.is_empty() {
        ui.add_space(6.0);
        let painter = ui.painter().clone();
        let mut i = 0;
        while i < attachments.len() {
            let mut end = i;
            let mut w = 0.0;
            while end < attachments.len() {
                let cw = chip_width(&painter, &attachments[end].to_string_lossy()).min(avail);
                let next = w + cw + if end > i { 6.0 } else { 0.0 };
                if end > i && next > avail {
                    break;
                }
                w = next;
                end += 1;
            }
            let (row, _) = ui.allocate_exact_size(Vec2::new(avail, CHIP_H + 4.0), Sense::hover());
            let mut cx = row.min.x;
            for (idx, a) in attachments.iter().enumerate().take(end).skip(i) {
                let cw = chip_width(&painter, &a.to_string_lossy()).min(avail);
                let r = Rect::from_min_size(Pos2::new(cx, row.min.y + 2.0), Vec2::new(cw, CHIP_H));
                cx = r.max.x + 6.0;
                frame(ui.painter(), r, 5.0, p.recess, p.separator);
                let xw = 8.0 + text_w(ui.painter(), "×", theme::ui(13.0));
                let x_rect = Rect::from_min_size(Pos2::new(r.max.x - 5.0 - xw, r.min.y + 1.0), Vec2::new(xw, r.height() - 2.0));
                let tr = Rect::from_min_max(Pos2::new(r.min.x + 10.0, r.min.y), Pos2::new(x_rect.min.x - 4.0, r.max.y));
                let g = galley_trunc(ui.painter(), &util::chip_label(&a.to_string_lossy()), theme::ui(11.5), p.text_2, tr.width());
                place(ui.painter(), tr, Align2::LEFT_CENTER, g, p.text_2);
                let chip_resp = ui.interact(r, Id::new(("task-chip", idx)), Sense::hover());
                tip(chip_resp, a.display().to_string());
                let xr = if !busy {
                    clickable(ui, x_rect, Id::new(("task-chip-x", idx)))
                } else {
                    ui.interact(x_rect, Id::new(("task-chip-x", idx)), Sense::hover())
                };
                text_in(
                    ui.painter(),
                    x_rect,
                    Align2::CENTER_CENTER,
                    "×",
                    theme::ui(13.0),
                    if xr.hovered() && !busy { p.text } else { p.text_2 },
                );
                if tip(xr, t!("Remove attachment")).clicked() && !busy {
                    acts.push(Act::RemoveAttachment(idx));
                }
            }
            i = end;
        }
    }

    ui.add_space(6.0);
    let label = t!("Attach files");
    let lw = text_w(ui.painter(), label, theme::ui(11.5));
    let bw = 10.0 + 14.0 + 6.0 + lw + 12.0;
    let (row, _) = ui.allocate_exact_size(Vec2::new(avail, 26.0), Sense::hover());
    let btn = Rect::from_min_size(row.min, Vec2::new(bw.min(avail), 26.0));
    let resp =
        if !busy { clickable(ui, btn, Id::new("tasks-new-attach")) } else { ui.interact(btn, Id::new("tasks-new-attach"), Sense::hover()) };
    let hot = !busy && resp.hovered();
    frame(ui.painter(), btn, 5.0, theme::with_alpha(p.text_strong, if hot { 0.08 } else { 0.04 }), theme::with_alpha(p.text_strong, 0.14));
    let fg = if !busy {
        if hot {
            p.text
        } else {
            p.text_2
        }
    } else {
        p.text_3
    };
    deck.icons.paint(ui.painter(), Icon::Attach, Pos2::new(btn.min.x + 16.0, btn.center().y), 13.5, fg, 0.0);
    text_in(
        ui.painter(),
        Rect::from_min_max(Pos2::new(btn.min.x + 28.0, btn.min.y), Pos2::new(btn.max.x - 8.0, btn.max.y)),
        Align2::LEFT_CENTER,
        label,
        theme::ui(11.5),
        fg,
    );
    if tip(resp, t!("Attach files or images to the request (you can also drag and drop onto the form)")).clicked() && !busy {
        acts.push(Act::PickAttachments);
    }
}

/// Field-shaped button that opens a menu: text (with the agent's dot, if any) and "▾".
#[allow(clippy::too_many_arguments)]
fn dropdown_button(
    ui: &mut Ui,
    r: Rect,
    id: Id,
    label: &str,
    dot: Option<Color32>,
    enabled: bool,
    open: bool,
    tooltip: &str,
) -> (Rect, bool) {
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let p = ui.painter();
    let pal = theme::pal();
    let hot = enabled && (resp.hovered() || open);
    frame(p, r, 6.0, if hot { pal.hover } else { pal.recess }, if open { theme::accent() } else { pal.separator });
    let mut x = r.min.x + 10.0;
    if let Some(c) = dot {
        super::dot(p, Pos2::new(x + 4.0, r.center().y), c);
        x += 16.0;
    }
    let color = if enabled { pal.text } else { pal.text_3 };
    let g = galley_trunc(p, label, theme::ui(12.0), color, (r.max.x - 28.0 - x).max(10.0));
    place(p, Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(r.max.x - 26.0, r.max.y)), Align2::LEFT_CENTER, g, color);
    text_in(p, Rect::from_min_max(Pos2::new(r.max.x - 26.0, r.min.y), r.max), Align2::CENTER_CENTER, "▾", theme::ui(11.5), pal.text_2);
    let clicked = tip(resp, tooltip).clicked() && enabled;
    (r, clicked)
}

/// Quiet mark on the right of a form menu row.
#[derive(Clone, Copy)]
pub(super) enum Detail {
    /// "free": a model with no per-token cost.
    Free,
    /// "N models": how many the provider has.
    Count(usize),
    /// Current use of the official CLI's quota; `None` means there is no reading yet.
    Quota(Option<u8>),
}

/// Items of a form or filter menu.
#[derive(Clone)]
pub(super) struct MenuItem {
    pub label: String,
    pub dot: Option<Color32>,
    pub selected: bool,
    pub enabled: bool,
    /// Quiet text on the right: "12 models" on the provider step, "free" on the no-cost models.
    pub detail: Option<Detail>,
    /// Opens another step: draws the "›" arrow on the right.
    pub submenu: bool,
}

/// [`dropdown_popup`] under `anchor`, closing through the tasks' own acts.
fn dropdown_menu(
    ctx: &egui::Context,
    id: &str,
    anchor: Rect,
    items: &[MenuItem],
    acts: &mut Vec<Act>,
    is_history_menu: bool,
) -> Option<usize> {
    let (chosen, closed) = dropdown_popup(ctx, id, anchor, items, 0.0, false);
    if closed {
        acts.push(if is_history_menu { Act::CloseHistoryMenu } else { Act::CloseMenu });
    }
    chosen
}

/// Menu next to `anchor` (opaque `popup` background, `glass_border`, corners 8, 26 px items with `hover`), at least
/// `min_w` wide, under the anchor or above it. Returns the chosen index and whether it asks to close (a click outside
/// or Esc, with nothing chosen).
pub(super) fn dropdown_popup(
    ctx: &egui::Context,
    id: &str,
    anchor: Rect,
    items: &[MenuItem],
    min_w: f32,
    above: bool,
) -> (Option<usize>, bool) {
    let row = 26.0;
    let h = (items.len() as f32 * row).min(260.0) + 8.0;
    let w = anchor.width().max(min_w);
    let y = if above { anchor.min.y - 2.0 - h } else { anchor.max.y + 2.0 };
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(anchor.min.x, y), Vec2::new(w, h));
    let menu_rect = Rect::from_min_size(pos, Vec2::new(w, h));
    let mut chosen = None;
    let p = theme::pal();
    Area::new(Id::new(id)).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
        shadow(ui.painter(), rect, 8, 8, 24, p.shadow);
        frame(ui.painter(), rect, 8.0, p.popup, p.glass_border);
        let list = rect.shrink(4.0);
        let mut child = ui.new_child(UiBuilder::new().max_rect(list));
        child.set_clip_rect(list);
        egui::ScrollArea::vertical().id_salt((id, "list")).auto_shrink([false, false]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            for (i, item) in items.iter().enumerate() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(list.width(), row), Sense::hover());
                let resp = if item.enabled { clickable(ui, r, Id::new((id, i))) } else { ui.interact(r, Id::new((id, i)), Sense::hover()) };
                if item.enabled && resp.hovered() {
                    ui.painter().rect_filled(r, 5.0, p.hover);
                }
                let mut x = r.min.x + 10.0;
                if let Some(c) = item.dot {
                    super::dot(ui.painter(), Pos2::new(x + 4.0, r.center().y), c);
                    x += 16.0;
                }
                let color = if !item.enabled {
                    p.text_3
                } else if item.selected || resp.hovered() {
                    p.text
                } else {
                    p.text_2
                };
                let mut right = r.max.x - 10.0;
                if item.submenu {
                    let ar = Rect::from_min_max(Pos2::new(right - 12.0, r.min.y), Pos2::new(right, r.max.y));
                    text_in(ui.painter(), ar, Align2::CENTER_CENTER, "›", theme::ui(13.0), p.text_2);
                    right -= 16.0;
                }
                if let Some(detail) = &item.detail {
                    let (text, dcolor) = match detail {
                        Detail::Free => (t!("free").to_owned(), p.success),
                        Detail::Count(1) => (t!("1 model").to_owned(), p.text_2),
                        Detail::Count(count) => (t!("{count} models", count), p.text_2),
                        Detail::Quota(Some(used)) => (t!("{used}% used", used), if *used >= 80 { p.warning } else { p.success }),
                        Detail::Quota(None) => (t!("no reading").to_owned(), p.text_3),
                    };
                    let g = galley(ui.painter(), &text, theme::ui(11.0), dcolor);
                    let w = g.size().x;
                    place(
                        ui.painter(),
                        Rect::from_min_max(Pos2::new(right - w, r.min.y), Pos2::new(right, r.max.y)),
                        Align2::RIGHT_CENTER,
                        g,
                        dcolor,
                    );
                    right -= w + 8.0;
                }
                let label = if item.selected { format!("✓ {}", item.label) } else { item.label.clone() };
                let g = galley_trunc(ui.painter(), &label, theme::ui(12.0), color, (right - x).max(10.0));
                place(ui.painter(), Rect::from_min_max(Pos2::new(x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, color);
                if item.enabled && resp.clicked() {
                    chosen = Some(i);
                }
            }
        });
    });
    let closed = chosen.is_none() && (ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, menu_rect, anchor));
    (chosen, closed)
}

fn form_view(deck: &mut Deck, ui: &mut Ui, rect: Rect, acts: &mut Vec<Act>) {
    let (head, _) = head_bar(ui, rect, None, t!("New task"), false, acts);
    let footer = Rect::from_min_max(Pos2::new(rect.min.x, rect.max.y - FOOTER_H), rect.max);
    let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), Pos2::new(rect.max.x, footer.min.y));
    let ctx = ui.ctx().clone();

    // What the menus and the button need from the Deck, before the closure borrows the fields.
    let kind = deck.tasks.form.kind.clone();
    let choices = deck.tasks_agent_choices();
    let quota_used = deck.core.quota_used();
    let form_model = deck.tasks.form.model.clone();
    let agent_label = deck.agents.name(&kind);
    let agent_dot = deck.agents.tab_color(&kind);
    let picks_model = super::menus::picks_model(&kind);
    let (known, loading) = {
        let (m, l) = deck.models_of(&kind);
        (m.to_vec(), l)
    };
    let agent_items: Vec<MenuItem> = choices
        .iter()
        .map(|k| MenuItem {
            label: deck.agents.name(k),
            dot: Some(deck.agents.tab_color(k)),
            selected: *k == kind,
            enabled: true,
            detail: if ["claude", "antigravity", "codex", "opencode"].contains(&k.as_str()) {
                // The selected agent shows the quota of the chosen model's group; the others, the agent's.
                let used =
                    if *k == kind { deck.core.model_usage(k, form_model.as_deref()).map(|m| m.used) } else { quota_used.get(k).copied() };
                Some(Detail::Quota(used.map(|used| (used * 100.0).round().clamp(0.0, 100.0) as u8)))
            } else {
                None
            },
            submenu: false,
        })
        .collect();
    // OpenCode lists every connected provider (`opencode`, `opencode-go`…): the menu
    // asks for the provider first and shows only its models. The other agents have a single list.
    let groups = models::by_provider(&known);
    let sole = models::sole_provider(&groups).map(str::to_owned);
    let step = deck.tasks.form.provider.clone().filter(|p| groups.iter().any(|(name, _)| name == p)).or(sole.clone());
    let model_label = deck.tasks.form.model.clone().unwrap_or_else(|| t!("Default").to_owned());
    // Effort: what the agent (and the chosen model) allows; without it the selector is off, with the tooltip.
    let effort_off = models::effort_unavailable(&kind, deck.tasks.form.model.as_deref());
    let effort_at = if effort_off.is_some() { 0 } else { effort_index(deck.tasks.form.effort) };
    let rows: Vec<(MenuItem, Act)> = if let Some(provider) = &step {
        let mut rows = Vec::new();
        // With a single provider there is no previous step: the menu opens straight on its models.
        if sole.is_none() {
            rows.push((
                MenuItem { label: t!("‹ Providers").into(), dot: None, selected: false, enabled: true, detail: None, submenu: false },
                Act::SetProvider(None),
            ));
        }
        rows.push((
            MenuItem {
                label: t!("Default").into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        ));
        if let Some((_, list)) = groups.iter().find(|(name, _)| name == provider) {
            rows.extend(list.iter().map(|m| {
                let item = MenuItem {
                    label: models::short_model(m).to_owned(),
                    dot: None,
                    selected: deck.tasks.form.model.as_deref() == Some(m.as_str()),
                    enabled: true,
                    detail: models::is_free(m).then_some(Detail::Free),
                    submenu: false,
                };
                (item, Act::SetModel(Some(m.clone())))
            }));
        }
        rows
    } else if groups.is_empty() {
        let mut rows = vec![(
            MenuItem {
                label: t!("Default").into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        )];
        rows.extend(models::free_first(&known).iter().map(|m| {
            let item = MenuItem {
                label: m.clone(),
                dot: None,
                selected: deck.tasks.form.model.as_deref() == Some(m.as_str()),
                enabled: true,
                detail: models::is_free(m).then_some(Detail::Free),
                submenu: false,
            };
            (item, Act::SetModel(Some(m.clone())))
        }));
        if known.is_empty() {
            let text = if loading { t!("Loading models…") } else { t!("No model list: default only") };
            rows.push((
                MenuItem { label: text.into(), dot: None, selected: false, enabled: false, detail: None, submenu: false },
                Act::CloseMenu,
            ));
        }
        rows
    } else {
        let mut rows = vec![(
            MenuItem {
                label: t!("Default").into(),
                dot: None,
                selected: deck.tasks.form.model.is_none(),
                enabled: true,
                detail: None,
                submenu: false,
            },
            Act::SetModel(None),
        )];
        rows.extend(groups.iter().map(|(provider, list)| {
            let item = MenuItem {
                label: provider.clone(),
                dot: None,
                selected: false,
                enabled: true,
                detail: Some(Detail::Count(list.len())),
                submenu: true,
            };
            (item, Act::SetProvider(Some(provider.clone())))
        }));
        rows
    };
    let model_items: Vec<MenuItem> = rows.iter().map(|(item, _)| item.clone()).collect();
    let open_menu = deck.tasks.form.menu;
    let (title_id, brief_id, base_id) = (Id::new("tasks-new-title"), Id::new("tasks-new-brief"), Id::new("tasks-new-base"));
    let scope_id = Id::new("tasks-new-scope");
    let any_focus = [title_id, brief_id, base_id, scope_id].iter().any(|id| ctx.memory(|m| m.has_focus(*id)));
    let ctrl_enter = any_focus && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Enter));
    let focus_title = std::mem::take(&mut deck.tasks.form.focus_title);
    let can_submit = !deck.tasks.form.busy && !deck.tasks.form.title.trim().is_empty() && !choices.is_empty();
    let start_now = deck.tasks.form.start_now;
    let busy = deck.tasks.form.busy;
    let error = deck.tasks.form.error.clone();

    let more_open = deck.tasks.form.more_open;
    let base_label =
        if deck.tasks.form.base.trim().is_empty() { t!("current branch").to_owned() } else { deck.tasks.form.base.trim().to_owned() };
    let scope_count = tasks_ui::parse_scope(&deck.tasks.form.scope).len();
    // The quiet line under the agent: a quota or capacity warning (amber), or how much of the quota is used.
    let advisory = deck.core.quota_advisory(&kind, &choices).map(|note| t!("Quota warning: {note}. The choice is still available.", note));
    let capacity = deck.tasks.form.capacity_note.clone().map(|note| t!("Capacity warning: {note}. The choice is still available.", note));
    // The quota of the selected model's group; an unknown model falls back to the agent number and the hint says so.
    let model_usage = deck.core.model_usage(&kind, deck.tasks.form.model.as_deref());
    let used = model_usage.as_ref().map(|m| (m.used * 100.0).round().clamp(0.0, 100.0) as u8);
    let used_text = |u: u8| match &model_usage {
        // No model chosen: the CLI default, whose group the quota reading does not name.
        Some(m) if m.fallback && deck.tasks.form.model.is_none() => t!("{used}% of the agent's quota used", used = u),
        Some(m) if m.fallback => t!("{used}% of the agent's quota used (no quota group known for this model)", used = u),
        Some(ModelUsage { group: Some(group), .. }) => t!("{used}% of the {group} quota used", used = u, group = group),
        _ => t!("{used}% of the quota used", used = u),
    };
    let agent_hint: Option<(String, Color32)> = match (advisory.or(capacity), used) {
        (Some(w), _) => Some((w, theme::pal().warning)),
        (None, Some(u)) if deck.tasks.form.kind_is_auto => {
            Some((t!("Recommended for this size · {hint}", hint = used_text(u)), theme::pal().text_3))
        }
        (None, Some(u)) => Some((used_text(u), theme::pal().text_3)),
        (None, None) if deck.tasks.form.kind_is_auto => Some((t!("Recommended for this size").to_owned(), theme::pal().text_3)),
        (None, None) => None,
    };

    let mut anchors: [Option<Rect>; 2] = [None, None];
    let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    egui::ScrollArea::vertical().id_salt("tasks-form").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        ui.add_space(6.0);
        padded(ui, |ui| {
            field_label(ui, t!("Title"));
            text_field(ui, title_id, &mut deck.tasks.form.title, t!("What the agent should do, in one line"), None);
            if focus_title {
                ui.ctx().memory_mut(|m| m.request_focus(title_id));
            }
            ui.add_space(8.0);
            field_label(ui, t!("Request"));
            text_field(ui, brief_id, &mut deck.tasks.form.brief, t!("Details, files, acceptance criteria… (optional)"), Some(6));
            form_attachments(deck, ui, acts);
            ui.add_space(8.0);
            field_label(ui, t!("Size of the work"));
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
            let level_labels = Level::ALL.map(Level::label);
            let level_at = Level::ALL.iter().position(|l| *l == deck.tasks.form.level).unwrap_or(1);
            if let Some(i) = segmented(ui, r, Id::new("tasks-new-level"), &level_labels, level_at, true) {
                acts.push(Act::SetLevel(Level::ALL[i]));
            }
            tip(
                ui.interact(r, Id::new("tasks-new-level-tip"), Sense::hover()),
                t!("The size of the work (not the effort): decides which models can handle it. If you do not choose, Medium applies"),
            );
            ui.add_space(8.0);
            field_label(ui, t!("Who does it"));
            let (row, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 28.0), Sense::hover());
            let half = (row.width() - 8.0) / 2.0;
            let (_, click) = dropdown_button(
                ui,
                Rect::from_min_size(row.min, Vec2::new(half, row.height())),
                Id::new("tasks-new-agent"),
                &agent_label,
                Some(agent_dot),
                !choices.is_empty(),
                open_menu == Some(FormMenu::Agent),
                t!("The agent that will run the task"),
            );
            anchors[0] = Some(row);
            if click {
                acts.push(Act::ToggleMenu(FormMenu::Agent));
            }
            let (_, click) = dropdown_button(
                ui,
                Rect::from_min_max(Pos2::new(row.max.x - half, row.min.y), row.max),
                Id::new("tasks-new-model"),
                &model_label,
                None,
                picks_model,
                open_menu == Some(FormMenu::Model),
                if !picks_model {
                    t!("This agent does not allow choosing the model")
                } else if groups.is_empty() || sole.is_some() {
                    t!("The agent's LLM model")
                } else {
                    t!("Choose the provider and then the model")
                },
            );
            anchors[1] = Some(row);
            if click {
                acts.push(Act::ToggleMenu(FormMenu::Model));
            }
            if let Some((text, color)) = &agent_hint {
                ui.add_space(5.0);
                let g = ui.painter().layout(text.clone(), theme::ui(11.0), *color, ui.available_width());
                let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), g.size().y), Sense::hover());
                ui.painter().galley(r.min, g, *color);
            }
            ui.add_space(8.0);
            field_label(ui, t!("Effort"));
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 26.0), Sense::hover());
            let picked = ui
                .scope(|ui| {
                    if effort_off.is_some() {
                        ui.set_opacity(0.4);
                    }
                    segmented(ui, r, Id::new("tasks-new-effort"), &effort_labels(), effort_at, true)
                })
                .inner;
            match effort_off {
                Some(why) => {
                    // Covers the selector: no click or hover highlight, only the tooltip.
                    tip(ui.interact(r, Id::new("tasks-new-effort-off"), Sense::hover()), why);
                }
                None => {
                    if let Some(i) = picked {
                        acts.push(Act::SetEffort(effort_of(i)));
                    }
                }
            }
            // "▸ Branch and scope · from dev, no scope": folded, with the current values.
            ui.add_space(12.0);
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 22.0), Sense::hover());
            let rr = clickable(ui, r, Id::new("tasks-new-more"));
            let pal = theme::pal();
            deck.icons.paint(
                ui.painter(),
                Icon::TreeChevron,
                Pos2::new(r.min.x + 5.0, r.center().y),
                10.0,
                pal.text_2,
                if more_open { std::f32::consts::FRAC_PI_2 } else { 0.0 },
            );
            let lr = text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(r.min.x + 16.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                t!("Branch and scope"),
                theme::ui(11.5),
                if rr.hovered() { pal.text } else { pal.text_2 },
            );
            let summary = match scope_count {
                0 => t!("from {base}, no scope", base = base_label),
                1 => t!("from {base}, 1 path", base = base_label),
                n => t!("from {base}, {n} paths", base = base_label, n),
            };
            let g = galley_trunc(ui.painter(), &format!("· {summary}"), theme::ui(11.5), pal.text_3, (r.max.x - lr.max.x - 6.0).max(10.0));
            place(ui.painter(), Rect::from_min_max(Pos2::new(lr.max.x + 6.0, r.min.y), r.max), Align2::LEFT_CENTER, g, pal.text_3);
            if tip(rr, t!("The branch the task starts from and the files it will touch")).clicked() {
                acts.push(Act::ToggleFormMore);
            }
            if more_open {
                ui.add_space(2.0);
                field_label(ui, t!("Base"));
                text_field(ui, base_id, &mut deck.tasks.form.base, t!("current branch"), None);
                ui.add_space(8.0);
                field_label(ui, t!("Scope"));
                text_field(
                    ui,
                    scope_id,
                    &mut deck.tasks.form.scope,
                    t!("Files or folders it will touch, one per line (optional)"),
                    Some(3),
                );
            }
            ui.add_space(10.0);
            // "Start now".
            let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), Sense::hover());
            let rr = clickable(ui, r, Id::new("tasks-new-start"));
            let cb = Rect::from_min_size(Pos2::new(r.min.x + 1.0, r.center().y - 7.0), Vec2::splat(14.0));
            ui.painter().rect(
                cb,
                3.0,
                if start_now { theme::accent() } else { theme::pal().recess },
                Stroke::new(1.0, if start_now { theme::accent() } else { theme::pal().separator }),
                egui::StrokeKind::Inside,
            );
            if start_now {
                text_in(ui.painter(), cb, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::pal().on_accent);
            }
            text_in(
                ui.painter(),
                Rect::from_min_max(Pos2::new(cb.max.x + 8.0, r.min.y), r.max),
                Align2::LEFT_CENTER,
                t!("Start now"),
                theme::ui(12.0),
                if rr.hovered() { theme::pal().text } else { theme::pal().text_2 },
            );
            if tip(rr, t!("Creates the worktree and opens the agent right away. Off, the task stays a draft to start later")).clicked() {
                acts.push(Act::ToggleStartNow);
            }
        });
        if let Some(e) = &error {
            ui.add_space(8.0);
            error_box(ui, e, false);
        }
        ui.add_space(14.0);
    });

    // Pinned footer: Create, Cancel and the shortcut.
    let painter = ui.painter_at(footer);
    line_sep(&painter, Pos2::new(footer.min.x, footer.min.y + 0.5), Pos2::new(footer.max.x, footer.min.y + 0.5));
    let label = if busy {
        t!("Creating…")
    } else if start_now {
        t!("Create and start")
    } else {
        t!("Create draft")
    };
    let by = footer.min.y + 8.5;
    let cw = btn_width(&painter, label, BtnKind::Primary);
    let create = Rect::from_min_size(Pos2::new(footer.min.x + LIST_PAD, by), Vec2::new(cw, BTN_H));
    let create_tip = if can_submit { t!("Creates the task (Ctrl+Enter)") } else { t!("Write the task title") };
    let clicked = button(ui, create, Id::new("tasks-create"), label, BtnKind::Primary, can_submit, create_tip);
    if clicked || (ctrl_enter && can_submit) {
        acts.push(Act::Submit);
    }
    let xw = btn_width(&painter, t!("Cancel"), BtnKind::Secondary);
    let cancel = Rect::from_min_size(Pos2::new(create.max.x + 6.0, by), Vec2::new(xw, BTN_H));
    if button(ui, cancel, Id::new("tasks-cancel"), t!("Cancel"), BtnKind::Secondary, !busy, t!("Goes back to the list without creating")) {
        acts.push(Act::Back);
    }
    let hint = galley(&painter, "Ctrl+Enter", theme::meta(10.5), theme::pal().text_3);
    if hint.size().x + 12.0 < footer.max.x - LIST_PAD - cancel.max.x {
        place(
            &painter,
            Rect::from_min_max(Pos2::new(cancel.max.x, by), Pos2::new(footer.max.x - PAD, by + BTN_H)),
            Align2::RIGHT_CENTER,
            hint,
            theme::pal().text_3,
        );
    }

    match (open_menu, anchors) {
        (Some(FormMenu::Agent), [Some(a), _]) => {
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-agent", a, &agent_items, acts, false) {
                acts.push(Act::SetKind(choices[i].clone()));
            }
        }
        (Some(FormMenu::Model), [_, Some(a)]) => {
            if let Some(i) = dropdown_menu(&ctx, "tasks-menu-model", a, &model_items, acts, false) {
                if let Some((_, act)) = rows.into_iter().nth(i) {
                    acts.push(act);
                }
            }
        }
        _ => {}
    }
}

//! System notices on screen: the bell in the title bar (with the count of unread warnings and errors), its history
//! popover and the toasts in the bottom-right corner. The list and the toasts' clock live in `notices.rs`.

use std::time::{Duration, Instant};

use egui::{Align2, Area, Color32, Id, Key, Order, Pos2, Rect, Sense, Shape, Stroke, Ui, UiBuilder, Vec2};
use holt_core::{t, Tone};

use super::{clickable, frame, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::icons::Icon;
use crate::notices::{toast_life, Notice};
use crate::theme;

/// Width of the history popover.
const POP_W: f32 = 340.0;
/// Maximum height of the history list.
const POP_LIST_MAX_H: f32 = 420.0;
/// Width of a toast (less in a narrow window).
const TOAST_W: f32 = 320.0;
/// Room left under the toasts for the right bar's footer ("+ New task").
pub const TOAST_BOTTOM: f32 = 52.0;

/// The colour of a tone (icons, the toast's border, the bell's count).
pub fn tone_color(tone: Tone) -> Color32 {
    let p = theme::pal();
    match tone {
        Tone::Info => p.text_3,
        Tone::Success => p.success,
        Tone::Warning => p.warning,
        Tone::Error => p.danger,
    }
}

/// The 14 px mark of a notice: a ring with a dot for a task (in the accent, or the tone's colour when it is not
/// plain information), otherwise the tone: an outlined "i", a check, a triangle with "!" or a cross.
pub fn tone_mark(painter: &egui::Painter, c: Pos2, tone: Tone, task: bool) {
    let p = theme::pal();
    if task {
        let color = if tone == Tone::Info { theme::accent() } else { tone_color(tone) };
        painter.circle_stroke(c, 5.6, Stroke::new(1.4, color));
        painter.circle_filled(c, 2.5, color);
        return;
    }
    match tone {
        Tone::Info => {
            painter.circle_stroke(c, 6.0, Stroke::new(1.3, p.text_3));
            painter.line_segment([c + Vec2::new(0.0, -0.6), c + Vec2::new(0.0, 3.2)], Stroke::new(1.5, p.text_3));
            painter.circle_filled(c + Vec2::new(0.0, -3.0), 0.9, p.text_3);
        }
        Tone::Success => {
            painter.circle_filled(c, 6.5, p.success);
            let pts = vec![c + Vec2::new(-2.8, 0.2), c + Vec2::new(-0.8, 2.0), c + Vec2::new(2.8, -2.0)];
            painter.add(Shape::line(pts, Stroke::new(1.6, p.on_accent)));
        }
        Tone::Warning => {
            let pts = vec![c + Vec2::new(0.0, -6.0), c + Vec2::new(6.4, 5.4), c + Vec2::new(-6.4, 5.4)];
            painter.add(Shape::convex_polygon(pts, p.warning, Stroke::NONE));
            painter.line_segment([c + Vec2::new(0.0, -1.8), c + Vec2::new(0.0, 1.6)], Stroke::new(1.5, p.on_accent));
            painter.circle_filled(c + Vec2::new(0.0, 3.5), 0.85, p.on_accent);
        }
        Tone::Error => {
            painter.circle_filled(c, 6.5, p.danger);
            let s = Stroke::new(1.5, p.text_strong);
            painter.line_segment([c + Vec2::new(-2.2, -2.2), c + Vec2::new(2.2, 2.2)], s);
            painter.line_segment([c + Vec2::new(2.2, -2.2), c + Vec2::new(-2.2, 2.2)], s);
        }
    }
}

/// The bell of the title bar (26×24) with the count of unread warnings and errors. Returns its rectangle.
pub fn bell(deck: &mut Deck, ui: &Ui, painter: &egui::Painter, rect: Rect) -> Rect {
    let p = theme::pal();
    let r = clickable(ui, rect, Id::new("notices-bell"));
    let open = deck.notices.open;
    if r.hovered() || open {
        painter.rect_filled(rect, 5.0, if open { p.selected } else { p.hover });
    }
    let color = if r.hovered() || open { p.text } else { p.text_2 };
    deck.icons.paint(painter, Icon::Bell, rect.center(), 15.0, color, 0.0);
    let unread = deck.notices.unread();
    if unread > 0 {
        let text = if unread > 9 { "9+".to_owned() } else { unread.to_string() };
        let font = theme::ui_bold(9.0);
        let w = (text_w(painter, &text, font.clone()) + 6.0).max(13.0);
        let pill = Rect::from_min_size(Pos2::new(rect.max.x - w + 2.0, rect.min.y - 1.0), Vec2::new(w, 13.0));
        let fill = tone_color(deck.notices.unread_tone().unwrap_or(Tone::Warning));
        painter.rect_filled(pill.expand(1.5), 8.0, p.bg);
        painter.rect_filled(pill, 7.0, fill);
        let fg = if fill == p.warning { p.on_accent } else { p.text_strong };
        text_in(painter, pill, Align2::CENTER_CENTER, &text, font, fg);
    }
    let tooltip = match unread {
        0 => t!("Notifications").to_owned(),
        n => t!("Notifications · {count} to see", count = n),
    };
    if tip(r, tooltip).clicked() {
        deck.notices.toggle();
    }
    rect
}

/// What was clicked in a notice (applied after painting, with the whole `Deck`).
enum Act {
    Open(u64),
    Dismiss(u64),
    Hold(u64),
    Clear,
    Close,
}

/// The history popover (under the bell) and the toasts. `corner` is the bottom-right point the toasts stack from.
pub fn draw(deck: &mut Deck, ctx: &egui::Context, bell: Rect, corner: Pos2) {
    let now = Instant::now();
    if let Some(next) = deck.notices.tick(now) {
        ctx.request_repaint_after(next.saturating_duration_since(now).min(Duration::from_millis(100)));
    }
    let mut acts = Vec::new();
    if deck.notices.open {
        popover(deck, ctx, bell, &mut acts);
    }
    toasts(deck, ctx, corner, now, &mut acts);
    for act in acts {
        match act {
            Act::Open(id) => {
                deck.notices.open = false;
                deck.open_notice_task(id);
            }
            Act::Dismiss(id) => deck.notices.dismiss(id),
            Act::Hold(id) => deck.notices.hold(id, now),
            Act::Clear => deck.notices.clear(),
            Act::Close => deck.notices.open = false,
        }
    }
}

/// The line under a notice: its age, the project when it is not the one on screen, and "Open t4" for a task.
/// Returns the clicked task link, if any.
fn meta_line(deck: &Deck, ui: &mut Ui, n: &Notice, x: f32, y: f32, max_x: f32, now_ms: u64) -> bool {
    let p = theme::pal();
    let painter = ui.painter().clone();
    let mut parts = vec![crate::tasks_ui::age_text(now_ms, n.at_ms)];
    if let Some(name) = deck.notice_project_name(n.project.as_deref()) {
        parts.push(name);
    }
    let text = parts.join(" · ");
    let g = galley_trunc(&painter, &text, theme::ui(10.5), p.text_3, (max_x - x).max(10.0));
    let r = place(&painter, Rect::from_min_max(Pos2::new(x, y), Pos2::new(max_x, y + 15.0)), Align2::LEFT_CENTER, g, p.text_3);
    let Some(task) = n.task.as_deref() else { return false };
    let label = t!("Open {id}", id = task);
    let lx = r.max.x + 10.0;
    let w = text_w(&painter, &label, theme::ui_medium(10.5));
    if lx + w > max_x {
        return false;
    }
    let link = Rect::from_min_size(Pos2::new(lx, y), Vec2::new(w, 15.0));
    let resp = clickable(ui, link, Id::new(("notice-open", n.id)));
    let color = if resp.hovered() { p.text_strong } else { p.text };
    text_in(&painter, link, Align2::LEFT_CENTER, &label, theme::ui_medium(10.5), color);
    painter.line_segment(
        [Pos2::new(link.min.x, link.max.y - 1.5), Pos2::new(link.max.x, link.max.y - 1.5)],
        Stroke::new(1.0, theme::with_alpha(color, 0.35)),
    );
    tip(resp, t!("Opens the task in the Tasks tab")).clicked()
}

fn popover(deck: &mut Deck, ctx: &egui::Context, bell: Rect, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let now_ms = holt_core::util::now_ms();
    let list: Vec<Notice> = deck.notices.list.iter().rev().cloned().collect();
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(bell.max.x - POP_W, bell.max.y + 4.0), Vec2::new(POP_W, 120.0));
    // The list's height is known after it is laid out: the frame uses the last frame's.
    let h_id = Id::new("notices-pop-h");
    let list_h = ctx.data(|d| d.get_temp::<f32>(h_id)).unwrap_or(60.0).min(POP_LIST_MAX_H);
    let head_h = 38.0;
    let size = Vec2::new(POP_W, head_h + list_h + 6.0);
    let rect = Rect::from_min_size(pos, size);
    Area::new(Id::new("notices-pop")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let (rect, _) = ui.allocate_exact_size(size, Sense::click());
        shadow(ui.painter(), rect, 10, 8, 24, p.shadow);
        frame(ui.painter(), rect, 10.0, p.popup, p.glass_border);
        let head = Rect::from_min_size(rect.min, Vec2::new(rect.width(), head_h));
        text_in(
            ui.painter(),
            Rect::from_min_max(Pos2::new(head.min.x + 12.0, head.min.y), head.max),
            Align2::LEFT_CENTER,
            t!("Notifications"),
            theme::ui_semibold(13.0),
            p.text_strong,
        );
        if !list.is_empty() {
            let label = t!("Clear");
            let w = text_w(ui.painter(), label, theme::ui(11.5)) + 14.0;
            let b = Rect::from_min_size(Pos2::new(head.max.x - 8.0 - w, head.center().y - 11.0), Vec2::new(w, 22.0));
            let r = clickable(ui, b, Id::new("notices-clear"));
            if r.hovered() {
                ui.painter().rect_filled(b, 5.0, p.hover);
            }
            text_in(ui.painter(), b, Align2::CENTER_CENTER, label, theme::ui(11.5), if r.hovered() { p.text } else { p.text_2 });
            if tip(r, t!("Removes every notification")).clicked() {
                acts.push(Act::Clear);
            }
        }
        let body = Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), Pos2::new(rect.max.x, rect.max.y - 6.0));
        let mut child = ui.new_child(UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
        child.set_clip_rect(body);
        let out = egui::ScrollArea::vertical().id_salt("notices-list").auto_shrink([false, true]).show(&mut child, |ui| {
            ui.spacing_mut().item_spacing = Vec2::ZERO;
            if list.is_empty() {
                let (r, _) = ui.allocate_exact_size(Vec2::new(body.width(), 44.0), Sense::hover());
                text_in(
                    ui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + 12.0, r.min.y), r.max),
                    Align2::LEFT_CENTER,
                    t!("No notifications. Errors, finished downloads and tasks that need you show up here."),
                    theme::ui(12.0),
                    p.text_3,
                );
                return;
            }
            for n in &list {
                notice_row(deck, ui, n, now_ms, acts);
            }
        });
        ctx.data_mut(|d| d.insert_temp(h_id, out.content_size.y));
    });
    if ctx.input(|i| i.key_pressed(Key::Escape)) || super::menus::closes_on_outside(ctx, rect, bell) {
        acts.push(Act::Close);
    }
}

/// A row of the history: the mark, the text (wrapped) and the meta line.
fn notice_row(deck: &Deck, ui: &mut Ui, n: &Notice, now_ms: u64, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let w = ui.available_width();
    let text_x = 12.0 + 14.0 + 9.0;
    let color = if n.read { p.text_2 } else { p.text };
    let g = ui.painter().layout(n.text.clone(), theme::ui(12.0), color, w - text_x - 12.0);
    let h = 7.0 + g.size().y + 3.0 + 15.0 + 7.0;
    let (r, resp) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
    if resp.hovered() {
        ui.painter().rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 6.0, p.hover);
    }
    let first_row = g.rows.first().map_or(16.0, |row| row.height());
    tone_mark(ui.painter(), Pos2::new(r.min.x + 12.0 + 7.0, r.min.y + 7.0 + first_row / 2.0), n.tone, n.task.is_some());
    ui.painter().galley(Pos2::new(r.min.x + text_x, r.min.y + 7.0), g.clone(), color);
    let y = r.min.y + 7.0 + g.size().y + 3.0;
    if meta_line(deck, ui, n, r.min.x + text_x, y, r.max.x - 12.0, now_ms) {
        acts.push(Act::Open(n.id));
    }
}

fn toasts(deck: &mut Deck, ctx: &egui::Context, corner: Pos2, now: Instant, acts: &mut Vec<Act>) {
    let p = theme::pal();
    let now_ms = holt_core::util::now_ms();
    let screen = ctx.content_rect();
    let w = TOAST_W.min(screen.width() - 24.0).max(160.0);
    let shown: Vec<(Notice, Option<Instant>)> =
        deck.notices.toasts.iter().filter_map(|t| deck.notices.get(t.id).cloned().map(|n| (n, t.until))).collect();
    let mut bottom = corner.y;
    // The newest at the bottom, nearest the corner.
    for (n, until) in shown.iter().rev() {
        let painter = ctx.layer_painter(egui::LayerId::new(Order::Foreground, Id::new("toast-measure")));
        let text_x = 12.0 + 14.0 + 9.0;
        let text_w_max = w - text_x - 30.0;
        let g = painter.layout(n.text.clone(), theme::ui(12.0), p.text, text_w_max);
        let meta = n.task.is_some() || deck.notice_project_name(n.project.as_deref()).is_some();
        let h = 10.0 + g.size().y + if meta { 3.0 + 15.0 } else { 0.0 } + 10.0;
        let rect = Rect::from_min_size(Pos2::new(corner.x - w, bottom - h), Vec2::new(w, h));
        bottom = rect.min.y - 8.0;
        Area::new(Id::new(("toast", n.id))).order(Order::Foreground).fixed_pos(rect.min).constrain(false).show(ctx, |ui| {
            let (rect, resp) = ui.allocate_exact_size(rect.size(), Sense::click());
            if resp.hovered() || resp.contains_pointer() {
                acts.push(Act::Hold(n.id));
            }
            let border = if n.tone == Tone::Error { theme::with_alpha(p.danger, 0.45) } else { p.glass_border };
            shadow(ui.painter(), rect, 9, 8, 24, p.shadow);
            frame(ui.painter(), rect, 9.0, p.popup, border);
            let first_row = g.rows.first().map_or(16.0, |row| row.height());
            tone_mark(ui.painter(), Pos2::new(rect.min.x + 12.0 + 7.0, rect.min.y + 10.0 + first_row / 2.0), n.tone, n.task.is_some());
            ui.painter().galley(Pos2::new(rect.min.x + text_x, rect.min.y + 10.0), g.clone(), p.text);
            if meta && meta_line(deck, ui, n, rect.min.x + text_x, rect.min.y + 10.0 + g.size().y + 3.0, rect.max.x - 30.0, now_ms) {
                acts.push(Act::Open(n.id));
            }
            // ✕ (drawn: Inter has no glyph for it).
            let x = Rect::from_min_size(Pos2::new(rect.max.x - 26.0, rect.min.y + 6.0), Vec2::splat(20.0));
            let xr = clickable(ui, x, Id::new(("toast-x", n.id)));
            if xr.hovered() {
                ui.painter().rect_filled(x, 4.0, p.hover);
            }
            let c = if xr.hovered() { p.text } else { p.text_3 };
            let s = Stroke::new(1.3, c);
            ui.painter().line_segment([x.center() + Vec2::splat(-3.5), x.center() + Vec2::splat(3.5)], s);
            ui.painter().line_segment([x.center() + Vec2::new(3.5, -3.5), x.center() + Vec2::new(-3.5, 3.5)], s);
            if tip(xr, t!("Close")).clicked() {
                acts.push(Act::Dismiss(n.id));
            }
            // The time left, as a thin line along the bottom edge.
            if let (Some(until), Some(life)) = (until, toast_life(n.tone)) {
                let left = until.saturating_duration_since(now).as_secs_f32() / life.as_secs_f32();
                let line = Rect::from_min_size(
                    Pos2::new(rect.min.x + 9.0, rect.max.y - 2.0),
                    Vec2::new((rect.width() - 18.0) * left.clamp(0.0, 1.0), 1.5),
                );
                ui.painter().rect_filled(line, 1.0, theme::with_alpha(p.text_3, 0.45));
            }
        });
    }
}

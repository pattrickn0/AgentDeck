//! Views of the Git tab beyond the list of changes: History, Stash and Conflicts. As in `git.rs`,
//! drawing only *collects* what was clicked ([`Act`]); `git::apply` is what applies it.

use egui::{Align2, Color32, Id, Key, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use holt_core::git::{ConflictBlock, ConflictChoice, EntryKind, GitStash, GitStatus, RepoState};
use holt_core::t;

use super::git::{diff_lines, Act, BTN_H};
use super::{clickable, frame, galley, galley_spaced, galley_trunc, place, text_in, text_w, tip};
use crate::deck::Deck;
use crate::git_ui::{GitSubView, OpenCommit, OpenConflict, OpenStashDiff};
use crate::theme;

const HEADER_H: f32 = 28.0;
const COMMIT_ROW_H: f32 = 42.0;
const STASH_ROW_H: f32 = 46.0;
const FILE_ROW_H: f32 = 18.0;
const CODE_ROW_H: f32 = 16.0;
/// Lines of each side shown in a conflict block (the rest becomes "+N lines").
const BLOCK_LINES: usize = 10;

/// Look of a text button.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Look {
    Normal,
    Primary,
    Danger,
}

/// Text button (pill) against `right`; returns the click and moves `right` back.
#[allow(clippy::too_many_arguments)]
pub(super) fn pill(ui: &Ui, right: &mut f32, y: f32, h: f32, label: &str, tooltip: &str, look: Look, enabled: bool, id: Id) -> bool {
    let painter = ui.painter();
    let w = galley(painter, label, theme::ui_semibold(11.5), theme::pal().text).size().x + 18.0;
    let r = Rect::from_min_size(Pos2::new(*right - w, y), Vec2::new(w, h));
    *right = r.min.x - 6.0;
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let hot = enabled && resp.hovered();
    let p = theme::pal();
    let (bg, border, fg) = match look {
        Look::Primary => {
            if enabled {
                let fill = if hot { theme::brighten(theme::accent(), 1.08) } else { theme::accent() };
                (fill, fill, p.on_accent)
            } else {
                (p.hover, p.separator, theme::with_opacity(p.text, 0.4))
            }
        }
        Look::Danger => {
            let fill = if hot { theme::with_alpha(p.danger, 0.18) } else { theme::with_alpha(p.danger, 0.10) };
            let border = theme::with_alpha(p.danger, 0.35);
            let fg = if enabled { p.danger_text } else { theme::with_opacity(p.danger_text, 0.4) };
            (fill, border, fg)
        }
        Look::Normal => {
            let fill = if hot { theme::with_alpha(p.text_strong, 0.08) } else { theme::with_alpha(p.text_strong, 0.04) };
            let border = theme::with_alpha(p.text_strong, 0.14);
            let fg = if enabled { p.text } else { theme::with_opacity(p.text, 0.4) };
            (fill, border, fg)
        }
    };
    frame(painter, r, 5.0, bg, border);
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), fg);
    tip(resp, tooltip.to_owned()).clicked() && enabled
}

/// Width of a pill (to align left or compute the space).
pub(super) fn pill_w(ui: &Ui, label: &str) -> f32 {
    galley(ui.painter(), label, theme::ui_semibold(11.5), theme::pal().text).size().x + 18.0
}

/// Header with "←", the title and the free space on the right (`right`) for buttons. Returns (header, back clicked, `right`).
/// The label of the button that takes a diff to Documents mode.
pub(super) fn open_docs_label() -> &'static str {
    t!("Open in Documents")
}
/// Minimum title space beside the button with text; with less, the button becomes only the icon.
const OPEN_DOCS_MIN_TITLE: f32 = 150.0;
/// The icon button (24 px) and the space before it.
const OPEN_DOCS_ICON_W: f32 = 26.0;

/// With `room` px for the title and the button, does the button go as an icon only?
pub(super) fn open_docs_compact(ui: &Ui, room: f32) -> bool {
    room - (pill_w(ui, open_docs_label()) + 6.0) < OPEN_DOCS_MIN_TITLE
}

/// Width the "Open in Documents" button takes on the right (with the space before it).
pub(super) fn open_docs_w(ui: &Ui, compact: bool) -> f32 {
    if compact {
        OPEN_DOCS_ICON_W
    } else {
        pill_w(ui, open_docs_label()) + 6.0
    }
}

/// "Open in Documents" in a diff header, against `right` (which moves left): with text, or
/// only the icon with the tooltip (`compact`, narrow bar). Returns whether it was clicked.
pub(super) fn open_docs(ui: &Ui, right: &mut f32, y: f32, compact: bool, tooltip: &str, id: Id) -> bool {
    if !compact {
        return pill(ui, right, y, 22.0, open_docs_label(), tooltip, Look::Normal, true, id);
    }
    let b = Rect::from_min_size(Pos2::new(*right - 24.0, y), Vec2::new(24.0, 22.0));
    *right = b.min.x - 2.0;
    let r = clickable(ui, b, id);
    let p = theme::pal();
    if r.hovered() {
        ui.painter().rect_filled(b, 5.0, p.separator);
    }
    paint_open_docs(ui.painter(), b.center(), if r.hovered() { p.text_strong } else { p.text });
    tip(r, format!("{}\n{tooltip}", open_docs_label())).clicked()
}

/// The "Open in Documents" icon: an open box in the corner and an arrow coming out of it.
fn paint_open_docs(p: &Painter, c: Pos2, color: Color32) {
    let s = Stroke::new(1.2, color);
    let (x0, y0, x1, y1) = (c.x - 5.5, c.y - 4.5, c.x + 4.5, c.y + 5.5);
    p.line_segment([Pos2::new(x0 + 4.0, y0), Pos2::new(x0, y0)], s);
    p.line_segment([Pos2::new(x0, y0), Pos2::new(x0, y1)], s);
    p.line_segment([Pos2::new(x0, y1), Pos2::new(x1, y1)], s);
    p.line_segment([Pos2::new(x1, y1), Pos2::new(x1, y1 - 4.0)], s);
    let tip_pt = Pos2::new(x1 + 1.0, y0 - 1.0);
    p.line_segment([Pos2::new(c.x - 1.0, c.y + 1.0), tip_pt], s);
    p.line_segment([tip_pt, Pos2::new(tip_pt.x - 4.5, tip_pt.y)], s);
    p.line_segment([tip_pt, Pos2::new(tip_pt.x, tip_pt.y + 4.5)], s);
}

fn header(ui: &Ui, body: Rect, id: &str, title: &str, hint: &str, reserved_right: f32) -> (Rect, bool, f32) {
    let painter = ui.painter_at(body);
    let head = Rect::from_min_size(body.min, Vec2::new(body.width(), HEADER_H));
    painter.rect_filled(head, 0.0, theme::pal().chip);
    let back = Rect::from_min_size(Pos2::new(head.min.x + 6.0, head.min.y + 3.0), Vec2::new(26.0, 22.0));
    let br = clickable(ui, back, Id::new(("git-back", id)));
    if br.hovered() {
        painter.rect_filled(back, 5.0, theme::pal().separator);
    }
    text_in(&painter, back, Align2::CENTER_CENTER, "←", theme::ui(15.0), theme::pal().text);
    let back_clicked = tip(br, t!("Back")).clicked();
    let right = head.max.x - 6.0 - reserved_right;
    let tr = Rect::from_min_max(Pos2::new(back.max.x + 8.0, head.min.y), Pos2::new(right, head.max.y));
    let g = galley_trunc(&painter, title, theme::ui(12.0), theme::pal().text, tr.width());
    place(&painter, tr, Align2::LEFT_CENTER, g, theme::pal().text);
    tip(ui.interact(tr, Id::new(("git-head-title", id)), Sense::hover()), hint.to_owned());
    (head, back_clicked, head.max.x - 6.0)
}

fn note(ui: &Ui, area: Rect, text: &str, color: egui::Color32) {
    let p = ui.painter_at(area);
    let g = p.layout(text.to_owned(), theme::ui(12.5), color, area.width() - 28.0);
    p.galley(area.min + Vec2::new(14.0, 14.0), g, color);
}

fn scroll_child(ui: &mut Ui, area: Rect) -> Ui {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(area).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(area);
    child
}

// ---------- History ----------

pub(super) fn history(deck: &mut Deck, ui: &mut Ui, body: Rect, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.commit_detail.take() {
        commit_detail(ui, body, &open, acts);
        deck.git.commit_detail = Some(open);
        return;
    }
    if deck.git.log.is_empty() {
        let text = if deck.git.log_loading { t!("Reading the history…") } else { t!("No commits yet on this branch.") };
        note(ui, body, text, theme::pal().text_2);
        return;
    }
    let log = &deck.git.log;
    let (has_more, loading) = (deck.git.log_has_more, deck.git.log_loading);
    let n = log.len() + usize::from(has_more);
    let mut child = scroll_child(ui, body);
    egui::ScrollArea::vertical().id_salt("git-log").auto_shrink([false, false]).show_rows(&mut child, COMMIT_ROW_H, n, |ui, range| {
        for i in range {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), COMMIT_ROW_H), Sense::click());
            let p = ui.painter();
            let Some(c) = log.get(i) else {
                // "Load more" at the end of the list.
                let label = if loading { t!("Loading…") } else { t!("Load more commits") };
                let hot = resp.hovered() && !loading;
                if hot {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                text_in(
                    p,
                    r,
                    Align2::CENTER_CENTER,
                    label,
                    theme::ui_semibold(12.0),
                    if hot { theme::pal().text } else { theme::pal().text_2 },
                );
                if resp.clicked() && !loading {
                    acts.push(Act::MoreLog);
                }
                continue;
            };
            if resp.hovered() {
                p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let w = r.width() - 28.0;
            let g = galley_trunc(p, &c.subject, theme::ui(12.5), theme::pal().text, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.min.y + 5.0), Vec2::new(w, 17.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            // Second line: hash (mono) · author · date.
            let hg = galley(p, &c.short_hash, theme::mono(11.0), theme::pal().info);
            let hw = hg.size().x;
            let y2 = r.min.y + 24.0;
            place(p, Rect::from_min_size(Pos2::new(r.min.x + 14.0, y2), Vec2::new(hw, 14.0)), Align2::LEFT_CENTER, hg, theme::pal().info);
            let meta = format!("{} · {}", c.author, c.date);
            let mg = galley_trunc(p, &meta, theme::ui(11.0), theme::pal().text_2, (w - hw - 8.0).max(20.0));
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 14.0 + hw + 8.0, y2), Vec2::new(w - hw - 8.0, 14.0)),
                Align2::LEFT_CENTER,
                mg,
                theme::pal().text_2,
            );
            if tip(resp, format!("{}\n{} · {} · {}", c.subject, c.short_hash, c.author, c.date)).clicked() {
                acts.push(Act::OpenCommit(c.clone()));
            }
        }
    });
}

fn commit_detail(ui: &mut Ui, body: Rect, open: &OpenCommit, acts: &mut Vec<Act>) {
    let title = format!("{}  {}", open.short_hash, open.subject);
    // The title sits between the "←" (6 + 26 + 8 px) and the button.
    let compact = open_docs_compact(ui, body.width() - 6.0 - 40.0);
    let (head, back, mut right) = header(ui, body, "commit", &title, &open.subject, open_docs_w(ui, compact));
    if back {
        acts.push(Act::CloseCommit);
    }
    let tip_text = t!("Opens the diff of this commit, read-only, with the full width of the center");
    if open_docs(ui, &mut right, head.min.y + 3.0, compact, tip_text, Id::new("git-commit-center")) {
        acts.push(Act::OpenCommitCenter(open.hash.clone(), open.short_hash.clone(), open.subject.clone()));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y), body.max);
    let d = match &open.detail {
        None => return note(ui, area, t!("Loading the commit…"), theme::pal().text_2),
        Some(Err(e)) => return note(ui, area, e, theme::pal().danger_text),
        Some(Ok(d)) => d,
    };
    let p = ui.painter_at(area);
    let mut y = area.min.y + 8.0;
    let w = area.width() - 28.0;
    let meta = format!("{} <{}> · {}", d.author, d.email, d.date);
    let g = galley_trunc(&p, &meta, theme::ui(11.5), theme::pal().text_2, w);
    place(&p, Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(w, 16.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    y += 20.0;
    // The whole message, at most 5 lines high.
    let mut job = egui::text::LayoutJob::simple(d.message.clone(), theme::ui(12.5), theme::pal().text, w);
    job.wrap.max_rows = 5;
    job.wrap.overflow_character = Some('…');
    let mg = p.layout_job(job);
    let mh = mg.size().y;
    p.galley(Pos2::new(area.min.x + 14.0, y), mg, theme::pal().text);
    y += mh + 8.0;
    // Changed files (up to 5 lines).
    let title = t!("{count} FILE(S) CHANGED", count = d.files.len());
    let g = galley_spaced(&p, &title, theme::label(10.0), theme::pal().text_2, 0.8);
    place(&p, Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(w, 16.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    y += 18.0;
    for (letter, path) in d.files.iter().take(5) {
        let color = match letter {
            'A' => theme::pal().success,
            'D' => theme::pal().danger_text,
            'M' => theme::pal().warning,
            _ => theme::pal().info,
        };
        text_in(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 14.0, y), Vec2::new(12.0, FILE_ROW_H)),
            Align2::CENTER_CENTER,
            &letter.to_string(),
            theme::mono_bold(11.5),
            color,
        );
        let g = galley_trunc(&p, path, theme::ui(12.0), theme::pal().text, w - 20.0);
        place(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 34.0, y), Vec2::new(w - 20.0, FILE_ROW_H)),
            Align2::LEFT_CENTER,
            g,
            theme::pal().text,
        );
        y += FILE_ROW_H;
    }
    if d.files.len() > 5 {
        text_in(
            &p,
            Rect::from_min_size(Pos2::new(area.min.x + 34.0, y), Vec2::new(w, FILE_ROW_H)),
            Align2::LEFT_CENTER,
            &format!("+{} outros (veja no diff abaixo)", d.files.len() - 5),
            theme::ui(11.5),
            theme::pal().text_2,
        );
        y += FILE_ROW_H;
    }
    y += 6.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(area.min.x, y), Vec2::new(area.width(), 1.0)), 0.0, theme::pal().separator);
    let diff_area = Rect::from_min_max(Pos2::new(area.min.x, y + 1.0), area.max);
    if diff_area.height() > 40.0 {
        diff_lines(ui, diff_area, &d.diff, None, acts);
    }
}

// ---------- Stash ----------

pub(super) fn stash(deck: &mut Deck, ui: &mut Ui, body: Rect, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.stash_diff.take() {
        stash_diff(ui, body, &open, busy, acts);
        deck.git.stash_diff = Some(open);
        return;
    }
    let p = ui.painter_at(body);
    // Form: message, "include untracked" and Stash.
    let x0 = body.min.x + 10.0;
    let x1 = body.max.x - 10.0;
    let input = Rect::from_min_size(Pos2::new(x0, body.min.y + 8.0), Vec2::new(x1 - x0, 28.0));
    let id = Id::new("git-stash-msg");
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&p, input, 6.0, theme::pal().recess, if focused { theme::accent() } else { theme::pal().separator });
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(input.shrink(1.0)));
    let out = egui::TextEdit::singleline(&mut deck.git.stash_message)
        .id(id)
        .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: 9, right: 9, top: 5, bottom: 5 }))
        .font(theme::ui(12.5))
        .text_color(theme::pal().text)
        .hint_text(egui::RichText::new(t!("Stash message (optional)")).font(theme::ui(12.5)).color(theme::pal().text_3))
        .desired_width(input.width() - 2.0)
        .show(&mut child);
    let has_changes = !st.entries.is_empty();
    let enter = out.response.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
    let by = input.max.y + 8.0;
    let mut right = x1;
    let can = has_changes && !busy;
    let why = if has_changes {
        t!("Stashes the changes and leaves the folder clean (git stash push)")
    } else {
        t!("There are no changes to stash")
    };
    if pill(ui, &mut right, by, BTN_H, t!("Stash"), why, Look::Primary, can, Id::new("git-stash-push")) || (enter && can) {
        acts.push(Act::StashPush);
    }
    let box_r = Rect::from_min_size(Pos2::new(x0 + 2.0, by + (BTN_H - 14.0) / 2.0), Vec2::splat(14.0));
    let label = t!("Include untracked");
    let lw = galley(&p, label, theme::ui(12.0), theme::pal().text_2).size().x;
    let hit = Rect::from_min_max(Pos2::new(x0, by), Pos2::new(box_r.max.x + 8.0 + lw, by + BTN_H));
    let ar = clickable(ui, hit, Id::new("git-stash-untracked"));
    let on = deck.git.stash_include_untracked;
    p.rect(
        box_r,
        3.0,
        if on { theme::accent() } else { theme::pal().recess },
        egui::Stroke::new(1.0, if on { theme::accent() } else { theme::pal().separator }),
        egui::StrokeKind::Inside,
    );
    if on {
        text_in(&p, box_r, Align2::CENTER_CENTER, "✓", theme::ui_bold(11.0), theme::pal().on_accent);
    }
    text_in(
        &p,
        Rect::from_min_max(Pos2::new(box_r.max.x + 8.0, by), Pos2::new(hit.max.x, by + BTN_H)),
        Align2::LEFT_CENTER,
        label,
        theme::ui(12.0),
        if ar.hovered() { theme::pal().text } else { theme::pal().text_2 },
    );
    if tip(ar, t!("Also stashes (and removes from the folder) the files not tracked yet")).clicked() {
        acts.push(Act::StashToggleUntracked);
    }
    let top = by + BTN_H + 8.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(body.min.x, top), Vec2::new(body.width(), 1.0)), 0.0, theme::pal().separator);
    let list = Rect::from_min_max(Pos2::new(body.min.x, top + 1.0), body.max);
    if deck.git.stashes.is_empty() {
        let text = if deck.git.stash_loading {
            t!("Reading the stashes…")
        } else {
            t!("No stash. Stash the work in progress to come back to it later.")
        };
        return note(ui, list, text, theme::pal().text_2);
    }
    let stashes = &deck.git.stashes;
    let mut child = scroll_child(ui, list);
    egui::ScrollArea::vertical().id_salt("git-stashes").auto_shrink([false, false]).show_rows(
        &mut child,
        STASH_ROW_H,
        stashes.len(),
        |ui, range| {
            for i in range {
                let s = &stashes[i];
                let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), STASH_ROW_H), Sense::click());
                let hovered = ui.rect_contains_pointer(r);
                let p = ui.painter();
                if hovered {
                    p.rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                }
                let w = r.width() - 28.0;
                let g = galley_trunc(p, &s.message, theme::ui(12.5), theme::pal().text, w);
                place(
                    p,
                    Rect::from_min_size(Pos2::new(r.min.x + 14.0, r.min.y + 5.0), Vec2::new(w, 17.0)),
                    Align2::LEFT_CENTER,
                    g,
                    theme::pal().text,
                );
                // Second line: index/date on the left; actions on the right.
                let y2 = r.min.y + 24.0;
                let mut right = r.max.x - 12.0;
                let mut clicked_button = false;
                let h = 18.0;
                for (label, tooltip, look, act) in [
                    (t!("Delete"), t!("Deletes this stash (cannot be undone)"), Look::Danger, Act::AskDropStash(s.clone())),
                    (
                        t!("Apply and delete"),
                        t!("Applies the changes and removes the stash (git stash pop)"),
                        Look::Normal,
                        Act::StashPop(s.index),
                    ),
                    (t!("Apply"), t!("Applies the changes and keeps the stash (git stash apply)"), Look::Normal, Act::StashApply(s.index)),
                ] {
                    if pill_small(ui, &mut right, y2, h, label, tooltip, look, !busy, Id::new(("git-stash-btn", i, label))) {
                        acts.push(act);
                        clicked_button = true;
                    }
                }
                let meta = s.date.clone();
                let mg = galley_trunc(ui.painter(), &meta, theme::ui(11.0), theme::pal().text_2, (right - r.min.x - 20.0).max(20.0));
                place(
                    ui.painter(),
                    Rect::from_min_max(Pos2::new(r.min.x + 14.0, y2), Pos2::new(right, y2 + h)),
                    Align2::LEFT_CENTER,
                    mg,
                    theme::pal().text_2,
                );
                if tip(resp, t!("stash@{{index}}\nClick to see the stashed changes", index = s.index)).clicked() && !clicked_button {
                    acts.push(Act::OpenStash(s.clone()));
                }
            }
        },
    );
}

/// Smaller pill (11 px) for list rows.
#[allow(clippy::too_many_arguments)]
fn pill_small(ui: &Ui, right: &mut f32, y: f32, h: f32, label: &str, tooltip: &str, look: Look, enabled: bool, id: Id) -> bool {
    let painter = ui.painter();
    let w = galley(painter, label, theme::ui(11.0), theme::pal().text).size().x + 12.0;
    let r = Rect::from_min_size(Pos2::new(*right - w, y), Vec2::new(w, h));
    *right = r.min.x - 4.0;
    let resp = if enabled { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
    let hot = enabled && resp.hovered();
    let p = theme::pal();
    let (bg, border, fg) = match look {
        Look::Danger => (
            if hot { theme::with_alpha(p.danger, 0.18) } else { theme::with_alpha(p.danger, 0.10) },
            theme::with_alpha(p.danger, 0.35),
            p.danger_text,
        ),
        _ => (
            if hot { theme::with_alpha(p.text_strong, 0.08) } else { theme::with_alpha(p.text_strong, 0.04) },
            theme::with_alpha(p.text_strong, 0.14),
            p.text,
        ),
    };
    frame(painter, r, 5.0, bg, border);
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui(11.0), if enabled { fg } else { theme::with_opacity(fg, 0.4) });
    tip(resp, tooltip.to_owned()).clicked() && enabled
}

fn stash_diff(ui: &mut Ui, body: Rect, open: &OpenStashDiff, busy: bool, acts: &mut Vec<Act>) {
    let s: &GitStash = &open.stash;
    let title = format!("stash@{{{}}}  {}", s.index, s.message);
    let (head, back, _) = header(ui, body, "stash", &title, &s.message, 0.0);
    if back {
        acts.push(Act::CloseStash);
    }
    // Second line: the actions (they do not fit next to the title).
    let bar = Rect::from_min_size(Pos2::new(body.min.x, head.max.y), Vec2::new(body.width(), 30.0));
    ui.painter().rect_filled(bar, 0.0, theme::pal().chip);
    let mut right = bar.max.x - 8.0;
    let (y, h) = (bar.min.y + 3.0, 24.0);
    if pill(ui, &mut right, y, h, t!("Delete"), t!("Deletes this stash (cannot be undone)"), Look::Danger, !busy, Id::new("git-sd-drop")) {
        acts.push(Act::AskDropStash(s.clone()));
    }
    if pill(
        ui,
        &mut right,
        y,
        h,
        t!("Apply and delete"),
        t!("Applies and removes the stash (git stash pop)"),
        Look::Normal,
        !busy,
        Id::new("git-sd-pop"),
    ) {
        acts.push(Act::StashPop(s.index));
    }
    if pill(
        ui,
        &mut right,
        y,
        h,
        t!("Apply"),
        t!("Applies and keeps the stash (git stash apply)"),
        Look::Primary,
        !busy,
        Id::new("git-sd-apply"),
    ) {
        acts.push(Act::StashApply(s.index));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, bar.max.y), body.max);
    match &open.diff {
        None => note(ui, area, t!("Loading the stash…"), theme::pal().text_2),
        Some(Err(e)) => note(ui, area, e, theme::pal().danger_text),
        Some(Ok(diff)) => diff_lines(ui, area, diff, None, acts),
    }
}

// ---------- Conflicts ----------

/// What the pair of status letters means, in words.
fn conflict_kind(index: char, worktree: char) -> &'static str {
    match (index, worktree) {
        ('U', 'U') => t!("both sides changed it"),
        ('A', 'A') => t!("both sides added it"),
        ('D', 'D') => t!("both sides deleted it"),
        ('A', 'U') | ('U', 'A') => t!("added by only one side"),
        ('D', 'U') => t!("we deleted it, they changed it"),
        ('U', 'D') => t!("we changed it, they deleted it"),
        _ => t!("in conflict"),
    }
}

pub(super) fn conflicts(deck: &mut Deck, ui: &mut Ui, body: Rect, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    if let Some(open) = deck.git.conflict_open.take() {
        conflict_file(ui, body, &open, st, busy, acts);
        deck.git.conflict_open = Some(open);
        return;
    }
    let files: Vec<_> = st.entries.iter().filter(|e| e.kind == EntryKind::Unmerged).collect();
    if files.is_empty() {
        let text = if st.state == RepoState::Normal {
            t!("No file in conflict.")
        } else {
            t!("All conflicts are resolved. Click Continue on the strip above to finish.")
        };
        return note(ui, body, text, theme::pal().text_2);
    }
    let mut child = scroll_child(ui, body);
    egui::ScrollArea::vertical().id_salt("git-conflicts").auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(6.0);
        let (r, _) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 32.0), Sense::hover());
        let hint = t!("Open each file and choose, block by block, to keep our side, theirs or both.");
        let g = ui.painter().layout(hint.to_owned(), theme::ui(11.5), theme::pal().text_2, r.width() - 28.0);
        ui.painter().galley(r.min + Vec2::new(14.0, 0.0), g, theme::pal().text_2);
        for (i, e) in files.iter().enumerate() {
            let (r, resp) = ui.allocate_exact_size(Vec2::new(ui.available_width(), 38.0), Sense::click());
            if resp.hovered() {
                ui.painter().rect_filled(r.shrink2(Vec2::new(4.0, 1.0)), 4.0, theme::pal().hover);
                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
            }
            let p = ui.painter();
            let letters = format!("{}{}", e.index, e.worktree);
            text_in(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y), Vec2::new(22.0, r.height())),
                Align2::CENTER_CENTER,
                &letters,
                theme::mono_bold(12.0),
                theme::pal().danger_text,
            );
            let w = r.width() - 56.0;
            let g = galley_trunc(p, &e.path, theme::ui(12.5), theme::pal().text, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 42.0, r.min.y + 4.0), Vec2::new(w, 17.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text,
            );
            let g = galley_trunc(p, conflict_kind(e.index, e.worktree), theme::ui(11.0), theme::pal().text_2, w);
            place(
                p,
                Rect::from_min_size(Pos2::new(r.min.x + 42.0, r.min.y + 21.0), Vec2::new(w, 14.0)),
                Align2::LEFT_CENTER,
                g,
                theme::pal().text_2,
            );
            if tip(resp, t!("{path}\nClick to resolve", path = e.path)).clicked() {
                acts.push(Act::OpenConflict(e.path.clone()));
            }
            let _ = i;
        }
    });
}

fn conflict_file(ui: &mut Ui, body: Rect, open: &OpenConflict, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) {
    let reserved = pill_w(ui, t!("Open")) + 6.0;
    let (head, back, mut right) =
        header(ui, body, "conflict", &open.path, &t!("{path}\n(Open: in the system program)", path = open.path), reserved);
    if back {
        acts.push(Act::CloseConflict);
    }
    if pill(
        ui,
        &mut right,
        head.min.y + 3.0,
        HEADER_H - 6.0,
        t!("Open"),
        t!("Opens the file in the system program"),
        Look::Normal,
        true,
        Id::new("git-cf-open"),
    ) {
        acts.push(Act::OpenFile(open.path.clone()));
    }
    let area = Rect::from_min_max(Pos2::new(body.min.x, head.max.y), body.max);
    let file = match &open.file {
        None => return note(ui, area, t!("Reading the file…"), theme::pal().text_2),
        Some(Err(e)) => return note(ui, area, e, theme::pal().danger_text),
        Some(Ok(f)) => f,
    };
    let still = st.entries.iter().find(|e| e.path == open.path && e.kind == EntryKind::Unmerged);
    let p = ui.painter_at(area);
    let (x0, x1) = (area.min.x + 12.0, area.max.x - 12.0);
    // Whole-file row: label + Ours / Theirs / Both.
    let mut y = area.min.y + 8.0;
    let remaining = file.blocks.len();
    let status_line = if file.binary {
        t!("Binary file: choose one of the sides.").to_owned()
    } else if remaining == 0 {
        match still {
            Some(e) => t!("No markers in the file ({kind}).", kind = conflict_kind(e.index, e.worktree)),
            None => t!("Resolved.").to_owned(),
        }
    } else {
        t!("{remaining} block(s) in conflict", remaining)
    };
    text_in(
        &p,
        Rect::from_min_size(Pos2::new(x0, y), Vec2::new(x1 - x0, 16.0)),
        Align2::LEFT_CENTER,
        &status_line,
        theme::ui_semibold(12.0),
        theme::pal().text,
    );
    y += 22.0;
    let g = galley_spaced(&p, t!("WHOLE FILE"), theme::label(10.0), theme::pal().text_2, 0.8);
    place(&p, Rect::from_min_size(Pos2::new(x0, y), Vec2::new(110.0, 24.0)), Align2::LEFT_CENTER, g, theme::pal().text_2);
    let mut right = x1;
    for (label, choice, tooltip) in [
        (t!("Both"), ConflictChoice::Both, t!("Joins both sides, ours first")),
        (t!("Theirs"), ConflictChoice::Theirs, t!("Keeps the other side's version (git checkout --theirs) and stages it")),
        (t!("Ours"), ConflictChoice::Ours, t!("Keeps our version (git checkout --ours) and stages it")),
    ] {
        let enabled = !busy && !(choice == ConflictChoice::Both && (file.binary || remaining == 0));
        if pill(ui, &mut right, y, 24.0, label, tooltip, Look::Normal, enabled, Id::new(("git-cf-all", label))) {
            acts.push(Act::ResolveFile(open.path.clone(), choice));
        }
    }
    y += 32.0;
    // Mark as resolved (only when no marker is left).
    let mut right = x1;
    let ready = remaining == 0 && !file.binary && still.is_some();
    let why = if ready { t!("Stages the file as it is (git add)") } else { t!("Resolve every block before marking it") };
    if pill(ui, &mut right, y, BTN_H, t!("Mark as resolved"), why, Look::Primary, ready && !busy, Id::new("git-cf-mark")) {
        acts.push(Act::Stage(vec![open.path.clone()]));
    }
    y += BTN_H + 8.0;
    p.rect_filled(Rect::from_min_size(Pos2::new(area.min.x, y), Vec2::new(area.width(), 1.0)), 0.0, theme::pal().separator);
    let list = Rect::from_min_max(Pos2::new(area.min.x, y + 1.0), area.max);
    if file.blocks.is_empty() || list.height() < 40.0 {
        return;
    }
    let mut child = scroll_child(ui, list);
    egui::ScrollArea::vertical().id_salt(("git-cf-blocks", &open.path)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(6.0);
        for (i, b) in file.blocks.iter().enumerate() {
            block_card(ui, &open.path, i, b, busy, acts);
            ui.add_space(8.0);
        }
    });
}

/// A conflict block: both sides side by side in file order and the three choices.
fn block_card(ui: &mut Ui, path: &str, index: usize, b: &ConflictBlock, busy: bool, acts: &mut Vec<Act>) {
    let side_h = |lines: &[String]| (lines.len().clamp(1, BLOCK_LINES + 1) as f32) * CODE_ROW_H + 6.0;
    let h = 24.0 + 18.0 + side_h(&b.ours) + 18.0 + side_h(&b.theirs) + 10.0 + 24.0 + 10.0;
    let w = ui.available_width() - 20.0;
    let (r, _) = ui.allocate_exact_size(Vec2::new(w + 20.0, h), Sense::hover());
    let r = Rect::from_min_size(r.min + Vec2::new(10.0, 0.0), Vec2::new(w, h));
    let p = ui.painter().clone();
    frame(&p, r, 8.0, theme::pal().recess, theme::pal().glass_border);
    text_in(
        &p,
        Rect::from_min_size(r.min + Vec2::new(10.0, 0.0), Vec2::new(w - 20.0, 24.0)),
        Align2::LEFT_CENTER,
        &t!("Block {number} · line {line}", number = index + 1, line = b.start_line),
        theme::ui_semibold(11.5),
        theme::pal().text,
    );
    let mut y = r.min.y + 24.0;
    let font = theme::mono(11.5);
    for (title, label, lines, tint) in [
        (t!("OURS"), &b.ours_label, &b.ours, theme::with_alpha(theme::pal().success, 0.12)),
        (t!("THEIRS"), &b.theirs_label, &b.theirs, theme::with_alpha(theme::pal().info, 0.14)),
    ] {
        let t = if label.is_empty() { title.to_owned() } else { format!("{title} · {label}") };
        let g = galley_spaced(&p, &t, theme::label(10.0), theme::pal().text_2, 0.8);
        place(
            &p,
            Rect::from_min_size(Pos2::new(r.min.x + 10.0, y), Vec2::new(w - 20.0, 18.0)),
            Align2::LEFT_CENTER,
            g,
            theme::pal().text_2,
        );
        y += 18.0;
        let sh = side_h(lines);
        let box_r = Rect::from_min_size(Pos2::new(r.min.x + 8.0, y), Vec2::new(w - 16.0, sh));
        p.rect_filled(box_r, 4.0, tint);
        let clip = p.with_clip_rect(box_r);
        if lines.is_empty() {
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H / 2.0),
                Align2::LEFT_CENTER,
                t!("(empty)"),
                font.clone(),
                theme::pal().text_2,
            );
        }
        for (k, line) in lines.iter().take(BLOCK_LINES).enumerate() {
            let shown: String = line.chars().take(300).collect();
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H * (k as f32 + 0.5)),
                Align2::LEFT_CENTER,
                shown,
                font.clone(),
                theme::pal().text,
            );
        }
        if lines.len() > BLOCK_LINES {
            let k = BLOCK_LINES;
            clip.text(
                Pos2::new(box_r.min.x + 8.0, box_r.min.y + 3.0 + CODE_ROW_H * (k as f32 + 0.5)),
                Align2::LEFT_CENTER,
                t!("… +{count} line(s)", count = lines.len() - BLOCK_LINES),
                font.clone(),
                theme::pal().text_2,
            );
        }
        y += sh;
    }
    y += 10.0;
    let mut right = r.max.x - 10.0;
    for (label, choice, tooltip) in [
        (t!("Both"), ConflictChoice::Both, t!("Keeps both, ours first")),
        (t!("Their side"), ConflictChoice::Theirs, t!("Keeps only theirs in this block")),
        (t!("Our side"), ConflictChoice::Ours, t!("Keeps only ours in this block")),
    ] {
        if pill(ui, &mut right, y, 24.0, label, tooltip, Look::Normal, !busy, Id::new(("git-cf-block", path, index, label))) {
            acts.push(Act::ResolveBlock(path.to_owned(), index, choice));
        }
    }
}

/// Tab of sub-views (Changes / History / Stash / Conflicts). Returns the height used.
pub(super) fn tab_strip(deck: &mut Deck, ui: &mut Ui, rect: Rect, y: f32, st: &GitStatus, acts: &mut Vec<Act>) -> f32 {
    let h = 30.0;
    let strip = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), h));
    let p = ui.painter_at(strip);
    p.rect_filled(strip, 0.0, theme::pal().chip);
    p.rect_filled(Rect::from_min_max(Pos2::new(strip.min.x, strip.max.y - 1.0), strip.max), 0.0, theme::pal().separator);
    let conflicts = crate::git_ui::conflict_count(st);
    let changes = st.entries.len();
    let mut tabs: Vec<(GitSubView, String)> = vec![
        (GitSubView::Changes, if changes > 0 { t!("Changes {changes}", changes) } else { t!("Changes").into() }),
        (GitSubView::History, t!("History").into()),
        (GitSubView::Stash, if st.stash_count > 0 { format!("Stash {}", st.stash_count) } else { "Stash".into() }),
    ];
    // Conflicts is the urgent tab: it comes first, so it is never the one cut off in a narrow bar.
    if conflicts > 0 {
        tabs.insert(0, (GitSubView::Conflicts, t!("Conflicts {conflicts}", conflicts)));
    }
    // Conflicts disappears when there is none left; the view goes back to Changes.
    if deck.git.subview == GitSubView::Conflicts && conflicts == 0 && st.state == RepoState::Normal {
        deck.git.subview = GitSubView::Changes;
    }
    let font_of = |view: GitSubView| if deck.git.subview == view { theme::ui_semibold(12.0) } else { theme::ui(12.0) };
    let texts: Vec<f32> = tabs.iter().map(|(v, l)| text_w(&p, l, font_of(*v))).collect();
    let keep = tabs.iter().position(|(v, _)| *v == deck.git.subview);
    let (pad, label_max) = super::fit_pills(&texts, &vec![0.0; tabs.len()], strip.width() - 12.0, (10.0, 16.0), 2.0, keep);
    let mut x = strip.min.x + 6.0;
    for ((view, label), label_max) in tabs.into_iter().zip(label_max) {
        let active = deck.git.subview == view;
        let danger = view == GitSubView::Conflicts;
        let color = if danger {
            theme::pal().danger_text
        } else if active {
            theme::pal().text
        } else {
            theme::pal().text_2
        };
        let font = font_of(view);
        let w = label_max + pad;
        let r = Rect::from_min_size(Pos2::new(x, strip.min.y), Vec2::new(w, h - 1.0));
        x = r.max.x + 2.0;
        let resp = clickable(ui, r, Id::new(("git-subtab", label.split(' ').next().unwrap_or(""))));
        if resp.hovered() && !active {
            p.rect_filled(r.shrink2(Vec2::new(0.0, 3.0)), 5.0, theme::pal().hover);
        }
        let fg = if resp.hovered() { theme::pal().text } else { color };
        place(&p, r, Align2::CENTER_CENTER, galley_trunc(&p, &label, font, fg, label_max + 0.5), fg);
        let resp = if label_max + 0.5 < text_w(&p, &label, font_of(view)) { tip(resp, label.clone()) } else { resp };
        if active {
            p.rect_filled(
                Rect::from_min_size(Pos2::new(r.min.x + 6.0, strip.max.y - 2.0), Vec2::new(w - 12.0, 2.0)),
                1.0,
                if danger { theme::pal().danger } else { deck.accent() },
            );
        }
        if resp.clicked() && !active {
            acts.push(Act::SetSub(view));
        }
    }
    h
}

/// Strip of the merge/rebase in progress, with Continue and Abort. Returns the height used.
pub(super) fn op_banner(ui: &mut Ui, rect: Rect, y: f32, st: &GitStatus, busy: bool, acts: &mut Vec<Act>) -> f32 {
    let (name, noun) = match st.state {
        RepoState::Normal => return 0.0,
        RepoState::Merging => (t!("Merging"), t!("the merge")),
        RepoState::Rebasing => ("Rebase", t!("the rebase")),
        RepoState::CherryPicking => ("Cherry-pick", t!("the cherry-pick")),
        RepoState::Reverting => ("Revert", t!("the revert")),
    };
    let h = 32.0;
    let r = Rect::from_min_size(Pos2::new(rect.min.x, y), Vec2::new(rect.width(), h));
    let p = ui.painter_at(r);
    let warn = theme::pal().warning;
    p.rect_filled(r, 0.0, theme::with_opacity(warn, 0.10));
    p.rect_filled(Rect::from_min_size(r.min, Vec2::new(3.0, h)), 0.0, warn);
    let conflicts = crate::git_ui::conflict_count(st);
    let mut right = r.max.x - 8.0;
    let top = r.min.y + 4.0;
    if pill(
        ui,
        &mut right,
        top,
        h - 8.0,
        t!("Abort"),
        &t!("Cancels {noun} and goes back to the previous state", noun),
        Look::Danger,
        !busy,
        Id::new("git-op-abort"),
    ) {
        acts.push(Act::AskAbort(st.state));
    }
    let can = !busy && conflicts == 0;
    let why = if conflicts > 0 { t!("Resolve the conflicts before continuing").to_owned() } else { t!("Finishes {noun}", noun) };
    if pill(ui, &mut right, top, h - 8.0, t!("Continue"), &why, Look::Primary, can, Id::new("git-op-continue")) {
        acts.push(Act::Continue);
    }
    let text = if conflicts > 0 { t!("{name} · {conflicts} conflict(s)", name, conflicts) } else { t!("{name} in progress", name) };
    let tr = Rect::from_min_max(Pos2::new(r.min.x + 12.0, r.min.y), Pos2::new(right, r.max.y));
    let g = galley_trunc(&p, &text, theme::ui_semibold(12.0), warn, tr.width());
    place(&p, tr, Align2::LEFT_CENTER, g, warn);
    tip(ui.interact(tr, Id::new("git-op-text"), Sense::hover()), text);
    h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn conflict_letters_are_explained() {
        assert_eq!(conflict_kind('U', 'U'), "both sides changed it");
        assert_eq!(conflict_kind('D', 'U'), "we deleted it, they changed it");
        assert_eq!(conflict_kind('U', 'D'), "we changed it, they deleted it");
        assert_eq!(conflict_kind('?', '?'), "in conflict");
    }
}

//! Documents mode of the center (see `docs.rs`): the Agents | Documents switch of the title bar, the strip of
//! document tabs (the same drawing as `tabs.rs`) and the document with the full width of the center.
//! Diff, text file (which can also be edited) and markdown.

use egui::{Align2, Id, Key, Painter, Pos2, Rect, Sense, Ui, Vec2};

use super::panes::HEAD_H;
use super::tabs::{self, Item, Lead, StripClick, StripKind};
use super::{
    button, button_width, clickable, code, diff_view, frame, galley, galley_trunc, place, section_label, text_in, text_w, tip, ButtonKind,
};
use agentdeck_core::t;

use crate::deck::Deck;
use crate::docs::{file_name, After, Answer, CenterView, DiffDoc, DiffFile, DocKind, DocState, DocTab, Find, Unsaved};
use crate::highlight::Lang;
use crate::theme;

/// Height of the line with the document title and its actions.
const SUBHEAD_H: f32 = 28.0;
/// Height of a row of a diff's file list and of its header.
const FILE_ROW_H: f32 = 20.0;
const FILE_HEAD_H: f32 = super::SECTION_LABEL_H;
/// Below this width the diff's file list moves above the diff.
const NARROW_W: f32 = 560.0;
/// Maximum width of the markdown column (above it the text gets too long to read).
const MD_MAX_W: f32 = 820.0;

// ---------------------------------------------------------------------------
// The switch
// ---------------------------------------------------------------------------

/// What a click on the switch asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switched {
    Agents,
    Docs,
}

/// Height of the switch rail (24 px segments and 2 px on each side).
pub const SWITCH_H: f32 = 28.0;
/// The compact switch of the title bar (sidebar collapsed): 20 px segments.
pub const SWITCH_COMPACT_H: f32 = 24.0;
/// The "Agents" badge: a 6 px dot, 6 px after the name.
const SELO: f32 = 6.0;
const SELO_GAP: f32 = 6.0;

/// Width of the compact switch: two equal segments, the size of the largest label.
pub fn switch_compact_width(painter: &Painter, deck: &Deck) -> f32 {
    let font = theme::ui_semibold(11.5);
    let agents = text_w(painter, t!("Agents"), font.clone()) + SELO_GAP + SELO;
    let docs = text_w(painter, &docs_label(deck.docs.count(&deck.active_project)), font);
    2.0 * (agents.max(docs) + 20.0) + 4.0
}

/// The word on the second side of the switch: whole if it and the count (`full_w`) fit in the segment of width
/// `seg_w` with room to spare, abbreviated if not.
fn docs_word(full_w: f32, seg_w: f32) -> &'static str {
    if full_w + 8.0 <= seg_w {
        t!("Documents")
    } else {
        t!("Docs")
    }
}

/// "Documents n"; with no document open, only "Documents" (the zero is noise).
fn docs_label(n: usize) -> String {
    if n == 0 {
        t!("Documents").to_owned()
    } else {
        t!("Documents {n}", n)
    }
}

/// Paints the **Agents | Documents n** switch in `rect` (the rail, `SWITCH_H` or `SWITCH_COMPACT_H`
/// high): the active segment in the accent color. The "Agents" badge shows what happens in the tabs
/// while reading: a dot if one finished a turn without being seen (in the text-on-accent color
/// if the side is active, in the agent's color if not) and, while reading documents, one that pulses if one is working
/// now. A click switches the center.
pub fn switch(deck: &mut Deck, ui: &Ui, rect: Rect, id: &str) {
    let painter = ui.painter();
    let p = theme::pal();
    let n_docs = deck.docs.count(&deck.active_project);
    let badge = deck.agents_badge();
    let view = deck.docs.view;
    let accent = theme::accent();
    let docs_enabled = n_docs > 0;
    let pulse = badge.done == 0 && badge.busy > 0 && view == CenterView::Docs;

    painter.rect_filled(rect, 7.0, p.recess);
    let inner = rect.shrink(2.0);
    let w = inner.width() / 2.0;
    let mut picked = None;
    for (i, which) in [Switched::Agents, Switched::Docs].into_iter().enumerate() {
        let seg = Rect::from_min_size(Pos2::new(inner.min.x + w * i as f32, inner.min.y), Vec2::new(w, inner.height()));
        let enabled = which == Switched::Agents || docs_enabled;
        let resp = if enabled { clickable(ui, seg, Id::new((id, i))) } else { ui.interact(seg, Id::new((id, i)), Sense::hover()) };
        let active = (which == Switched::Agents) == (view == CenterView::Agents);
        let (fill, fg, font) = if active {
            (Some(accent), p.on_accent, theme::ui_semibold(11.5))
        } else if resp.hovered() && enabled {
            (Some(p.hover), p.text, theme::ui(11.5))
        } else if enabled {
            (None, p.text_2, theme::ui(11.5))
        } else {
            (None, theme::with_opacity(p.text_2, 0.5), theme::ui(11.5))
        };
        if let Some(fill) = fill {
            painter.rect_filled(seg, 5.0, fill);
        }
        if which == Switched::Agents {
            let has_selo = badge.done > 0 || pulse;
            let g = galley(painter, t!("Agents"), font, fg);
            let total = g.size().x + if has_selo { SELO_GAP + SELO } else { 0.0 };
            let x = seg.center().x - total / 2.0;
            let text = place(
                painter,
                Rect::from_min_size(Pos2::new(x, seg.min.y), Vec2::new(g.size().x, seg.height())),
                Align2::LEFT_CENTER,
                g,
                fg,
            );
            let c = Pos2::new(text.max.x + SELO_GAP + SELO / 2.0, seg.center().y);
            let selo = if active { p.on_accent } else { accent };
            if badge.done > 0 {
                painter.circle_filled(c, SELO / 2.0, selo);
            } else if pulse {
                let t = ui.ctx().input(|i| i.time);
                let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
                painter.circle_filled(c, SELO / 2.0 + 3.0 * wave, theme::with_opacity(selo, 0.35 * (1.0 - wave)));
                painter.circle_filled(c, SELO / 2.0, selo);
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
            }
        } else {
            // "Documents" (or "Docs", in the narrowest sidebar) and the count, this one in `text_3`
            // (dimmed over the accent); with no document open, no count.
            let count = if n_docs > 0 { n_docs.to_string() } else { String::new() };
            let count_gap = if n_docs > 0 { 5.0 } else { 0.0 };
            let count_w = text_w(painter, &count, theme::ui(11.5));
            let label = docs_word(text_w(painter, t!("Documents"), font.clone()) + count_gap + count_w, seg.width());
            let g = galley(painter, label, font, fg);
            let count_color = if active {
                theme::with_opacity(p.on_accent, 0.7)
            } else if enabled {
                p.text_3
            } else {
                fg
            };
            let cg = galley(painter, &count, theme::ui(11.5), count_color);
            let total = g.size().x + count_gap + cg.size().x;
            let x = seg.center().x - total / 2.0;
            let gw = g.size().x;
            place(painter, Rect::from_min_size(Pos2::new(x, seg.min.y), Vec2::new(gw, seg.height())), Align2::LEFT_CENTER, g, fg);
            let cw = cg.size().x;
            place(
                painter,
                Rect::from_min_size(Pos2::new(x + gw + count_gap, seg.min.y), Vec2::new(cw, seg.height())),
                Align2::LEFT_CENTER,
                cg,
                count_color,
            );
        }
        let tip_text = switch_tip(which, n_docs, badge.done, badge.busy);
        if tip(resp, tip_text).clicked() && enabled {
            picked = Some(which);
        }
    }
    match picked {
        Some(Switched::Agents) => deck.show_agents(),
        Some(Switched::Docs) => deck.docs.view = CenterView::Docs,
        None => {}
    }
}

/// The tooltip of each side of the switch (with the shortcut).
fn switch_tip(which: Switched, n_docs: usize, done: usize, busy: usize) -> String {
    match which {
        Switched::Agents => {
            let mut parts = vec![t!("Agents: the terminal windows").to_owned()];
            if done > 0 {
                parts.push(t!("{done} finished without you seeing", done));
            }
            if busy > 0 {
                parts.push(t!("{busy} working", busy));
            }
            parts.push(t!("Ctrl+Shift+D toggles").to_owned());
            parts.join("\n")
        }
        Switched::Docs if n_docs == 0 => {
            t!("Documents: nothing open in this project\nOpen a diff from the tasks desk, a commit from the Git tab\nor a file (double-click in the explorer)").to_owned()
        }
        Switched::Docs => t!("Documents: diffs, files and markdown, to read\nCtrl+Shift+D toggles").to_owned(),
    }
}

// ---------------------------------------------------------------------------
// The center in Documents mode
// ---------------------------------------------------------------------------

/// What a click on the document asks for (applied after painting).
enum Act {
    Focus(u64),
    Close(u64),
    Reload(u64),
    OpenExternal(String),
    SelectFile(u64, usize),
    Save(u64),
}

/// Paints the documents of the active project in place of the agent windows.
pub fn draw(deck: &mut Deck, ui: &mut Ui, area: Rect) {
    let painter = ui.painter_at(area);
    let project = deck.active_project.clone();
    let accent = deck.accent();
    let items: Vec<Item> = deck
        .docs
        .of_project(&project)
        .map(|t| Item {
            uid: t.uid,
            label: t.title.clone(),
            color: accent,
            alive: true,
            busy: false,
            done: false,
            mic: crate::dictation::ButtonVisual::Idle,
            lead: Lead::Glyph(glyph(&t.kind)),
            tip: t.subtitle.clone(),
            dirty: t.dirty(),
        })
        .collect();
    let uids: Vec<u64> = items.iter().map(|i| i.uid).collect();
    let Some(current) = deck.docs.current_uid(&project).and_then(|c| uids.iter().position(|u| *u == c)) else {
        text_in(&painter, area, Align2::CENTER_CENTER, t!("No document open in this project."), theme::ui(13.0), theme::pal().text_2);
        return;
    };

    // ---------- Tab strip ----------
    let head = Rect::from_min_size(area.min, Vec2::new(area.width(), HEAD_H));
    painter.rect_filled(head, 0.0, theme::pal().strip);
    painter.rect_filled(Rect::from_min_max(Pos2::new(head.min.x, head.max.y - 1.0), head.max), 0.0, theme::pal().separator);
    let strip_rect = Rect::from_min_max(head.min, Pos2::new(head.max.x, head.max.y - 1.0));
    let mut acts = Vec::new();
    match tabs::strip_items(
        ui,
        strip_rect,
        &items,
        current,
        true,
        Id::new("doc-tabs"),
        StripKind { scope: "doc-tab", tab_menu: false, colored: false },
    ) {
        Some(StripClick::Activate(n)) => acts.push(Act::Focus(uids[n])),
        Some(StripClick::Close(n)) => acts.push(Act::Close(uids[n])),
        _ => {}
    }

    // ---------- Focused document ----------
    let uid = uids[current];
    let below = Rect::from_min_max(Pos2::new(area.min.x, head.max.y), area.max);
    let mut cache = std::mem::take(&mut deck.docs.md_cache);
    if let Some(tab) = deck.docs.get_mut(uid) {
        document(ui, below, tab, &mut cache, &mut acts);
    }
    deck.docs.md_cache = cache;

    for act in acts {
        match act {
            Act::Focus(uid) => deck.docs.focus(uid),
            Act::Close(uid) => deck.docs_request_close(uid),
            Act::Reload(uid) => deck.docs_reload(uid),
            Act::Save(uid) => deck.docs_save(uid, After::Nothing),
            Act::OpenExternal(rel) => {
                if let Err(e) = deck.core.open_path(&rel) {
                    deck.system(t!("Could not open {rel} in the default program: {error}", rel, error = e));
                }
            }
            Act::SelectFile(uid, n) => {
                if let Some(DocState::Diff(d)) = deck.docs.get_mut(uid).map(|t| &mut t.state) {
                    d.select(n);
                }
            }
        }
    }
}

/// The sign of the document type, before the tab name.
fn glyph(kind: &DocKind) -> &'static str {
    match kind {
        DocKind::TaskDiff { .. } | DocKind::CommitDiff(_) | DocKind::FileDiff { .. } => "±",
        DocKind::File(_) => "≡",
        DocKind::Markdown(_) => "¶",
    }
}

/// The title line of the document (with the actions), the notices and the search; then, its body.
fn document(ui: &mut Ui, area: Rect, tab: &mut DocTab, cache: &mut egui_commonmark::CommonMarkCache, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let sub = Rect::from_min_size(area.min, Vec2::new(area.width(), SUBHEAD_H));
    painter.rect_filled(Rect::from_min_max(Pos2::new(sub.min.x, sub.max.y - 1.0), sub.max), 0.0, theme::pal().separator);
    let uid = tab.uid;

    // Actions on the right, from the last to the first.
    let mut right = sub.max.x - 8.0;
    let cy = sub.min.y + (SUBHEAD_H - 1.0) / 2.0;
    if let Some(path) = tab.kind.file_path() {
        let label = t!("Open in the default program");
        if action(
            ui,
            &painter,
            &mut right,
            cy,
            (label, t!("Opens the file in the program the system associates with it")),
            Id::new(("doc-open", uid)),
            (ButtonKind::Secondary, true),
        ) {
            acts.push(Act::OpenExternal(path.to_owned()));
        }
    }
    if tab.kind.reloadable() {
        let loading = matches!(tab.state, DocState::Loading);
        let tooltip = t!("Reads the document again (with an unsaved edit, it asks first)");
        if action(ui, &painter, &mut right, cy, (t!("Reload"), tooltip), Id::new(("doc-reload", uid)), (ButtonKind::Secondary, !loading)) {
            acts.push(Act::Reload(uid));
        }
    }
    let mut dirty = false;
    if let DocState::Source(src) = &mut tab.state {
        dirty = src.dirty();
        let is_md = matches!(tab.kind, DocKind::Markdown(_));
        if src.can_edit() && src.editing {
            let hint = t!("Stops editing (what you typed stays on screen until you save or close)");
            if action(ui, &painter, &mut right, cy, (t!("Read-only"), hint), Id::new(("doc-stop", uid)), (ButtonKind::Secondary, true)) {
                src.editing = false;
            }
            let ready = src.dirty() && src.external.is_none() && !src.saving;
            let label = if src.saving { t!("Saving…") } else { t!("Save") };
            let hint = if src.external.is_some() {
                t!("First resolve the notice: the file changed on disk")
            } else {
                t!("Saves the file (Ctrl+S)")
            };
            if action(ui, &painter, &mut right, cy, (label, hint), Id::new(("doc-save", uid)), (ButtonKind::Primary, ready)) {
                acts.push(Act::Save(uid));
            }
        } else if src.can_edit() {
            let hint = t!("Edits the file right here (Ctrl+S saves)");
            if action(ui, &painter, &mut right, cy, (t!("Edit"), hint), Id::new(("doc-edit", uid)), (ButtonKind::Primary, true)) {
                src.set_editing(true);
            }
        }
        if is_md && !src.editing {
            let (label, hint) = if src.raw {
                (t!("View formatted"), t!("Shows the formatted markdown"))
            } else {
                (t!("View code"), t!("Shows the markdown text, with highlighting"))
            };
            if action(ui, &painter, &mut right, cy, (label, hint), Id::new(("doc-raw", uid)), (ButtonKind::Secondary, true)) {
                src.raw = !src.raw;
            }
        }
    }
    let title_r = Rect::from_min_max(Pos2::new(sub.min.x + 14.0, sub.min.y), Pos2::new(right - 8.0, sub.max.y - 1.0));
    if title_r.width() > 20.0 {
        let mut title = title_r;
        if let DocState::Source(src) = &tab.state {
            if src.lang != Lang::Plain {
                let g = galley(&painter, src.lang.label(), theme::meta(10.5), theme::pal().text_3);
                let w = g.size().x;
                place(
                    &painter,
                    Rect::from_min_size(Pos2::new(title.max.x - w, title.min.y), Vec2::new(w, title.height())),
                    Align2::LEFT_CENTER,
                    g,
                    theme::pal().text_3,
                );
                title.max.x -= w + 12.0;
            }
        }
        if dirty {
            let g = galley(&painter, t!("● Unsaved"), theme::ui(11.5), theme::pal().warning);
            let w = g.size().x;
            place(
                &painter,
                Rect::from_min_size(Pos2::new(title.max.x - w, title.min.y), Vec2::new(w, title.height())),
                Align2::LEFT_CENTER,
                g,
                theme::pal().warning,
            );
            title.max.x -= w + 10.0;
        }
        let g = galley_trunc(&painter, &tab.subtitle, theme::ui(12.0), theme::pal().text, title.width().max(10.0));
        place(&painter, title, Align2::LEFT_CENTER, g, theme::pal().text);
    }

    let mut body = Rect::from_min_max(Pos2::new(area.min.x, sub.max.y), area.max);
    notices(ui, &mut body, tab);
    if tab.find.open {
        find_bar(ui, &mut body, uid, &mut tab.find);
    }
    let is_md = matches!(tab.kind, DocKind::Markdown(_));
    let diff_path = tab.kind.diff_path().map(str::to_owned);
    let DocTab { state, find, shaped, .. } = tab;
    match state {
        DocState::Loading => note(&painter, body, t!("Loading…"), theme::pal().text_2),
        DocState::Failed(e) => note(&painter, body, e, theme::pal().danger_text),
        DocState::Unreadable(why) => note(&painter, body, why, theme::pal().text_2),
        DocState::Diff(doc) => diff_body(ui, body, uid, doc, find, acts, diff_path.as_deref()),
        DocState::Text(doc) => code::text_body(ui, body, uid, doc, find),
        DocState::Source(src) if is_md && !src.raw && !src.editing => markdown_body(ui, body, uid, &src.text, cache),
        DocState::Source(src) => code::source_body(ui, body, uid, src, find, shaped),
        DocState::Markdown(text) => markdown_body(ui, body, uid, text, cache),
    }
}

/// A text button at the end of the title line; `right` moves left. Returns whether it was clicked (a
/// disabled button shows dimmed and does not click).
fn action(
    ui: &Ui,
    painter: &Painter,
    right: &mut f32,
    cy: f32,
    (label, tooltip): (&str, &str),
    id: Id,
    (kind, enabled): (ButtonKind, bool),
) -> bool {
    let w = button_width(painter, label, kind);
    let r = Rect::from_min_size(Pos2::new(*right - w, cy - 11.0), Vec2::new(w, 22.0));
    *right = r.min.x - 6.0;
    if enabled {
        return tip(button(ui, r, id, label, kind), tooltip).clicked();
    }
    // Disabled: the same drawing at 45%, no click.
    painter.rect_filled(r, 5.0, theme::with_alpha(theme::pal().text_strong, 0.03));
    text_in(painter, r, Align2::CENTER_CENTER, label, theme::ui(11.5), theme::with_opacity(theme::pal().text_2, 0.55));
    tip(ui.interact(r, id, Sense::hover()), tooltip);
    false
}

/// A message at the top of the body (loading, error, file that does not become text), with line wrapping.
pub(super) fn note(painter: &Painter, body: Rect, text: &str, color: egui::Color32) {
    let g = painter.layout(text.to_owned(), theme::ui(12.5), color, (body.width() - 28.0).max(40.0));
    painter.galley(body.min + Vec2::splat(14.0), g, color);
}

// ---------------------------------------------------------------------------
// Notices and search
// ---------------------------------------------------------------------------

/// Height of a notice strip and of the search bar.
const NOTICE_H: f32 = 30.0;
const FIND_H: f32 = 34.0;

/// What a notice button asks for.
#[derive(Clone, Copy)]
enum Fix {
    KeepMine,
    TakeDisk,
    Dismiss,
}

/// A notice strip at the top of `body` (which shrinks): the text, in the tone color, and the buttons on the right. Returns the clicked button.
fn notice(ui: &Ui, body: &mut Rect, tone: egui::Color32, text: &str, buttons: &[(&str, &str, Fix)], key: Id) -> Option<Fix> {
    let r = Rect::from_min_size(body.min, Vec2::new(body.width(), NOTICE_H));
    body.min.y += NOTICE_H;
    let painter = ui.painter_at(r);
    painter.rect_filled(r, 0.0, theme::with_alpha(tone, 0.10));
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - 1.0), r.max), 0.0, theme::pal().separator);
    let mut right = r.max.x - 8.0;
    let mut clicked = None;
    for (n, (label, hint, fix)) in buttons.iter().enumerate().rev() {
        let w = button_width(&painter, label, ButtonKind::Secondary);
        let b = Rect::from_min_size(Pos2::new(right - w, r.center().y - 11.0), Vec2::new(w, 22.0));
        right = b.min.x - 6.0;
        if tip(button(ui, b, key.with(n), label, ButtonKind::Secondary), *hint).clicked() {
            clicked = Some(*fix);
        }
    }
    let text_r = Rect::from_min_max(Pos2::new(r.min.x + 14.0, r.min.y), Pos2::new(right - 6.0, r.max.y - 1.0));
    let g = galley_trunc(&painter, text, theme::ui(12.0), theme::pal().text, text_r.width().max(10.0));
    let shown = place(&painter, text_r, Align2::LEFT_CENTER, g, theme::pal().text);
    painter.circle_filled(Pos2::new(shown.min.x - 8.0, r.center().y - 0.5), 3.0, tone);
    tip(ui.interact(text_r, key.with("text"), Sense::hover()), text.to_owned());
    clicked
}

/// The notices of the file opened as code (and of the one that only opened read-only), from the most important to the least.
fn notices(ui: &Ui, body: &mut Rect, tab: &mut DocTab) {
    let p = theme::pal();
    let key = Id::new(("doc-notice", tab.uid));
    match &mut tab.state {
        DocState::Source(src) => {
            let fix = if src.external.is_some() {
                notice(
                    ui,
                    body,
                    p.warning,
                    t!("The file changed on disk (an agent?) while you were editing."),
                    &[
                        (t!("Keep mine"), t!("Keeps what you typed; the next save writes over what is on disk"), Fix::KeepMine),
                        (t!("Reload from disk"), t!("Throws away what you typed and shows what is on disk"), Fix::TakeDisk),
                    ],
                    key.with("external"),
                )
            } else if src.gone {
                notice(
                    ui,
                    body,
                    p.warning,
                    t!("The file was deleted or does not open any more. What you edited stays here; Ctrl+S saves it again."),
                    &[],
                    key.with("gone"),
                )
            } else {
                None
            };
            match fix {
                Some(Fix::KeepMine) => src.keep_mine(),
                Some(Fix::TakeDisk) => src.take_disk(),
                _ => {}
            }
            if let Some(e) = src.save_error.clone() {
                if let Some(Fix::Dismiss) = notice(
                    ui,
                    body,
                    p.danger,
                    &t!("Could not save: {error}", error = e),
                    &[(t!("Close"), t!("Hides the error"), Fix::Dismiss)],
                    key.with("error"),
                ) {
                    src.save_error = None;
                }
            }
            if let Some(why) = &src.locked {
                notice(ui, body, p.text_2, why, &[], key.with("locked"));
            }
        }
        DocState::Text(_) => {
            let why = t!("Opened read-only: the file is large or has a line too long for the editor.");
            notice(ui, body, p.text_2, why, &[], key.with("large"));
        }
        _ => {}
    }
}

/// The search bar (Ctrl+F): the field, "n of m", previous, next and close. Enter goes to the next one (Shift+Enter, the previous); Esc closes.
fn find_bar(ui: &mut Ui, body: &mut Rect, uid: u64, find: &mut Find) {
    let r = Rect::from_min_size(body.min, Vec2::new(body.width(), FIND_H));
    body.min.y += FIND_H;
    let p = theme::pal();
    let painter = ui.painter_at(r);
    painter.rect_filled(Rect::from_min_max(Pos2::new(r.min.x, r.max.y - 1.0), r.max), 0.0, p.separator);
    let field = Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y + 5.0), Vec2::new(240.0f32.min(r.width() - 24.0), 24.0));
    let id = Id::new(("doc-find", uid));
    let focused = ui.ctx().memory(|m| m.has_focus(id));
    frame(&painter, field, 6.0, p.recess, if focused { theme::accent() } else { p.separator });
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(field.shrink2(Vec2::new(8.0, 3.0))));
    let out = egui::TextEdit::singleline(&mut find.query)
        .id(id)
        .frame(egui::Frame::NONE)
        .font(theme::ui(12.0))
        .hint_text(t!("Search in the document"))
        .text_color(p.text)
        .desired_width(field.width() - 16.0)
        .show(&mut child);
    if std::mem::take(&mut find.focus) {
        out.response.response.request_focus();
    }
    let (enter, shift, esc) = ui.input(|i| (i.key_pressed(Key::Enter), i.modifiers.shift, i.key_pressed(Key::Escape)));
    if out.response.response.lost_focus() && enter {
        find.step(shift);
        out.response.response.request_focus();
    } else if focused && esc {
        find.close();
        return;
    }
    let n = find.count();
    let counter = match (find.query.is_empty(), n) {
        (true, _) => String::new(),
        (false, 0) => t!("No results").to_owned(),
        (false, n) => t!("{current} of {n}", current = find.current.min(n - 1) + 1, n),
    };
    let mut x = field.max.x + 10.0;
    if !counter.is_empty() {
        let w = text_w(&painter, &counter, theme::meta(10.5));
        text_in(
            &painter,
            Rect::from_min_size(Pos2::new(x, r.min.y), Vec2::new(w, r.height() - 1.0)),
            Align2::LEFT_CENTER,
            &counter,
            theme::meta(10.5),
            p.text_2,
        );
        x += w + 10.0;
    }
    for (label, hint, act) in
        [("‹", t!("Previous match (Shift+Enter)"), 0), ("›", t!("Next match (Enter)"), 1), (t!("Close"), t!("Closes the search (Esc)"), 2)]
    {
        let w = if act == 2 { button_width(&painter, label, ButtonKind::Ghost) } else { 24.0 };
        let b = Rect::from_min_size(Pos2::new(x, r.min.y + 5.0), Vec2::new(w, 24.0));
        x += w + 4.0;
        if x > r.max.x {
            break;
        }
        if tip(button(ui, b, Id::new(("doc-find-btn", uid, act)), label, ButtonKind::Ghost), hint).clicked() {
            match act {
                0 => find.step(true),
                1 => find.step(false),
                _ => find.close(),
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The question about an unsaved edit
// ---------------------------------------------------------------------------

/// The "Save the changes?" modal: when closing a tab with an edit, when reloading over it or when closing the app.
pub fn unsaved_modal(deck: &mut Deck, ctx: &egui::Context) {
    let Some(asked) = deck.docs.confirm else { return };
    let name_of = |uid: u64| deck.docs.get(uid).map_or_else(|| t!("the file").to_owned(), |t| t.subtitle.clone());
    let (title, detail, buttons): (String, String, Vec<(&str, Answer)>) = match asked {
        Unsaved::Tab(uid) => (
            t!("Save the changes to {name}?", name = name_of(uid)),
            t!("If you do not save, what you typed is lost.").to_owned(),
            vec![(t!("Save"), Answer::Save), (t!("Don't save"), Answer::Discard), (t!("Cancel"), Answer::Cancel)],
        ),
        Unsaved::Revert(uid) => (
            t!("Discard the changes to {name}?", name = name_of(uid)),
            t!("The file is read from disk again; what you typed is lost.").to_owned(),
            vec![(t!("Discard"), Answer::Discard), (t!("Cancel"), Answer::Cancel)],
        ),
        Unsaved::Busy => {
            let tabs = deck.working_tabs();
            let shown = tabs.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let more = if tabs.len() > 4 { t!(" and {count} more", count = tabs.len() - 4) } else { String::new() };
            (
                t!("Agents are working").to_owned(),
                t!("{shown}{more}. If you quit now, the turn in progress is lost.", shown, more),
                vec![(t!("Quit anyway"), Answer::Discard), (t!("Cancel"), Answer::Cancel)],
            )
        }
        Unsaved::Quit => {
            let files: Vec<String> = deck.docs.unsaved().iter().map(|t| t.subtitle.clone()).collect();
            let shown = files.iter().take(4).cloned().collect::<Vec<_>>().join(", ");
            let more = if files.len() > 4 { t!(" and {count} more", count = files.len() - 4) } else { String::new() };
            (
                t!("There are unsaved changes").to_owned(),
                t!("{shown}{more}. If you quit without saving, what you typed is lost.", shown, more),
                vec![(t!("Save all and quit"), Answer::Save), (t!("Quit without saving"), Answer::Discard), (t!("Cancel"), Answer::Cancel)],
            )
        }
    };
    super::modal_backdrop(deck, ctx, "docs-unsaved-backdrop");
    let p = theme::pal();
    let w = 420.0;
    let mut answer = None;
    let screen = ctx.content_rect();
    egui::Area::new(Id::new("docs-unsaved"))
        .order(egui::Order::Tooltip)
        .fixed_pos(Pos2::new(screen.center().x - w / 2.0, screen.min.y + 120.0))
        .constrain(false)
        .show(ctx, |ui| {
            let g1 = ui.painter().layout(title.clone(), theme::ui_semibold(13.0), p.text_strong, w - 28.0);
            let g2 = ui.painter().layout(detail.clone(), theme::ui(12.0), p.text_2, w - 28.0);
            let btn_h = 28.0;
            let h = 14.0 + g1.size().y + 6.0 + g2.size().y + 14.0 + btn_h + 14.0;
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::click());
            super::shadow(ui.painter(), rect, 10, 8, 24, p.shadow);
            frame(ui.painter(), rect, 10.0, p.popup, p.glass_border);
            ui.painter().galley(Pos2::new(rect.min.x + 14.0, rect.min.y + 14.0), g1.clone(), p.text_strong);
            ui.painter().galley(Pos2::new(rect.min.x + 14.0, rect.min.y + 14.0 + g1.size().y + 6.0), g2, p.text_2);
            let mut right = rect.max.x - 14.0;
            for (n, (label, which)) in buttons.iter().enumerate().rev() {
                let kind = if *which == Answer::Save { ButtonKind::Primary } else { ButtonKind::Secondary };
                let bw = button_width(ui.painter(), label, kind);
                let b = Rect::from_min_size(Pos2::new(right - bw, rect.max.y - 14.0 - btn_h), Vec2::new(bw, btn_h));
                right = b.min.x - 8.0;
                let id = Id::new(("docs-unsaved-btn", n));
                let resp = if *which == Answer::Discard { super::button_danger(ui, b, id, label) } else { button(ui, b, id, label, kind) };
                if resp.clicked() {
                    answer = Some(*which);
                }
            }
            if ui.input(|i| i.key_pressed(Key::Escape)) {
                answer = Some(Answer::Cancel);
            }
        });
    if let Some(a) = answer {
        deck.docs_answer(a);
    }
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

fn diff_body(ui: &mut Ui, body: Rect, uid: u64, doc: &mut DiffDoc, find: &mut Find, acts: &mut Vec<Act>, path: Option<&str>) {
    let id = Id::new(("doc-diff", uid));
    if find.open {
        let version = doc.diff.lines.len() as u64;
        find.refresh(version, |q| (Vec::new(), diff_view::search(&doc.diff, q)));
    }
    let current = find.current_line();
    let mut jump = doc.jump.take();
    if std::mem::take(&mut find.jump) {
        jump = current.or(jump);
    }
    let query = if find.open { find.query.as_str() } else { "" };
    let opts = diff_view::View { path, query, found: &find.lines, current: current.filter(|_| find.open), jump };
    if doc.files.is_empty() {
        super::git::diff_lines_at(ui, body, &doc.diff, None, &mut Vec::new(), id, opts);
        return;
    }
    let narrow = body.width() < NARROW_W;
    let (list, view) = if narrow {
        let h = (FILE_HEAD_H + FILE_ROW_H * doc.files.len() as f32).min(body.height() * 0.35).max(FILE_HEAD_H + FILE_ROW_H);
        let list = Rect::from_min_size(body.min, Vec2::new(body.width(), h));
        (list, Rect::from_min_max(Pos2::new(body.min.x, list.max.y + 1.0), body.max))
    } else {
        let w = (body.width() * 0.28).clamp(190.0, 320.0);
        let list = Rect::from_min_size(body.min, Vec2::new(w, body.height()));
        (list, Rect::from_min_max(Pos2::new(list.max.x + 1.0, body.min.y), body.max))
    };
    let painter = ui.painter_at(body);
    let sep = if narrow {
        Rect::from_min_size(Pos2::new(list.min.x, list.max.y), Vec2::new(list.width(), 1.0))
    } else {
        Rect::from_min_size(Pos2::new(list.max.x, list.min.y), Vec2::new(1.0, list.height()))
    };
    painter.rect_filled(sep, 0.0, theme::pal().separator);
    file_list(ui, list, uid, doc, acts);
    super::git::diff_lines_at(ui, view, &doc.diff, None, &mut Vec::new(), id, opts);
}

/// The diff's file list: letter of what changed, name and `+`/`−`. Clicking scrolls the diff to the file.
fn file_list(ui: &mut Ui, area: Rect, uid: u64, doc: &DiffDoc, acts: &mut Vec<Act>) {
    let painter = ui.painter_at(area);
    let (added, removed) = doc.files.iter().fold((0, 0), |(a, r), f| (a + f.added, r + f.removed));
    let title = t!("{count} file(s)", count = doc.files.len());
    let head = section_label(&painter, Rect::from_min_size(area.min, Vec2::new(area.width(), FILE_HEAD_H)), &title);
    let totals = format!("+{added} −{removed}");
    text_in(&painter, head.shrink2(Vec2::new(12.0, 0.0)), Align2::RIGHT_CENTER, &totals, theme::meta(10.0), theme::pal().text_2);

    let rows = Rect::from_min_max(Pos2::new(area.min.x, head.max.y), area.max);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rows).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(rows);
    egui::ScrollArea::vertical().id_salt(("doc-files", uid)).auto_shrink([false, false]).show_rows(
        &mut child,
        FILE_ROW_H,
        doc.files.len(),
        |ui, range| {
            for n in range {
                let file = &doc.files[n];
                let (r, _) = ui.allocate_exact_size(Vec2::new(rows.width(), FILE_ROW_H), Sense::hover());
                let resp = clickable(ui, r, Id::new(("doc-file", uid, n)));
                file_row(ui.painter(), r, file, n == doc.selected, resp.hovered());
                if tip(resp, t!("{path}\nClick to go to it in the diff", path = file.path)).clicked() {
                    acts.push(Act::SelectFile(uid, n));
                }
            }
        },
    );
}

fn file_row(p: &Painter, r: Rect, file: &DiffFile, selected: bool, hovered: bool) {
    let pal = theme::pal();
    if selected {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, pal.selected_soft);
    } else if hovered {
        p.rect_filled(r.shrink2(Vec2::new(4.0, 0.0)), 4.0, pal.hover);
    }
    let letter_color = match file.status {
        'A' => pal.success,
        'D' => pal.danger_text,
        'R' => pal.info,
        _ => pal.warning,
    };
    let letter = Rect::from_min_size(Pos2::new(r.min.x + 12.0, r.min.y), Vec2::new(12.0, r.height()));
    text_in(p, letter, Align2::CENTER_CENTER, &file.status.to_string(), theme::meta(10.0), letter_color);
    let mut right = r.max.x - 12.0;
    for (text, color) in [(format!("−{}", file.removed), pal.danger_text), (format!("+{}", file.added), pal.success)] {
        if text.len() <= 2 && text.ends_with('0') {
            continue;
        }
        let g = galley(p, &text, theme::meta(10.0), color);
        let w = g.size().x;
        place(p, Rect::from_min_size(Pos2::new(right - w, r.min.y), Vec2::new(w, r.height())), Align2::LEFT_CENTER, g, color);
        right -= w + 8.0;
    }
    let name_x = r.min.x + 32.0;
    let name = file_name(&file.path);
    let color = if selected { pal.text_strong } else { pal.text };
    let g = galley_trunc(p, name, theme::ui(11.5), color, (right - name_x - 6.0).max(20.0));
    place(p, Rect::from_min_max(Pos2::new(name_x, r.min.y), Pos2::new(right, r.max.y)), Align2::LEFT_CENTER, g, color);
}

// ---------------------------------------------------------------------------
// Markdown
// ---------------------------------------------------------------------------

/// The formatted markdown (`egui_commonmark`: headings, lists, `- [ ]` boxes, tables, quotes, code),
/// in a centered column of up to `MD_MAX_W`.
fn markdown_body(ui: &mut Ui, body: Rect, uid: u64, text: &str, cache: &mut egui_commonmark::CommonMarkCache) {
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(body).layout(egui::Layout::top_down(egui::Align::Min)));
    child.set_clip_rect(body);
    let side = ((body.width() - MD_MAX_W) / 2.0).max(18.0);
    let col = (body.width() - 2.0 * side).max(40.0);
    egui::ScrollArea::vertical().id_salt(("doc-md", uid)).auto_shrink([false, false]).show(&mut child, |ui| {
        ui.add_space(14.0);
        ui.horizontal_top(|ui| {
            ui.add_space(side);
            ui.allocate_ui_with_layout(Vec2::new(col, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
                ui.set_max_width(col);
                markdown_style(ui);
                egui_commonmark::CommonMarkViewer::new().show(ui, cache, text);
            });
        });
        ui.add_space(24.0);
    });
}

/// The markdown text style: egui zeroes the app's spacing, and `egui_commonmark` needs it.
fn markdown_style(ui: &mut Ui) {
    let style = ui.style_mut();
    style.spacing.item_spacing = Vec2::new(6.0, 8.0);
    style.text_styles.insert(egui::TextStyle::Body, theme::ui(13.5));
    style.text_styles.insert(egui::TextStyle::Heading, theme::ui_semibold(21.0));
    style.text_styles.insert(egui::TextStyle::Monospace, theme::mono(12.5));
    style.text_styles.insert(egui::TextStyle::Small, theme::ui(11.5));
    style.visuals.override_text_color = Some(theme::pal().text);
    style.visuals.code_bg_color = theme::pal().hover;
    style.visuals.faint_bg_color = theme::pal().chip;
    style.visuals.extreme_bg_color = theme::pal().hover;
    style.visuals.hyperlink_color = theme::pal().info;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documents_word_shrinks_in_a_narrow_segment() {
        // "Documents 13" needs ~80 px plus room to spare.
        assert_eq!(docs_word(80.0, 100.0), "Documents");
        assert_eq!(docs_word(80.0, 88.0), "Documents");
        assert_eq!(docs_word(80.0, 87.0), "Docs");
        assert_eq!(docs_word(80.0, 60.0), "Docs");
    }
}

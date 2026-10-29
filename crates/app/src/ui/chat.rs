//! Chat view of a tab: the conversation of an agent as messages, tool calls with their diffs, permission cards and a
//! composer, instead of the CLI's terminal screen. The state is in `deck/chat.rs`, the events and the transcript in
//! `holt_core::chat`.
//!
//! The conversation is a scroll area of items laid out one under the other in a centered column. An item's height is
//! measured when it is drawn and remembered (`ChatTab::heights`), so only the items near the viewport are laid out:
//! markdown is the expensive part, and a resumed conversation can have a thousand items.

use std::collections::HashMap;

use egui::{Align, Align2, CornerRadius, Id, Key, Margin, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use holt_core::chat::{Item, PermState, PermissionCard, ToolCall};
use holt_core::t;
use serde_json::Value;

use super::{button, button_width, clickable, frame, galley_trunc, text_in, tip, ButtonKind};
use crate::deck::{ChatTab, Deck};
use crate::icons::{Icon, IconCache};
use crate::theme;

/// Widest the conversation column gets.
const COL_MAX: f32 = 760.0;
const GUTTER: f32 = 16.0;
/// Space between two items.
const GAP: f32 = 14.0;
/// Height assumed for an item that has not been laid out yet.
const GUESS_H: f32 = 48.0;
/// Items this far (px) outside the viewport are laid out anyway, so scrolling does not show blanks.
const OVERSCAN: f32 = 700.0;
const ROW_H: f32 = 17.0;
const TOOL_HEAD_H: f32 = 28.0;
/// Rows of a tool call's body shown before "N more lines".
const BODY_ROWS: usize = 40;
const COMPOSER_MAX_ROWS: usize = 8;
const SLASH_ROWS: usize = 6;

/// What the painted view asks the deck to do (applied after painting).
enum Act {
    Submit,
    Answer { request_id: String, allow: bool, for_session: bool },
    Interrupt,
    Restart,
}

/// Draws the chat of tab `idx` in `body`. Returns whether the pane should take the focus (a click on it).
pub fn show(deck: &mut Deck, ui: &mut Ui, body: Rect, idx: usize) -> bool {
    let Some(mut chat) = deck.tabs[idx].chat.take() else { return false };
    let (uid, alive, color, label) = {
        let t = &deck.tabs[idx];
        (t.uid, t.alive, t.color, t.label.clone())
    };
    let mut acts = Vec::new();
    let senders: HashMap<String, egui::Color32> = chat
        .transcript
        .items
        .iter()
        .filter_map(|i| match i {
            Item::User { from: Some(who), .. } => Some(who.clone()),
            _ => None,
        })
        .filter_map(|who| holt_core::Ident::parse(&who).map(|i| (who, deck.agents.tab_color(&i.kind))))
        .collect();
    let clicked = draw(ui, body, uid, &mut chat, &mut deck.icons, (alive, color, &label), &senders, &mut acts);
    deck.tabs[idx].chat = Some(chat);
    for act in acts {
        match act {
            Act::Submit => deck.chat_submit(idx),
            Act::Answer { request_id, allow, for_session } => deck.chat_answer(idx, &request_id, allow, for_session),
            Act::Interrupt => deck.chat_interrupt(idx),
            Act::Restart => deck.start(idx),
        }
    }
    clicked
}

fn draw(
    ui: &mut Ui,
    body: Rect,
    uid: u64,
    chat: &mut ChatTab,
    icons: &mut IconCache,
    who: (bool, egui::Color32, &str),
    senders: &HashMap<String, egui::Color32>,
    acts: &mut Vec<Act>,
) -> bool {
    let (alive, color, label) = who;
    let ctx = ui.ctx().clone();
    let p = theme::pal();
    let painter = ui.painter_at(body);
    let busy = chat.transcript.busy();
    let pending = chat.transcript.pending_permission().map(|c| c.request_id.clone());

    // ---------- Geometry: the composer sits at the bottom, the conversation takes the rest ----------
    let col_w = (body.width() - 2.0 * GUTTER).clamp(120.0, COL_MAX);
    let side = ((body.width() - col_w) / 2.0).max(GUTTER.min(body.width() / 4.0));
    let font = theme::ui(13.0);
    let rows = composer_rows(&painter, &chat.composer, &font, col_w - 40.0);
    let box_h = (rows as f32 * 18.0 + 22.0).max(42.0);
    let status_h = 20.0;
    let dock_h = box_h + status_h + 16.0;
    let dock = Rect::from_min_max(Pos2::new(body.min.x, body.max.y - dock_h), body.max);
    let view = Rect::from_min_max(body.min, Pos2::new(body.max.x, dock.min.y));
    let box_r = Rect::from_min_size(Pos2::new(body.min.x + side, dock.min.y + 8.0), Vec2::new(col_w, box_h));
    let status_r = Rect::from_min_size(Pos2::new(box_r.min.x, box_r.max.y + 3.0), Vec2::new(col_w, status_h));

    // ---------- Conversation ----------
    if (chat.heights_width - col_w).abs() > 0.5 {
        chat.heights.clear();
        chat.heights_width = col_w;
    }
    let empty = chat.transcript.items.is_empty();
    if empty {
        empty_state(&painter, view, label, alive);
    }
    {
        let ChatTab { transcript, heights, markdown, opened, .. } = chat;
        heights.resize(transcript.items.len(), GUESS_H);
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(view).layout(egui::Layout::top_down(Align::Min)));
        child.set_clip_rect(view);
        let waiting = transcript.waiting_for_permission();
        egui::ScrollArea::vertical().id_salt(("chat-scroll", uid)).auto_shrink([false, false]).stick_to_bottom(true).show_viewport(
            &mut child,
            |ui, viewport| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                ui.add_space(18.0);
                let mut y = 18.0;
                for (i, item) in transcript.items.iter().enumerate() {
                    let guess = heights[i];
                    if y + guess < viewport.min.y - OVERSCAN || y > viewport.max.y + OVERSCAN {
                        ui.add_space(guess + GAP);
                        y += guess + GAP;
                        continue;
                    }
                    let first_in_run = i == 0 || matches!(transcript.items[i - 1], Item::User { .. } | Item::Notice { .. });
                    let mut cx = ItemCx {
                        markdown: &mut *markdown,
                        opened: &mut *opened,
                        acts: &mut *acts,
                        icons: &mut *icons,
                        color,
                        waiting,
                        first_in_run,
                        senders,
                    };
                    let row = ui.push_id(i, |ui| {
                        ui.horizontal_top(|ui| {
                            ui.add_space(side);
                            ui.allocate_ui_with_layout(Vec2::new(col_w, 0.0), egui::Layout::top_down(Align::Min), |ui| {
                                ui.set_max_width(col_w);
                                draw_item(ui, item, col_w, &mut cx);
                            });
                        });
                    });
                    heights[i] = row.response.rect.height().max(1.0);
                    // Calls that follow each other sit closer than the paragraphs around them.
                    let gap =
                        if matches!(item, Item::Tool(_)) && matches!(transcript.items.get(i + 1), Some(Item::Tool(_))) { 6.0 } else { GAP };
                    ui.add_space(gap);
                    y += heights[i] + gap;
                }
                ui.add_space(10.0);
            },
        );
    }

    // ---------- Composer ----------
    painter.rect_filled(dock, 0.0, theme::with_alpha(p.well, 0.0));
    let id = Id::new(("chat-composer", uid));
    let focused = ctx.memory(|m| m.has_focus(id));
    let slash = slash_matches(&chat.composer, &chat.transcript.commands);
    // Keys the text field would otherwise take.
    let mut submit = false;
    if focused {
        if ui.input(|i| i.key_pressed(Key::Tab)) && !slash.is_empty() {
            ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Tab));
            chat.composer = format!("/{} ", slash[0]);
            set_cursor_end(&ctx, id, &chat.composer);
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Enter)) {
            match (&pending, chat.composer.trim().is_empty()) {
                (Some(request_id), true) => acts.push(Act::Answer { request_id: request_id.clone(), allow: true, for_session: false }),
                _ => submit = true,
            }
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            match (&pending, busy) {
                (Some(request_id), _) if chat.composer.trim().is_empty() => {
                    acts.push(Act::Answer { request_id: request_id.clone(), allow: false, for_session: false })
                }
                (_, true) => acts.push(Act::Interrupt),
                _ => {}
            }
        }
        let first_line = chat.composer.is_empty() || chat.history_pos.is_some();
        if first_line && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowUp)) {
            browse(chat, true);
            set_cursor_end(&ctx, id, &chat.composer);
        } else if chat.history_pos.is_some() && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::ArrowDown)) {
            browse(chat, false);
            set_cursor_end(&ctx, id, &chat.composer);
        }
    }
    let edge = if focused { color } else { p.separator };
    frame(&painter, box_r, 10.0, p.recess, edge);
    let mut field = ui.new_child(egui::UiBuilder::new().max_rect(box_r.shrink2(Vec2::new(2.0, 1.0))));
    field.set_clip_rect(box_r);
    let hint = if alive {
        t!("Message {agent}  ·  / for commands", agent = label)
    } else {
        t!("The process ended. Restart it to keep talking.").to_owned()
    };
    let before = chat.composer.clone();
    let te = egui::TextEdit::multiline(&mut chat.composer)
        .id(id)
        .frame(egui::Frame::NONE.inner_margin(Margin { left: 12, right: 12, top: 10, bottom: 8 }))
        .font(font.clone())
        .text_color(p.text)
        .hint_text(egui::RichText::new(hint).font(font).color(p.text_3))
        .desired_width(box_r.width() - 4.0)
        .desired_rows(rows)
        .interactive(alive)
        .lock_focus(true);
    let resp = te.show(&mut field);
    if chat.focus_composer && alive {
        resp.response.request_focus();
        chat.focus_composer = false;
    }
    if chat.composer != before && chat.history_pos.is_some() && !is_history_text(chat) {
        chat.history_pos = None;
    }

    // Slash command suggestions float over the conversation.
    if let Some(name) = slash_popup(ui, box_r, &slash, uid) {
        chat.composer = format!("/{name} ");
        chat.focus_composer = true;
    }

    // ---------- Status line: model and cost on the left, what is going on on the right ----------
    let mut left = String::new();
    if !chat.transcript.model.is_empty() {
        left.push_str(&chat.transcript.model);
    }
    if chat.transcript.cost_usd > 0.0 {
        if !left.is_empty() {
            left.push_str("  ·  ");
        }
        left.push_str(&format!("${:.2}", chat.transcript.cost_usd));
    }
    let sp = ui.painter_at(status_r);
    let g = galley_trunc(&sp, &left, theme::meta(10.5), p.text_3, col_w * 0.5);
    super::place(&sp, status_r, Align2::LEFT_CENTER, g, p.text_3);
    let mut right = status_r.max.x;
    if !alive {
        let w = button_width(&sp, t!("Restart"), ButtonKind::Primary);
        let r = Rect::from_min_size(Pos2::new(right - w, status_r.min.y - 1.0), Vec2::new(w, status_h));
        if button(ui, r, Id::new(("chat-restart", uid)), t!("Restart"), ButtonKind::Primary).clicked() {
            acts.push(Act::Restart);
        }
    } else if busy {
        let label = if pending.is_some() { t!("Waiting for your answer").to_owned() } else { working_text(chat) };
        let stop = t!("Stop");
        let w = button_width(&sp, stop, ButtonKind::Secondary);
        let r = Rect::from_min_size(Pos2::new(right - w, status_r.min.y - 1.0), Vec2::new(w, status_h));
        let resp = button(ui, r, Id::new(("chat-stop", uid)), stop, ButtonKind::Secondary);
        if tip(resp.clone(), t!("Interrupt the turn (Esc)")).clicked() {
            acts.push(Act::Interrupt);
        }
        right = r.min.x - 8.0;
        let g = galley_trunc(&sp, &label, theme::meta(10.5), color, (right - status_r.min.x) * 0.6);
        let rect = Rect::from_min_max(Pos2::new(status_r.min.x, status_r.min.y), Pos2::new(right, status_r.max.y));
        super::place(&sp, rect, Align2::RIGHT_CENTER, g, color);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    } else {
        let send = t!("Enter sends  ·  Shift+Enter new line  ·  ↑ history");
        let g = galley_trunc(&sp, send, theme::meta(10.5), p.text_faint, col_w * 0.6);
        super::place(&sp, status_r, Align2::RIGHT_CENTER, g, p.text_faint);
    }
    // Streaming text arrives between frames: keep drawing while the turn lasts.
    if busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
    if submit {
        acts.push(Act::Submit);
    }
    let _ = icons_unused(icons);
    ui.interact(body, Id::new(("chat-body", uid)), Sense::click()).clicked() || resp.response.clicked()
}

fn icons_unused(_: &mut IconCache) {}

fn working_text(chat: &ChatTab) -> String {
    let secs = chat.turn_started.map_or(0, |t| t.elapsed().as_secs());
    t!("Working  {time}", time = format!("{:02}:{:02}", secs / 60, secs % 60))
}

fn empty_state(painter: &egui::Painter, view: Rect, label: &str, alive: bool) {
    let p = theme::pal();
    let c = view.center();
    text_in(
        painter,
        Rect::from_center_size(Pos2::new(c.x, c.y - 10.0), Vec2::new(view.width(), 22.0)),
        Align2::CENTER_CENTER,
        &t!("Ask {agent} about this project", agent = label),
        theme::ui_semibold(15.0),
        if alive { p.text } else { p.text_3 },
    );
    text_in(
        painter,
        Rect::from_center_size(Pos2::new(c.x, c.y + 12.0), Vec2::new(view.width(), 18.0)),
        Align2::CENTER_CENTER,
        t!("Files you attach become @mentions. Type / for commands."),
        theme::ui(12.0),
        p.text_3,
    );
}

/// Lines the composer needs: its own line breaks plus the wrapping of long lines.
fn composer_rows(painter: &egui::Painter, text: &str, font: &egui::FontId, wrap: f32) -> usize {
    if text.is_empty() {
        return 1;
    }
    let galley = painter.layout(text.to_owned(), font.clone(), egui::Color32::WHITE, wrap.max(40.0));
    let trailing = usize::from(text.ends_with('\n'));
    (galley.rows.len() + trailing).clamp(1, COMPOSER_MAX_ROWS)
}

fn set_cursor_end(ctx: &egui::Context, id: Id, text: &str) {
    if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
        let end = egui::text::CCursor::new(text.chars().count());
        state.cursor.set_char_range(Some(egui::text::CCursorRange::one(end)));
        state.store(ctx, id);
    }
}

/// ↑/↓ in the composer: browses what was sent before; the unsent text comes back at the bottom.
fn browse(chat: &mut ChatTab, up: bool) {
    if chat.history.is_empty() {
        return;
    }
    let next = match (chat.history_pos, up) {
        (None, true) => {
            chat.draft = chat.composer.clone();
            Some(chat.history.len() - 1)
        }
        (None, false) => None,
        (Some(0), true) => Some(0),
        (Some(n), true) => Some(n - 1),
        (Some(n), false) if n + 1 < chat.history.len() => Some(n + 1),
        (Some(_), false) => None,
    };
    chat.history_pos = next;
    chat.composer = match next {
        Some(n) => chat.history[n].clone(),
        None => std::mem::take(&mut chat.draft),
    };
}

fn is_history_text(chat: &ChatTab) -> bool {
    chat.history_pos.and_then(|n| chat.history.get(n)).is_some_and(|h| *h == chat.composer)
}

/// Slash commands that start like what is being typed (only while the line is just a command name).
fn slash_matches(text: &str, commands: &[String]) -> Vec<String> {
    let Some(word) = text.strip_prefix('/') else { return Vec::new() };
    if word.contains(char::is_whitespace) {
        return Vec::new();
    }
    let word = word.to_lowercase();
    let mut found: Vec<String> = commands.iter().filter(|c| c.to_lowercase().starts_with(&word) && **c != word).cloned().collect();
    found.sort();
    found.dedup();
    found.truncate(SLASH_ROWS);
    found
}

/// The list of suggestions above the composer; returns the one clicked.
fn slash_popup(ui: &mut Ui, box_r: Rect, names: &[String], uid: u64) -> Option<String> {
    if names.is_empty() {
        return None;
    }
    let p = theme::pal();
    let item_h = 24.0;
    let h = item_h * names.len() as f32 + 8.0;
    let rect = Rect::from_min_size(Pos2::new(box_r.min.x, box_r.min.y - h - 6.0), Vec2::new(box_r.width().min(320.0), h));
    let mut picked = None;
    egui::Area::new(Id::new(("chat-slash", uid))).order(egui::Order::Foreground).fixed_pos(rect.min).show(ui.ctx(), |ui| {
        let (r, _) = ui.allocate_exact_size(rect.size(), Sense::hover());
        let painter = ui.painter();
        super::shadow(painter, r, 8, 8, 24, p.shadow);
        frame(painter, r, 8.0, p.popup, p.glass_border);
        for (n, name) in names.iter().enumerate() {
            let row = Rect::from_min_size(Pos2::new(r.min.x + 4.0, r.min.y + 4.0 + n as f32 * item_h), Vec2::new(r.width() - 8.0, item_h));
            let resp = clickable(ui, row, Id::new(("chat-slash-item", uid, n)));
            if resp.hovered() {
                painter.rect_filled(row, 5.0, p.hover);
            }
            text_in(painter, row.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, &format!("/{name}"), theme::mono(12.0), p.text);
            if resp.clicked() {
                picked = Some(name.clone());
            }
        }
    });
    picked
}

// ---------- Items ----------

struct ItemCx<'a> {
    markdown: &'a mut egui_commonmark::CommonMarkCache,
    opened: &'a mut HashMap<String, bool>,
    acts: &'a mut Vec<Act>,
    icons: &'a mut IconCache,
    /// The tab's agent color.
    color: egui::Color32,
    /// A permission card is waiting (the tool calls before it are not "running", they are waiting for the answer).
    waiting: bool,
    /// The item opens a run of the agent's turns (the avatar is drawn once per run).
    first_in_run: bool,
    /// Color of each agent that sent a message to this tab (id → color).
    senders: &'a HashMap<String, egui::Color32>,
}

fn draw_item(ui: &mut Ui, item: &Item, col_w: f32, cx: &mut ItemCx) {
    let p = theme::pal();
    match item {
        Item::User { text, from: None } => {
            ui.with_layout(egui::Layout::top_down(Align::Max), |ui| {
                egui::Frame::new()
                    .fill(p.recess)
                    .stroke(Stroke::new(1.0, p.separator))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(col_w * 0.82);
                        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(13.0)).color(p.text_strong)).selectable(true));
                    });
            });
        }
        Item::User { text, from: Some(who) } => {
            let sender = cx.senders.get(who).copied().unwrap_or(p.info);
            let card = egui::Frame::new()
                .fill(theme::with_alpha(sender, 0.07))
                .stroke(Stroke::new(1.0, p.separator))
                .corner_radius(8)
                .inner_margin(Margin { left: 15, right: 12, top: 8, bottom: 8 })
                .show(ui, |ui| {
                    ui.set_width(col_w - 29.0);
                    ui.label(egui::RichText::new(t!("From {agent}", agent = who).to_uppercase()).font(theme::label(10.0)).color(sender));
                    ui.add_space(3.0);
                    ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(13.0)).color(p.text)).selectable(true));
                });
            let r = card.response.rect;
            ui.painter().rect_filled(
                Rect::from_min_max(r.min, Pos2::new(r.min.x + 3.0, r.max.y)),
                CornerRadius { nw: 8, sw: 8, ne: 0, se: 0 },
                sender,
            );
        }
        Item::Assistant { text, streaming, .. } => {
            ui.horizontal_top(|ui| {
                let (mark, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
                if cx.first_in_run {
                    ui.painter().rect_filled(mark, 6.0, theme::accent_soft(cx.color));
                    cx.icons.paint(ui.painter(), Icon::ClaudeLogo, mark.center(), 12.0, cx.color, 0.0);
                }
                ui.add_space(10.0);
                ui.allocate_ui_with_layout(Vec2::new(col_w - 32.0, 0.0), egui::Layout::top_down(Align::Min), |ui| {
                    ui.set_max_width(col_w - 32.0);
                    markdown_style(ui);
                    let shown = if *streaming { format!("{text} ▍") } else { text.clone() };
                    egui_commonmark::CommonMarkViewer::new().show(ui, cx.markdown, &shown);
                });
            });
        }
        Item::Thinking(text) => {
            ui.label(egui::RichText::new(text).font(theme::ui(12.0)).italics().color(p.text_3));
        }
        Item::Tool(call) => tool_card(ui, call, col_w, cx),
        Item::Permission(card) => permission_card(ui, card, col_w, cx),
        Item::Notice { text, error } => {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(text).font(theme::ui(12.0)).color(if *error { p.danger_text } else { p.text_3 }));
            });
        }
    }
}

/// Text style of the conversation's markdown (egui zeroes the app's spacing and `egui_commonmark` needs it).
fn markdown_style(ui: &mut Ui) {
    let p = theme::pal();
    let style = ui.style_mut();
    style.spacing.item_spacing = Vec2::new(6.0, 8.0);
    style.text_styles.insert(egui::TextStyle::Body, theme::ui(13.0));
    style.text_styles.insert(egui::TextStyle::Heading, theme::ui_semibold(16.0));
    style.text_styles.insert(egui::TextStyle::Monospace, theme::mono(12.0));
    style.text_styles.insert(egui::TextStyle::Small, theme::ui(11.5));
    style.visuals.override_text_color = Some(p.text);
    style.visuals.code_bg_color = p.hover;
    style.visuals.faint_bg_color = p.chip;
    style.visuals.extreme_bg_color = p.recess;
    style.visuals.hyperlink_color = p.info;
}

// ---------- Tool calls ----------

#[derive(Clone, Copy, PartialEq, Debug)]
enum RowKind {
    Add,
    Del,
    Plain,
    Output,
    Error,
}

struct Row {
    kind: RowKind,
    text: String,
}

fn rows_of(kind: RowKind, text: &str) -> impl Iterator<Item = Row> + '_ {
    text.lines().map(move |l| Row { kind, text: l.replace('\t', "    ") })
}

fn str_field<'a>(input: &'a Value, key: &str) -> &'a str {
    input.get(key).and_then(Value::as_str).unwrap_or("")
}

/// The one-line description of a call: the command, the file, the pattern.
fn tool_summary(name: &str, input: &Value) -> String {
    let first = |keys: &[&str]| keys.iter().map(|k| str_field(input, k)).find(|s| !s.is_empty()).unwrap_or("").to_owned();
    let line = match name {
        "Bash" => first(&["command"]),
        "Read" | "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => first(&["file_path", "notebook_path"]),
        "Grep" => {
            let path = str_field(input, "path");
            format!("\"{}\"{}", str_field(input, "pattern"), if path.is_empty() { String::new() } else { format!(" in {path}") })
        }
        "Glob" => first(&["pattern"]),
        "WebFetch" => first(&["url"]),
        "WebSearch" => first(&["query"]),
        "Task" | "Agent" => first(&["description", "prompt"]),
        _ => input.as_object().and_then(|o| o.values().find_map(Value::as_str)).unwrap_or("").to_owned(),
    };
    line.lines().next().unwrap_or("").trim().to_owned()
}

/// The lines under a call's header: the diff of an edit, the file being written, the command and its output.
fn tool_rows(call: &ToolCall) -> Vec<Row> {
    let mut rows = Vec::new();
    let input = &call.input;
    match call.name.as_str() {
        "Edit" => {
            rows.extend(rows_of(RowKind::Del, str_field(input, "old_string")));
            rows.extend(rows_of(RowKind::Add, str_field(input, "new_string")));
        }
        "MultiEdit" => {
            for e in input.get("edits").and_then(Value::as_array).into_iter().flatten() {
                rows.extend(rows_of(RowKind::Del, str_field(e, "old_string")));
                rows.extend(rows_of(RowKind::Add, str_field(e, "new_string")));
            }
        }
        "Write" => rows.extend(rows_of(RowKind::Add, str_field(input, "content"))),
        "Bash" => rows.extend(rows_of(RowKind::Plain, &format!("$ {}", str_field(input, "command")))),
        _ => {}
    }
    if let Some(out) = &call.outcome {
        let edits = matches!(call.name.as_str(), "Edit" | "MultiEdit" | "Write");
        // A successful edit says nothing the diff does not.
        if out.is_error || !edits {
            rows.extend(rows_of(if out.is_error { RowKind::Error } else { RowKind::Output }, &out.text));
        }
    }
    rows
}

fn diff_counts(rows: &[Row]) -> (usize, usize) {
    (rows.iter().filter(|r| r.kind == RowKind::Add).count(), rows.iter().filter(|r| r.kind == RowKind::Del).count())
}

fn default_open(call: &ToolCall) -> bool {
    matches!(call.name.as_str(), "Edit" | "MultiEdit" | "Write") || call.outcome.as_ref().is_some_and(|o| o.is_error)
}

fn tool_card(ui: &mut Ui, call: &ToolCall, col_w: f32, cx: &mut ItemCx) {
    let p = theme::pal();
    let rows = tool_rows(call);
    let open = cx.opened.get(&call.id).copied().unwrap_or_else(|| default_open(call)) && !rows.is_empty();
    let body_rows = rows.len().min(BODY_ROWS) + usize::from(rows.len() > BODY_ROWS);
    let body_h = if open { body_rows as f32 * ROW_H + 8.0 } else { 0.0 };
    let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w, TOOL_HEAD_H + body_h), Sense::hover());
    let painter = ui.painter_at(rect);
    frame(&painter, rect, 8.0, theme::with_alpha(p.text_strong, 0.025), p.separator);

    // Header
    let head = Rect::from_min_size(rect.min, Vec2::new(col_w, TOOL_HEAD_H));
    let resp = clickable(ui, head, Id::new(("tool-head", &call.id)));
    if resp.hovered() {
        painter.rect_filled(head.shrink(1.0), 7.0, p.hover);
    }
    if !rows.is_empty() && resp.clicked() {
        cx.opened.insert(call.id.clone(), !open);
    }
    let chev = if rows.is_empty() {
        " "
    } else if open {
        "▾"
    } else {
        "▸"
    };
    text_in(
        &painter,
        Rect::from_min_size(head.min + Vec2::new(8.0, 0.0), Vec2::new(14.0, TOOL_HEAD_H)),
        Align2::CENTER_CENTER,
        chev,
        theme::ui(10.0),
        p.text_3,
    );
    let name_w = super::text_w(&painter, &call.name, theme::mono_bold(11.0));
    text_in(
        &painter,
        Rect::from_min_size(head.min + Vec2::new(26.0, 0.0), Vec2::new(name_w + 2.0, TOOL_HEAD_H)),
        Align2::LEFT_CENTER,
        &call.name,
        theme::mono_bold(11.0),
        p.text_strong,
    );
    let (status, status_color) = match &call.outcome {
        None if cx.waiting => (t!("waiting").to_owned(), p.warning),
        None => (t!("running…").to_owned(), p.warning),
        Some(o) if o.is_error => (t!("error").to_owned(), p.danger_text),
        Some(_) => {
            let (a, d) = diff_counts(&rows);
            let edits = matches!(call.name.as_str(), "Edit" | "MultiEdit" | "Write");
            (if edits && (a > 0 || d > 0) { format!("+{a} −{d}") } else { t!("done").to_owned() }, p.success)
        }
    };
    let status_w = super::text_w(&painter, &status, theme::meta(10.5)) + 4.0;
    text_in(
        &painter,
        Rect::from_min_max(Pos2::new(head.max.x - status_w - 10.0, head.min.y), Pos2::new(head.max.x - 10.0, head.max.y)),
        Align2::RIGHT_CENTER,
        &status,
        theme::meta(10.5),
        status_color,
    );
    let sum_x = head.min.x + 26.0 + name_w + 10.0;
    let sum_w = (head.max.x - status_w - 22.0 - sum_x).max(0.0);
    let g = galley_trunc(&painter, &tool_summary(&call.name, &call.input), theme::mono(11.0), p.text_3, sum_w);
    super::place(
        &painter,
        Rect::from_min_size(Pos2::new(sum_x, head.min.y), Vec2::new(sum_w, TOOL_HEAD_H)),
        Align2::LEFT_CENTER,
        g,
        p.text_3,
    );

    if open {
        paint_rows(&painter, Rect::from_min_max(Pos2::new(rect.min.x, head.max.y), rect.max), &rows);
    }
}

/// The rows of a call's body, as a diff or as output.
fn paint_rows(painter: &egui::Painter, area: Rect, rows: &[Row]) {
    let p = theme::pal();
    painter.rect_filled(Rect::from_min_max(Pos2::new(area.min.x, area.min.y), Pos2::new(area.max.x, area.min.y + 1.0)), 0.0, p.separator);
    let mut y = area.min.y + 4.0;
    for row in rows.iter().take(BODY_ROWS) {
        let r = Rect::from_min_size(Pos2::new(area.min.x + 1.0, y), Vec2::new(area.width() - 2.0, ROW_H));
        let (bg, fg, mark) = match row.kind {
            RowKind::Add => (Some(theme::with_alpha(p.success, 0.13)), p.text, "+"),
            RowKind::Del => (Some(theme::with_alpha(p.danger, 0.13)), p.text, "−"),
            RowKind::Plain => (None, p.text_soft, " "),
            RowKind::Output => (None, p.text_2, " "),
            RowKind::Error => (None, p.danger_text, " "),
        };
        if let Some(bg) = bg {
            painter.rect_filled(r, 0.0, bg);
        }
        let mark_color = match row.kind {
            RowKind::Add => p.success,
            RowKind::Del => p.danger_text,
            _ => p.text_faint,
        };
        text_in(painter, Rect::from_min_size(r.min, Vec2::new(22.0, ROW_H)), Align2::CENTER_CENTER, mark, theme::mono(11.5), mark_color);
        let g = galley_trunc(painter, &row.text, theme::mono(11.5), fg, r.width() - 30.0);
        super::place(
            painter,
            Rect::from_min_size(Pos2::new(r.min.x + 26.0, y), Vec2::new(r.width() - 30.0, ROW_H)),
            Align2::LEFT_CENTER,
            g,
            fg,
        );
        y += ROW_H;
    }
    if rows.len() > BODY_ROWS {
        let more = t!("… {n} more lines", n = rows.len() - BODY_ROWS);
        text_in(
            painter,
            Rect::from_min_size(Pos2::new(area.min.x + 26.0, y), Vec2::new(area.width() - 30.0, ROW_H)),
            Align2::LEFT_CENTER,
            &more,
            theme::meta(10.5),
            p.text_3,
        );
    }
}

// ---------- Permission cards ----------

fn permission_title(tool: &str) -> String {
    match tool {
        "Bash" => t!("Run a command").to_owned(),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => t!("Edit a file").to_owned(),
        "Read" | "Glob" | "Grep" => t!("Read files").to_owned(),
        "WebFetch" | "WebSearch" => t!("Use the web").to_owned(),
        other => t!("Use {tool}", tool = other),
    }
}

fn permission_card(ui: &mut Ui, card: &PermissionCard, col_w: f32, cx: &mut ItemCx) {
    let p = theme::pal();
    let call = ToolCall { id: card.tool_use_id.clone(), name: card.tool.clone(), input: card.input.clone(), outcome: None };
    let rows = tool_rows(&call);
    // A decided card shrinks to one line.
    if card.state != PermState::Pending {
        let (text, color) = match card.state {
            PermState::Allowed => (t!("Allowed").to_owned(), p.success),
            PermState::Denied => (t!("Denied").to_owned(), p.danger_text),
            _ => (t!("Not answered").to_owned(), p.text_3),
        };
        let line = format!("{}  ·  {}  ·  {}", permission_title(&card.tool), tool_summary(&card.tool, &card.input), text);
        let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w, 22.0), Sense::hover());
        let painter = ui.painter_at(rect);
        let g = galley_trunc(&painter, &line, theme::meta(10.5), color, col_w - 8.0);
        super::place(&painter, rect, Align2::LEFT_CENTER, g, color);
        return;
    }
    let shown = rows.len().min(10);
    let summary = if rows.is_empty() { tool_summary(&card.tool, &card.input) } else { String::new() };
    let content_h = if rows.is_empty() {
        if summary.is_empty() {
            0.0
        } else {
            30.0
        }
    } else {
        shown as f32 * ROW_H + 8.0 + if rows.len() > shown { ROW_H } else { 0.0 }
    };
    let h = 12.0 + 20.0 + 8.0 + content_h + if content_h > 0.0 { 10.0 } else { 0.0 } + 28.0 + 12.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w, h), Sense::hover());
    let painter = ui.painter_at(rect);
    // A permission is a safety question, not the agent's personality: amber, whatever the agent.
    painter.rect_filled(rect, 10.0, theme::with_alpha(p.warning, 0.10));
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, theme::with_alpha(p.warning, 0.55)), egui::StrokeKind::Inside);
    painter.rect_filled(
        Rect::from_min_max(rect.min, Pos2::new(rect.min.x + 3.0, rect.max.y)),
        CornerRadius { nw: 10, sw: 10, ne: 0, se: 0 },
        p.warning,
    );
    let mut y = rect.min.y + 12.0;
    let tag_w = super::text_w(&painter, &t!("Permission").to_uppercase(), theme::label(10.0)) + 12.0;
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(rect.min.x + 14.0, y), Vec2::new(tag_w, 20.0)),
        Align2::LEFT_CENTER,
        &t!("Permission").to_uppercase(),
        theme::label(10.0),
        p.warning,
    );
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(rect.min.x + 14.0 + tag_w, y), Vec2::new(col_w - 24.0 - tag_w, 20.0)),
        Align2::LEFT_CENTER,
        &permission_title(&card.tool),
        theme::ui_semibold(13.0),
        p.text_strong,
    );
    y += 28.0;
    if content_h > 0.0 {
        let area = Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(col_w - 24.0, content_h));
        painter.rect_filled(area, 6.0, p.recess);
        if rows.is_empty() {
            let g = galley_trunc(&painter, &summary, theme::mono(12.0), p.text, area.width() - 16.0);
            super::place(&painter, area.shrink2(Vec2::new(8.0, 0.0)), Align2::LEFT_CENTER, g, p.text);
        } else {
            let mut limited = rows.iter().take(shown).map(|r| Row { kind: r.kind, text: r.text.clone() }).collect::<Vec<_>>();
            if rows.len() > shown {
                limited.push(Row { kind: RowKind::Output, text: t!("… {n} more lines", n = rows.len() - shown) });
            }
            paint_rows(&painter, area, &limited);
        }
        y += content_h + 10.0;
    }
    // Buttons
    let mut x = rect.min.x + 12.0;
    let can_session = !card.suggestions.is_empty();
    let buttons: [(&str, ButtonKind, bool, bool); 3] = [
        (t!("Allow"), ButtonKind::Primary, true, false),
        (t!("Allow for this session"), ButtonKind::Secondary, true, true),
        (t!("Deny"), ButtonKind::Secondary, false, false),
    ];
    for (n, (label, kind, allow, for_session)) in buttons.into_iter().enumerate() {
        if for_session && !can_session {
            continue;
        }
        let w = button_width(&painter, label, kind);
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 28.0));
        let id = Id::new(("perm-btn", &card.request_id, n));
        let clicked = if kind == ButtonKind::Primary {
            let resp = clickable(ui, r, id);
            let fill = if resp.hovered() { theme::brighten(p.warning, 1.1) } else { p.warning };
            painter.rect_filled(r, 5.0, fill);
            text_in(&painter, r, Align2::CENTER_CENTER, label, theme::ui_semibold(11.5), p.on_accent);
            resp.clicked()
        } else {
            button(ui, r, id, label, kind).clicked()
        };
        if clicked {
            cx.acts.push(Act::Answer { request_id: card.request_id.clone(), allow, for_session });
        }
        x += w + 6.0;
    }
    let hint = t!("Enter allows  ·  Esc denies");
    text_in(
        &painter,
        Rect::from_min_max(Pos2::new(x + 6.0, y), Pos2::new(rect.max.x - 12.0, y + 28.0)),
        Align2::RIGHT_CENTER,
        hint,
        theme::meta(10.0),
        p.text_3,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::chat::ToolOutcome;
    use serde_json::json;

    fn call(name: &str, input: Value, out: Option<(&str, bool)>) -> ToolCall {
        ToolCall {
            id: "t1".into(),
            name: name.into(),
            input,
            outcome: out.map(|(text, is_error)| ToolOutcome { tool_use_id: "t1".into(), text: text.into(), is_error }),
        }
    }

    #[test]
    fn a_call_is_summed_up_in_one_line() {
        assert_eq!(tool_summary("Bash", &json!({ "command": "cargo test\nmore" })), "cargo test");
        assert_eq!(tool_summary("Edit", &json!({ "file_path": "src/a.rs" })), "src/a.rs");
        assert_eq!(tool_summary("Grep", &json!({ "pattern": "fn x", "path": "src" })), "\"fn x\" in src");
        assert_eq!(tool_summary("Mystery", &json!({ "n": 3, "what": "this" })), "this");
    }

    #[test]
    fn an_edit_shows_removed_then_added_lines_and_hides_its_boring_result() {
        let c = call("Edit", json!({ "old_string": "a\nb", "new_string": "c" }), Some(("updated", false)));
        let rows = tool_rows(&c);
        let kinds: Vec<RowKind> = rows.iter().map(|r| r.kind).collect();
        assert_eq!(kinds, [RowKind::Del, RowKind::Del, RowKind::Add]);
        assert_eq!(diff_counts(&rows), (1, 2));
        // A failed edit shows why.
        let failed = call("Edit", json!({ "old_string": "a", "new_string": "c" }), Some(("String to replace not found", true)));
        assert_eq!(tool_rows(&failed).last().map(|r| r.kind), Some(RowKind::Error));
    }

    #[test]
    fn a_command_shows_the_line_and_its_output() {
        let c = call("Bash", json!({ "command": "ls" }), Some(("a\nb", false)));
        let rows = tool_rows(&c);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].text, "$ ls");
        assert_eq!(rows[2].kind, RowKind::Output);
    }

    #[test]
    fn edits_start_open_and_the_rest_closed_unless_they_failed() {
        assert!(default_open(&call("Write", json!({}), None)));
        assert!(!default_open(&call("Read", json!({}), Some(("x", false)))));
        assert!(default_open(&call("Read", json!({}), Some(("nope", true)))));
    }

    #[test]
    fn slash_suggestions_complete_a_command_name() {
        let cmds: Vec<String> = ["context", "compact", "cost", "clear"].iter().map(|s| (*s).to_owned()).collect();
        assert_eq!(slash_matches("/co", &cmds), ["compact", "context", "cost"]);
        assert!(slash_matches("/context", &cmds).is_empty(), "already complete");
        assert!(slash_matches("/context now", &cmds).is_empty(), "arguments started");
        assert!(slash_matches("hello", &cmds).is_empty());
    }

    #[test]
    fn the_arrow_keys_browse_what_was_sent_and_bring_the_draft_back() {
        let mut chat = ChatTab::new(Default::default());
        chat.history = vec!["one".into(), "two".into()];
        chat.composer = "draft".into();
        browse(&mut chat, true);
        assert_eq!(chat.composer, "two");
        browse(&mut chat, true);
        assert_eq!(chat.composer, "one");
        browse(&mut chat, true);
        assert_eq!(chat.composer, "one");
        browse(&mut chat, false);
        assert_eq!(chat.composer, "two");
        browse(&mut chat, false);
        assert_eq!((chat.composer.as_str(), chat.history_pos), ("draft", None));
    }
}

//! Chat view of a tab: the conversation of an agent as messages, tool calls with their diffs, permission cards and a
//! composer, instead of the CLI's terminal screen. The state is in `deck/chat.rs`, the events and the transcript in
//! `holt_core::chat`.
//!
//! The conversation is a scroll area of items laid out one under the other in a centered column. An item's height is
//! measured when it is drawn and remembered (`ChatTab::heights`), so only the items near the viewport are laid out:
//! markdown is the expensive part, and a resumed conversation can have a thousand items.

use std::collections::HashMap;

use egui::{Align, Align2, CornerRadius, Id, Key, Margin, Modifiers, Pos2, Rect, Sense, Stroke, Ui, Vec2};
use holt_core::chat::{Item, PatchHunk, PermState, PermissionCard, ToolCall};
use holt_core::t;
use serde_json::Value;

use super::{button, button_width, clickable, frame, galley_trunc, text_in, tip, ButtonKind};
use crate::audio;
use crate::deck::{ChatTab, Deck};
use crate::dictation::{self, ButtonVisual, MicTarget};
use crate::icons::{Icon, IconCache};
use crate::theme;

/// Widest the conversation column gets.
const COL_MAX: f32 = 760.0;
const GUTTER: f32 = 16.0;
/// Space between two items.
const GAP: f32 = 18.0;
/// Space between the items of one run of the agent's turn.
const RUN_GAP: f32 = 10.0;
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
/// Height of the row inside the composer box (attach, mic, model, send).
const BOX_ROW_H: f32 = 30.0;
/// Slash commands offered before the CLI has told its own.
const USUAL_COMMANDS: &[&str] = &["clear", "compact", "context", "cost", "help", "init", "memory", "model", "review", "status"];

/// What the painted view asks the deck to do (applied after painting).
enum Act {
    Submit,
    Answer { request_id: String, allow: bool, for_session: bool },
    Interrupt,
    Restart,
    Attach,
    Mic,
}

/// What the view needs to know about its tab besides the conversation.
struct Who<'a> {
    kind: &'a str,
    alive: bool,
    color: egui::Color32,
    label: &'a str,
    /// Short model name (`sonnet`) for the line under the text.
    model: String,
    mic: ButtonVisual,
    mic_tip: String,
    attach_tip: String,
}

/// Draws the chat of tab `idx` in `body`. Returns whether the pane should take the focus (a click on it).
pub fn show(deck: &mut Deck, ui: &mut Ui, body: Rect, idx: usize) -> bool {
    let Some(mut chat) = deck.tabs[idx].chat.take() else { return false };
    let (uid, kind, alive, color, label, model) = {
        let t = &deck.tabs[idx];
        (t.uid, t.kind.clone(), t.alive, t.color, t.label.clone(), t.model.clone())
    };
    let mic = dictation::button_visual(deck.mic_phase, MicTarget::Tab(uid));
    let mic_tip = if alive {
        dictation::mic_tooltip(mic, audio::AVAILABLE, deck.dictation_setup(), &deck.engine_label, Some("Ctrl+Shift+M"))
    } else {
        t!("Dictation unavailable (process ended)").to_owned()
    };
    let who = Who {
        kind: &kind,
        alive,
        color,
        label: &label,
        model: model.unwrap_or_else(|| chat.transcript.model.clone()),
        mic,
        mic_tip,
        attach_tip: t!("Attach files to {label}", label = label),
    };
    let mut acts = Vec::new();
    let clicked = draw(ui, body, uid, &mut chat, &mut deck.icons, &who, &mut acts);
    deck.tabs[idx].chat = Some(chat);
    for act in acts {
        match act {
            Act::Submit => deck.chat_submit(idx),
            Act::Answer { request_id, allow, for_session } => deck.chat_answer(idx, &request_id, allow, for_session),
            Act::Interrupt => deck.chat_interrupt(idx),
            Act::Restart => deck.start(idx),
            Act::Attach => deck.pick_tab_attachments(uid),
            Act::Mic => deck.toggle_tab_mic(uid),
        }
    }
    clicked
}

fn draw(ui: &mut Ui, body: Rect, uid: u64, chat: &mut ChatTab, icons: &mut IconCache, who: &Who, acts: &mut Vec<Act>) -> bool {
    let Who { alive, color, label, .. } = *who;
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
    // The box holds the text and, under it, the row with the attach and mic buttons, the model and the send button.
    let text_h = rows as f32 * 18.0 + 16.0;
    let box_h = text_h + BOX_ROW_H + 6.0;
    let note_h = 18.0;
    let dock_h = box_h + note_h + 18.0;
    let dock = Rect::from_min_max(Pos2::new(body.min.x, body.max.y - dock_h), body.max);
    let view = Rect::from_min_max(body.min, Pos2::new(body.max.x, dock.min.y));
    let box_r = Rect::from_min_size(Pos2::new(body.min.x + side, dock.min.y + 8.0), Vec2::new(col_w, box_h));
    let text_r = Rect::from_min_size(box_r.min, Vec2::new(col_w, text_h));
    let row_r =
        Rect::from_min_max(Pos2::new(box_r.min.x + 8.0, box_r.max.y - BOX_ROW_H - 4.0), Pos2::new(box_r.max.x - 8.0, box_r.max.y - 4.0));
    let note_r = Rect::from_min_size(Pos2::new(box_r.min.x, box_r.max.y + 4.0), Vec2::new(col_w, note_h));

    // ---------- Conversation ----------
    if (chat.heights_width - col_w).abs() > 0.5 {
        chat.heights.clear();
        chat.heights_width = col_w;
    }
    let icon = match who.kind {
        "antigravity" => Icon::AntigravityLogo,
        "codex" => Icon::CodexLogo,
        _ => Icon::ClaudeLogo,
    };
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
                    let first_in_run = i == 0 || !is_agent_item(&transcript.items[i - 1]);
                    let mut cx = ItemCx {
                        markdown: &mut *markdown,
                        opened: &mut *opened,
                        acts: &mut *acts,
                        icons: &mut *icons,
                        icon,
                        color,
                        waiting,
                        first_in_run,
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
                    // The agent's items sit closer to each other than to what the user said.
                    let gap = if is_agent_item(item) && transcript.items.get(i + 1).is_some_and(is_agent_item) { RUN_GAP } else { GAP };
                    ui.add_space(gap);
                    y += heights[i] + gap;
                }
                ui.add_space(10.0);
            },
        );
    }

    // ---------- Composer ----------
    let id = Id::new(("chat-composer", uid));
    let focused = ctx.memory(|m| m.has_focus(id));
    // The CLI lists its commands in the first `init`, which only comes with the first turn: until then, the usual ones.
    let usual: Vec<String> = USUAL_COMMANDS.iter().map(|c| (*c).to_owned()).collect();
    let known = if chat.transcript.commands.is_empty() { &usual } else { &chat.transcript.commands };
    let slash = slash_matches(&chat.composer, known);
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
    // The box keeps the agent's edge like the mockup; the focus makes it solid.
    let edge = if focused { color } else { theme::accent_edge(color) };
    frame(&painter, box_r, 10.0, p.recess, edge);
    let mut field = ui.new_child(egui::UiBuilder::new().max_rect(text_r.shrink2(Vec2::new(2.0, 1.0))));
    field.set_clip_rect(text_r);
    let hint = if alive {
        t!("Message {agent}  ·  / for commands", agent = label)
    } else {
        t!("The process ended. Restart it to keep talking.").to_owned()
    };
    let before = chat.composer.clone();
    let te = egui::TextEdit::multiline(&mut chat.composer)
        .id(id)
        .frame(egui::Frame::NONE.inner_margin(Margin { left: 12, right: 12, top: 9, bottom: 7 }))
        .font(font.clone())
        .text_color(p.text)
        .hint_text(egui::RichText::new(hint).font(font).color(p.text_3))
        .desired_width(box_r.width() - 4.0)
        .desired_rows(rows)
        .interactive(alive);
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

    // ---------- Row inside the box: attach, mic, model and context on the left; cost and send on the right ----------
    let rp = ui.painter_at(box_r);
    let mut x = row_r.min.x;
    let icon_btn =
        |ui: &Ui, icons: &mut IconCache, x: f32, icon: Icon, id: Id, on: bool, hot_color: egui::Color32, tooltip: String| -> bool {
            let r = Rect::from_min_size(Pos2::new(x, row_r.min.y + (BOX_ROW_H - 22.0) / 2.0), Vec2::splat(22.0));
            let resp = if on { clickable(ui, r, id) } else { ui.interact(r, id, Sense::hover()) };
            let hot = resp.hovered() && on;
            if hot {
                ui.painter().rect_filled(r, 5.0, p.hover);
            }
            let fg = if hot {
                p.text
            } else if on {
                p.text_2
            } else {
                theme::with_opacity(p.text_2, 0.45)
            };
            icons.paint(ui.painter(), icon, r.center(), 14.0, if hot_color == egui::Color32::TRANSPARENT { fg } else { hot_color }, 0.0);
            tip(resp, tooltip).clicked() && on
        };
    if icon_btn(ui, icons, x, Icon::Attach, Id::new(("chat-attach", uid)), alive, egui::Color32::TRANSPARENT, who.attach_tip.clone()) {
        acts.push(Act::Attach);
    }
    x += 26.0;
    let mic_on = alive && !who.mic.is_busy() && audio::AVAILABLE;
    let recording = who.mic == ButtonVisual::Recording;
    if recording {
        let r = Rect::from_min_size(Pos2::new(x, row_r.min.y + (BOX_ROW_H - 22.0) / 2.0), Vec2::splat(22.0));
        let a = super::right_panel::blink_alpha(ctx.input(|i| i.time), 1.2);
        rp.rect_filled(r, 5.0, theme::with_opacity(p.danger, a));
        ctx.request_repaint_after(std::time::Duration::from_millis(50));
    }
    if icon_btn(
        ui,
        icons,
        x,
        Icon::Mic,
        Id::new(("chat-mic", uid)),
        alive && (mic_on || recording),
        if recording { p.text_strong } else { egui::Color32::TRANSPARENT },
        who.mic_tip.clone(),
    ) {
        acts.push(Act::Mic);
    }
    x += 30.0;
    let mut meta = who.model.clone();
    if let Some(pct) = chat.transcript.context_percent() {
        if !meta.is_empty() {
            meta.push_str("  ·  ");
        }
        meta.push_str(&t!("{pct}% context", pct = pct));
    }
    let right_end = row_r.max.x;
    // The send button (or Stop, or Restart) and the cost sit at the right.
    let action: (&str, ButtonKind, bool) = if !alive {
        (t!("Restart"), ButtonKind::Primary, true)
    } else if busy {
        (t!("Stop"), ButtonKind::Secondary, true)
    } else {
        (t!("Send"), ButtonKind::Primary, !chat.composer.trim().is_empty())
    };
    let bw = button_width(&rp, action.0, action.1).max(52.0);
    let br = Rect::from_min_size(Pos2::new(right_end - bw, row_r.min.y + (BOX_ROW_H - 24.0) / 2.0), Vec2::new(bw, 24.0));
    let bid = Id::new(("chat-action", uid));
    let pressed = if action.2 {
        button(ui, br, bid, action.0, action.1).clicked()
    } else {
        // Nothing to send: the button is there, dimmed.
        rp.rect_filled(br, 5.0, theme::with_opacity(color, 0.35));
        text_in(&rp, br, Align2::CENTER_CENTER, action.0, theme::ui_semibold(11.5), theme::with_opacity(p.on_accent, 0.7));
        ui.interact(br, bid, Sense::hover());
        false
    };
    if pressed {
        acts.push(if !alive {
            Act::Restart
        } else if busy {
            Act::Interrupt
        } else {
            Act::Submit
        });
    }
    let mut cost_x = br.min.x - 10.0;
    if chat.transcript.cost_usd > 0.0 {
        let cost = format!("${:.2}", chat.transcript.cost_usd);
        let w = super::text_w(&rp, &cost, theme::meta(10.5));
        text_in(
            &rp,
            Rect::from_min_max(Pos2::new(cost_x - w, row_r.min.y), Pos2::new(cost_x, row_r.max.y)),
            Align2::RIGHT_CENTER,
            &cost,
            theme::meta(10.5),
            p.text_3,
        );
        cost_x -= w + 10.0;
    }
    let g = galley_trunc(&rp, &meta, theme::meta(10.5), p.text_3, (cost_x - x).max(0.0));
    super::place(&rp, Rect::from_min_max(Pos2::new(x, row_r.min.y), Pos2::new(cost_x, row_r.max.y)), Align2::LEFT_CENTER, g, p.text_3);

    // ---------- Under the box: the keys on the left, what is going on on the right ----------
    let np = ui.painter_at(note_r);
    let g = galley_trunc(&np, t!("Enter sends  ·  Shift+Enter new line  ·  ↑ history"), theme::meta(10.5), p.text_faint, col_w * 0.62);
    super::place(&np, note_r, Align2::LEFT_CENTER, g, p.text_faint);
    if alive && busy {
        let text = if pending.is_some() { t!("Waiting for your answer").to_owned() } else { working_text(chat) };
        let g = galley_trunc(&np, &text, theme::meta(10.5), color, col_w * 0.5);
        super::place(&np, note_r, Align2::RIGHT_CENTER, g, color);
        ctx.request_repaint_after(std::time::Duration::from_millis(500));
    }
    // Streaming text arrives between frames: keep drawing while the turn lasts.
    if busy {
        ctx.request_repaint_after(std::time::Duration::from_millis(120));
    }
    if submit {
        acts.push(Act::Submit);
    }
    // A press anywhere in the pane focuses it (a widget over the whole pane would swallow the clicks of the ones below).
    ui.input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| body.contains(p)))
}

fn working_text(chat: &ChatTab) -> String {
    let secs = chat.turn_started.map_or(0, |t| t.elapsed().as_secs());
    t!("working {time}  ·  Esc interrupts", time = format!("{:02}:{:02}", secs / 60, secs % 60))
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
    icon: Icon,
    /// The tab's agent color.
    color: egui::Color32,
    /// A permission card is waiting (the tool calls before it are not "running", they are waiting for the answer).
    waiting: bool,
    /// The item opens a run of the agent's turn (the avatar is drawn once per run).
    first_in_run: bool,
}

/// What the agent says and does: laid out under its avatar, in a column to the right of it.
fn is_agent_item(item: &Item) -> bool {
    matches!(item, Item::Assistant { .. } | Item::Thinking(_) | Item::Tool(_) | Item::Permission(_))
}

/// Width taken by the avatar column.
const AVATAR_COL: f32 = 32.0;

fn draw_item(ui: &mut Ui, item: &Item, col_w: f32, cx: &mut ItemCx) {
    let p = theme::pal();
    match item {
        Item::User { text, from: None } => {
            ui.with_layout(egui::Layout::top_down(Align::Max), |ui| {
                egui::Frame::new()
                    .fill(p.hover)
                    .stroke(Stroke::new(1.0, p.separator))
                    .corner_radius(CornerRadius { nw: 10, ne: 10, sw: 10, se: 3 })
                    .inner_margin(Margin::symmetric(12, 8))
                    .show(ui, |ui| {
                        ui.set_max_width(col_w * 0.82);
                        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(13.0)).color(p.text_strong)).selectable(true));
                    });
            });
        }
        Item::User { text, from: Some(who) } => {
            // A message from another agent: a recessed card with a dashed edge, like a note slipped in.
            let card = egui::Frame::new().fill(p.recess).corner_radius(8).inner_margin(Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.set_width(col_w - 24.0);
                ui.label(egui::RichText::new(t!("{agent} asks", agent = who).to_uppercase()).font(theme::label(10.0)).color(p.info));
                ui.add_space(3.0);
                ui.add(egui::Label::new(egui::RichText::new(text).font(theme::ui(13.0)).color(p.text)).selectable(true));
            });
            let r = card.response.rect.shrink(0.5);
            let outline = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
            ui.painter().extend(egui::Shape::dashed_line(&outline, Stroke::new(1.0, p.separator), 4.0, 3.0));
        }
        Item::Assistant { text, streaming, .. } => agent_row(ui, col_w, cx, |ui, _, cx| {
            markdown_style(ui);
            let shown = if *streaming { format!("{text} ▍") } else { text.clone() };
            egui_commonmark::CommonMarkViewer::new().show(ui, cx.markdown, &shown);
        }),
        Item::Thinking(text) => agent_row(ui, col_w, cx, |ui, _, _| {
            ui.label(egui::RichText::new(text).font(theme::ui(12.0)).italics().color(p.text_3));
        }),
        Item::Tool(call) => agent_row(ui, col_w, cx, |ui, w, cx| tool_card(ui, call, w, cx)),
        Item::Permission(card) => agent_row(ui, col_w, cx, |ui, w, cx| permission_card(ui, card, w, cx)),
        Item::Notice { text, error } => {
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(text).font(theme::ui(12.0)).color(if *error { p.danger_text } else { p.text_3 }));
            });
        }
    }
}

/// The agent's avatar (once per run of its items) and, beside it, the item in a column of `col_w - AVATAR_COL`.
fn agent_row(ui: &mut Ui, col_w: f32, cx: &mut ItemCx, body: impl FnOnce(&mut Ui, f32, &mut ItemCx)) {
    ui.horizontal_top(|ui| {
        let (mark, _) = ui.allocate_exact_size(Vec2::splat(22.0), Sense::hover());
        if cx.first_in_run {
            ui.painter().rect_filled(mark, 6.0, theme::accent_soft(cx.color));
            cx.icons.paint(ui.painter(), cx.icon, mark.center(), 12.0, cx.color, 0.0);
        }
        ui.add_space(AVATAR_COL - 22.0);
        let w = col_w - AVATAR_COL;
        ui.allocate_ui_with_layout(Vec2::new(w, 0.0), egui::Layout::top_down(Align::Min), |ui| {
            ui.set_max_width(w);
            // What still cannot wrap (a wide table, a long word) is cut at the column instead of drawn over the edge.
            let clip = ui.clip_rect().intersect(Rect::from_min_size(ui.max_rect().min, Vec2::new(w, f32::INFINITY)));
            ui.set_clip_rect(clip);
            body(ui, w, cx);
        });
    });
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
    // `egui_commonmark` lays bullets and table rows out in horizontal sub-layouts, where egui does not wrap text by
    // default: long lines ran past the column. Wrap everywhere inside the message.
    style.wrap_mode = Some(egui::TextWrapMode::Wrap);
}

// ---------- Tool calls ----------

#[derive(Clone, Copy, PartialEq, Debug)]
enum RowKind {
    Add,
    Del,
    /// A line of the file around a change.
    Context,
    /// The command a call runs.
    Plain,
    Output,
    Error,
}

struct Row {
    kind: RowKind,
    text: String,
    /// Line number in the file, when the result of the edit told it.
    no: Option<u32>,
}

fn rows_of(kind: RowKind, text: &str) -> impl Iterator<Item = Row> + '_ {
    text.lines().map(move |l| Row { kind, text: l.replace('\t', "    "), no: None })
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

/// The lines of an edit as the CLI's own patch gives them, with the numbers of the file.
fn patch_rows(patch: &[PatchHunk]) -> Vec<Row> {
    let mut rows = Vec::new();
    for hunk in patch {
        let (mut old, mut new) = (hunk.old_start, hunk.new_start);
        for line in &hunk.lines {
            let (kind, no) = match line.chars().next() {
                Some('-') => (RowKind::Del, old),
                Some('+') => (RowKind::Add, new),
                _ => (RowKind::Context, new),
            };
            if kind != RowKind::Add {
                old += 1;
            }
            if kind != RowKind::Del {
                new += 1;
            }
            rows.push(Row { kind, text: line.chars().skip(1).collect::<String>().replace('\t', "    "), no: Some(no) });
        }
    }
    rows
}

/// The lines under a call's header: the diff of an edit, the file being written, the command and its output.
fn tool_rows(call: &ToolCall) -> Vec<Row> {
    let mut rows = Vec::new();
    let input = &call.input;
    let edits = matches!(call.name.as_str(), "Edit" | "MultiEdit" | "Write");
    let patch = call.outcome.as_ref().map(|o| patch_rows(&o.patch)).unwrap_or_default();
    match call.name.as_str() {
        // The result's patch has the context and the line numbers; before it arrives, what the call says.
        "Edit" | "MultiEdit" if !patch.is_empty() => rows.extend(patch),
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
        "Write" => {
            rows.extend(rows_of(RowKind::Add, str_field(input, "content")).enumerate().map(|(n, mut r)| {
                r.no = u32::try_from(n + 1).ok();
                r
            }));
        }
        "Bash" => rows.extend(rows_of(RowKind::Plain, &format!("$ {}", str_field(input, "command")))),
        _ => {}
    }
    if let Some(out) = &call.outcome {
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

/// What a finished call reports on its header: the size of the diff, the lines read, the matches found.
fn tool_status(call: &ToolCall, rows: &[Row]) -> String {
    let Some(out) = &call.outcome else { return String::new() };
    let lines = out.text.lines().filter(|l| !l.trim().is_empty()).count();
    // A long output was cut when stored: its line count would be wrong.
    let whole = !out.text.contains("more characters)");
    match call.name.as_str() {
        "Edit" | "MultiEdit" | "Write" => {
            let (a, d) = diff_counts(rows);
            if a > 0 || d > 0 {
                return format!("+{a} −{d}");
            }
        }
        "Read" if whole => return if lines == 1 { t!("1 line").to_owned() } else { t!("{n} lines", n = lines) },
        "Grep" if whole => return if lines == 1 { t!("1 match").to_owned() } else { t!("{n} matches", n = lines) },
        "Glob" if whole => return if lines == 1 { t!("1 file").to_owned() } else { t!("{n} files", n = lines) },
        _ => {}
    }
    t!("done").to_owned()
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
        Some(_) => (tool_status(call, &rows), p.success),
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

/// The rows of a call's body, as a diff (with the line numbers in a gutter) or as output.
fn paint_rows(painter: &egui::Painter, area: Rect, rows: &[Row]) {
    let p = theme::pal();
    painter.rect_filled(Rect::from_min_max(area.min, Pos2::new(area.max.x, area.min.y + 1.0)), 0.0, p.separator);
    let numbered = rows.iter().any(|r| r.no.is_some());
    let gutter = if numbered { 38.0 } else { 22.0 };
    let mut y = area.min.y + 4.0;
    for row in rows.iter().take(BODY_ROWS) {
        let r = Rect::from_min_size(Pos2::new(area.min.x + 1.0, y), Vec2::new(area.width() - 2.0, ROW_H));
        let (bg, fg) = match row.kind {
            RowKind::Add => (Some(theme::with_alpha(p.success, 0.13)), theme::brighten(p.success, 1.25)),
            RowKind::Del => (Some(theme::with_alpha(p.danger, 0.13)), p.danger_text),
            RowKind::Context => (None, p.text_3),
            RowKind::Plain => (None, p.text_soft),
            RowKind::Output => (None, p.text_2),
            RowKind::Error => (None, p.danger_text),
        };
        if let Some(bg) = bg {
            painter.rect_filled(r, 0.0, bg);
        }
        // The gutter: the line number, or the +/− when the lines are not numbered.
        let (label, label_color) = match (row.no, row.kind) {
            (Some(n), _) => (n.to_string(), p.text_faint),
            (None, RowKind::Add) => ("+".to_owned(), p.success),
            (None, RowKind::Del) => ("−".to_owned(), p.danger_text),
            _ => (String::new(), p.text_faint),
        };
        if !label.is_empty() {
            text_in(
                painter,
                Rect::from_min_size(r.min, Vec2::new(gutter - 6.0, ROW_H)),
                Align2::RIGHT_CENTER,
                &label,
                theme::mono(10.5),
                label_color,
            );
        }
        let text_w = r.width() - gutter - 4.0;
        let g = galley_trunc(painter, &row.text, theme::mono(11.5), fg, text_w);
        super::place(painter, Rect::from_min_size(Pos2::new(r.min.x + gutter, y), Vec2::new(text_w, ROW_H)), Align2::LEFT_CENTER, g, fg);
        y += ROW_H;
    }
    if rows.len() > BODY_ROWS {
        let more = t!("… {n} more lines", n = rows.len() - BODY_ROWS);
        text_in(
            painter,
            Rect::from_min_size(Pos2::new(area.min.x + gutter, y), Vec2::new(area.width() - gutter, ROW_H)),
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
    let h = 11.0 + 20.0 + 8.0 + content_h + if content_h > 0.0 { 8.0 } else { 0.0 } + 26.0 + 11.0;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(col_w, h), Sense::hover());
    let painter = ui.painter_at(rect);
    // The card wears the agent's color, like the mockup: it is the agent asking.
    painter.rect_filled(rect, 10.0, theme::accent_soft(cx.color));
    painter.rect_stroke(rect, 10.0, Stroke::new(1.0, theme::accent_edge(cx.color)), egui::StrokeKind::Inside);
    let mut y = rect.min.y + 11.0;
    let tag = t!("Permission").to_uppercase();
    let tag_w = super::text_w(&painter, &tag, theme::label(10.0)) + 10.0;
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(rect.min.x + 12.0, y), Vec2::new(tag_w, 20.0)),
        Align2::LEFT_CENTER,
        &tag,
        theme::label(10.0),
        cx.color,
    );
    text_in(
        &painter,
        Rect::from_min_size(Pos2::new(rect.min.x + 12.0 + tag_w, y), Vec2::new(col_w - 24.0 - tag_w, 20.0)),
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
            let mut limited: Vec<Row> = rows.iter().take(shown).map(|r| Row { kind: r.kind, text: r.text.clone(), no: r.no }).collect();
            if rows.len() > shown {
                limited.push(Row { kind: RowKind::Output, text: t!("… {n} more lines", n = rows.len() - shown), no: None });
            }
            paint_rows(&painter, area, &limited);
        }
        y += content_h + 8.0;
    }
    // Buttons: the key each one answers to sits inside it.
    let mut x = rect.min.x + 12.0;
    let can_session = !card.suggestions.is_empty();
    let buttons: [(&str, Option<&str>, bool, bool); 3] = [
        (t!("Allow"), Some("Enter"), true, false),
        (t!("Allow for this session"), None, true, true),
        (t!("Deny"), Some("Esc"), false, false),
    ];
    for (n, (label, key, allow, for_session)) in buttons.into_iter().enumerate() {
        if for_session && !can_session {
            continue;
        }
        let label_w = super::text_w(&painter, label, theme::ui_semibold(11.5));
        let key_w = key.map_or(0.0, |k| super::text_w(&painter, k, theme::meta(10.0)) + 6.0);
        let w = label_w + key_w + 24.0;
        let r = Rect::from_min_size(Pos2::new(x, y), Vec2::new(w, 26.0));
        let resp = clickable(ui, r, Id::new(("perm-btn", &card.request_id, n)));
        let primary = allow && !for_session;
        let (fill, fg, key_fg) = if primary {
            let fill = if resp.hovered() { theme::brighten(cx.color, 1.1) } else { cx.color };
            (fill, p.on_accent, theme::with_opacity(p.on_accent, 0.6))
        } else {
            let fill = theme::with_alpha(p.text_strong, if resp.hovered() { 0.10 } else { 0.06 });
            (fill, if allow { p.text } else { p.danger_text }, p.text_3)
        };
        painter.rect_filled(r, 6.0, fill);
        if !primary {
            painter.rect_stroke(r, 6.0, Stroke::new(1.0, p.separator), egui::StrokeKind::Inside);
        }
        let font = if primary { theme::ui_semibold(11.5) } else { theme::ui_medium(11.5) };
        text_in(
            &painter,
            Rect::from_min_size(r.min + Vec2::new(12.0, 0.0), Vec2::new(label_w + 2.0, 26.0)),
            Align2::LEFT_CENTER,
            label,
            font,
            fg,
        );
        if let Some(key) = key {
            text_in(
                &painter,
                Rect::from_min_size(r.min + Vec2::new(12.0 + label_w + 6.0, 0.0), Vec2::new(key_w, 26.0)),
                Align2::LEFT_CENTER,
                key,
                theme::meta(10.0),
                key_fg,
            );
        }
        if resp.clicked() {
            cx.acts.push(Act::Answer { request_id: card.request_id.clone(), allow, for_session });
        }
        x += w + 6.0;
    }
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
            outcome: out.map(|(text, is_error)| ToolOutcome { tool_use_id: "t1".into(), text: text.into(), is_error, patch: vec![] }),
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
    fn an_edit_with_its_patch_shows_context_and_the_line_numbers_of_the_file() {
        let patch = vec![PatchHunk {
            old_start: 10,
            new_start: 10,
            lines: vec![" keep".into(), "-old".into(), "+new".into(), "+more".into(), " tail".into()],
        }];
        let rows = patch_rows(&patch);
        let shape: Vec<(RowKind, Option<u32>)> = rows.iter().map(|r| (r.kind, r.no)).collect();
        assert_eq!(
            shape,
            [
                (RowKind::Context, Some(10)),
                (RowKind::Del, Some(11)),
                (RowKind::Add, Some(11)),
                (RowKind::Add, Some(12)),
                (RowKind::Context, Some(13)),
            ]
        );
        assert_eq!(rows[1].text, "old");
        let c = ToolCall {
            id: "t1".into(),
            name: "Edit".into(),
            input: json!({ "old_string": "old", "new_string": "new\nmore" }),
            outcome: Some(ToolOutcome { tool_use_id: "t1".into(), text: "ok".into(), is_error: false, patch }),
        };
        assert_eq!(tool_rows(&c).len(), 5, "the patch replaces the guess made from the call");
        assert_eq!(tool_status(&c, &tool_rows(&c)), "+2 −1");
    }

    #[test]
    fn a_finished_read_or_search_says_how_much_it_found() {
        let c = |name: &str, text: &str| call(name, json!({}), Some((text, false)));
        assert_eq!(tool_status(&c("Read", "a\nb\nc"), &[]), "3 lines");
        assert_eq!(tool_status(&c("Grep", "a.rs"), &[]), "1 match");
        assert_eq!(tool_status(&c("Bash", "x"), &[]), "done");
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

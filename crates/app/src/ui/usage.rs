//! Subscription usage in the title bar: for each agent with quota, the model logo and two
//! stacked bars (5 h session on top, weekly on bottom) with percentages. Hovering an item opens
//! that agent's usage panel in detail (how much was used and when it renews; on Antigravity, by model
//! group, and clicking a group name pins it to the bar). Local agents have no quota and no item.

use std::collections::HashMap;

use egui::{Align2, Color32, Id, Order, Painter, Pos2, Rect, Sense, Ui, Vec2};
use holt_core::t;
use holt_core::usage::{UsageGroup, UsageWindow};
use holt_core::AgentUsage;

use crate::deck::Deck;
use crate::theme;

/// Width and height of the bars.
const BAR_W: f32 = 40.0;
const BAR_H: f32 = 4.0;
/// Line height of each percentage (the two stacked, 1 px between them).
const LINE_H: f32 = 10.0;
/// Column width of percentages ("100%").
const PCT_W: f32 = 22.0;
/// Padding, item height (hover background) and gap between items.
const ITEM_PAD: f32 = 6.0;
const ITEM_H: f32 = 26.0;
const ITEM_GAP: f32 = 8.0;
/// Width of the usage panel.
const PANEL_W: f32 = 340.0;
/// Above this threshold the bar warns (`warning`); at 100% it turns red.
const WARN_AT: f32 = 0.8;

/// Fill color: agent's color, up to warning and exhaustion.
pub fn bar_color(used: f32, agent: Color32) -> Color32 {
    if used >= 1.0 {
        theme::pal().danger
    } else if used >= WARN_AT {
        theme::pal().warning
    } else {
        agent
    }
}

/// Formats the duration until session renewal (e.g. "2h10", "45 min").
pub fn format_duration(ms: u64) -> String {
    let secs = ms / 1000;
    let hours = secs / 3600;
    let mins = (secs % 3600) / 60;
    if hours > 0 {
        format!("{hours}h{mins:02}")
    } else {
        format!("{} min", mins.max(1))
    }
}

fn local_time(ms: u64) -> Option<chrono::DateTime<chrono::Local>> {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => Some(t),
        chrono::LocalResult::None => None,
    }
}

/// Local time of an instant (e.g. "16:40").
pub fn format_clock(ms: u64) -> Option<String> {
    local_time(ms).map(|t| t.format("%H:%M").to_string())
}

/// Formats the day of the week and local time of weekly renewal (e.g. "Mon 14:00").
pub fn format_weekday_time(ms: u64) -> Option<String> {
    use chrono::{Datelike, Weekday};
    let dt = local_time(ms)?;
    let day = match dt.weekday() {
        Weekday::Mon => t!("Mon"),
        Weekday::Tue => t!("Tue"),
        Weekday::Wed => t!("Wed"),
        Weekday::Thu => t!("Thu"),
        Weekday::Fri => t!("Fri"),
        Weekday::Sat => t!("Sat"),
        Weekday::Sun => t!("Sun"),
    };
    Some(format!("{day} {}", dt.format("%H:%M")))
}

/// How long ago usage was read (e.g. "read 1 min ago").
pub fn format_read(ms: u64) -> String {
    let secs = ms / 1000;
    if secs < 60 {
        t!("read just now").to_owned()
    } else if secs < 3600 {
        let mins = secs / 60;
        t!("read {mins} min ago", mins)
    } else if secs < 48 * 3600 {
        let hours = secs / 3600;
        t!("read {hours} h ago", hours)
    } else {
        let days = secs / 86_400;
        t!("read {days} days ago", days)
    }
}

/// Window that has already renewed (`resets_at` in the past) without a new reading since then:
/// the read % is from before the renewal and is no longer valid.
pub fn expired(w: &UsageWindow, updated_ms: u64, now_ms: u64) -> bool {
    w.resets_at_ms.is_some_and(|r| r <= now_ms && updated_ms < r)
}

/// Usage as it applies now: expired windows zeroed (and without reset date, which has passed).
/// Also returns which windows expired (session, weekly), to fade and explain in the panel.
pub fn current(usage: &AgentUsage, now_ms: u64) -> (AgentUsage, bool, bool) {
    let mut u = usage.clone();
    let zero = |w: &mut Option<UsageWindow>| match w {
        Some(win) if expired(win, usage.updated_ms, now_ms) => {
            *win = UsageWindow { used: 0.0, resets_at_ms: None };
            true
        }
        _ => false,
    };
    let (s, wk) = (zero(&mut u.session), zero(&mut u.weekly));
    (u, s, wk)
}

/// Next upcoming renewal (to repaint when it passes), including groups: the title bar
/// might be showing a pinned group that renews at a different time.
fn next_reset(usage: &AgentUsage, now_ms: u64) -> Option<u64> {
    let groups = usage.groups.iter().flat_map(|g| [g.session, g.weekly]);
    [usage.session, usage.weekly].into_iter().chain(groups).flatten().filter_map(|w| w.resets_at_ms).filter(|&r| r > now_ms).min()
}

/// Has quota to show (not local and source reported at least one window).
pub fn has_quota(usage: &AgentUsage) -> bool {
    !usage.local && (usage.session.is_some() || usage.weekly.is_some())
}

/// Agent order on the bar: matching the agents menu, followed by those
/// not in the menu in alphabetical order.
fn ordered_models(available: &[String], usage: &HashMap<String, AgentUsage>) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for id in available {
        let key = crate::split::group_key(id);
        if usage.contains_key(key) && !out.iter().any(|k| k == key) {
            out.push(key.to_owned());
        }
    }
    let mut rest: Vec<String> = usage.keys().filter(|k| !out.contains(k)).cloned().collect();
    rest.sort();
    out.extend(rest);
    out
}

/// A window as it appears now: fraction used (0..=1), when it renews, and whether it renewed
/// without a new reading (faded). `None` when source does not report this window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bar {
    pub used: f32,
    pub resets_at_ms: Option<u64>,
    pub faded: bool,
}

/// Session and weekly windows of an agent, as they apply now.
pub fn bars(usage: &AgentUsage, now_ms: u64) -> [Option<Bar>; 2] {
    let (fresh, session_gone, weekly_gone) = current(usage, now_ms);
    let bar = |w: Option<UsageWindow>, faded| w.map(|w| Bar { used: w.used.clamp(0.0, 1.0), resets_at_ms: w.resets_at_ms, faded });
    [bar(fresh.session, session_gone), bar(fresh.weekly, weekly_gone)]
}

/// Model groups of an agent (name, session, and weekly), as they apply now.
pub fn group_bars(usage: &AgentUsage, now_ms: u64) -> Vec<(String, [Option<Bar>; 2])> {
    let as_usage = |g: &UsageGroup| AgentUsage { session: g.session, weekly: g.weekly, updated_ms: usage.updated_ms, ..Default::default() };
    usage.groups.iter().map(|g| (g.name.clone(), bars(&as_usage(g), now_ms))).collect()
}

/// Bottom line of each window in the panel: when it renews ("renews in 2h10 · at 16:40" for session,
/// "renews Mon 14:00" for weekly) or why there is no number.
pub fn reset_text(bar: Option<Bar>, weekly: bool, now_ms: u64) -> String {
    let Some(bar) = bar else { return t!("not reported by source").to_owned() };
    if bar.faded {
        return t!("renewed · awaiting new reading").to_owned();
    }
    let Some(at) = bar.resets_at_ms.filter(|&r| r > now_ms) else { return String::new() };
    if weekly {
        format_weekday_time(at).map(|t| t!("renews {weekday_time}", weekday_time = t)).unwrap_or_default()
    } else {
        match format_clock(at) {
            Some(clock) => t!("renews in {duration} · at {clock}", duration = format_duration(at - now_ms), clock),
            None => t!("renews in {duration}", duration = format_duration(at - now_ms)),
        }
    }
}

/// Label for an agent that exhausted its session ("blocked until 15:10").
pub fn blocked_text(session: Option<Bar>, now_ms: u64) -> Option<String> {
    let s = session.filter(|s| s.used >= 1.0 && !s.faded)?;
    Some(match s.resets_at_ms.filter(|&r| r > now_ms).and_then(format_clock) {
        Some(clock) => t!("blocked until {clock}", clock),
        None => t!("blocked").to_owned(),
    })
}

/// Percentage text ("28%", or "–" without data).
fn pct_text(bar: Option<Bar>) -> String {
    bar.map(|b| format!("{}%", (b.used * 100.0).round() as i32)).unwrap_or_else(|| "–".to_owned())
}

/// Model logo (dot if no logo) at `size` px.
fn paint_mark(deck: &mut Deck, p: &Painter, center: Pos2, kind: &str, color: Color32, size: f32) {
    match super::projects::agent_icon(kind) {
        Some(icon) => deck.icons.paint(p, icon, center, size, color, 0.0),
        None => super::dot(p, center, color),
    }
}

/// A bar (track and fill) of `w`×`h` with vertical center at `cy`.
fn paint_bar(p: &Painter, x: f32, cy: f32, w: f32, h: f32, bar: Option<Bar>, agent: Color32) {
    let track = Rect::from_min_size(Pos2::new(x, cy - h / 2.0), Vec2::new(w, h));
    p.rect_filled(track, h / 2.0, theme::pal().separator);
    if let Some(b) = bar.filter(|b| b.used > 0.0) {
        let alpha = if b.faded { 0.5 } else { 1.0 };
        let fill = Rect::from_min_size(track.min, Vec2::new((w * b.used).max(h), h));
        p.rect_filled(fill, h / 2.0, theme::with_opacity(bar_color(b.used, agent), alpha));
    }
}

/// Percentage color: red at 100%, faded if window renewed without a new reading.
fn pct_color(bar: Option<Bar>, base: Color32) -> Color32 {
    match bar {
        Some(b) if b.used >= 1.0 && !b.faded => theme::pal().danger_text,
        Some(b) if b.faded => theme::with_opacity(base, 0.5),
        Some(_) => base,
        None => theme::pal().text_3,
    }
}

/// An agent with quota, as rendered by the bar and panel.
struct Row {
    model: String,
    name: String,
    color: Color32,
    /// What the title bar shows: pinned group or, without one, the summary.
    bars: [Option<Bar>; 2],
    /// Model groups (only Antigravity has these); with them, panel shows each group instead of summary.
    groups: Vec<(String, [Option<Bar>; 2])>,
    /// Group pinned to the bar, if it still exists.
    pinned: Option<String>,
    updated_ms: u64,
}

/// Title bar bars and effective pinned group: group `pin`, if source still returns it;
/// otherwise, the summary (`summary`) and no pinned group.
fn shown_bars(summary: [Option<Bar>; 2], groups: &[(String, [Option<Bar>; 2])], pin: Option<&str>) -> ([Option<Bar>; 2], Option<String>) {
    match pin.and_then(|pin| groups.iter().find(|(name, _)| name == pin)) {
        Some((name, bars)) => (*bars, Some(name.clone())),
        None => (summary, None),
    }
}

/// Draws one item per agent with quota (data from `Deck::usage`, with or without open window),
/// right-aligned up to `rect.max.x` and centered vertically in `rect`, plus the usage panel while
/// mouse is over an item or over the panel. Returns the leftmost x coordinate used (`rect.max.x`
/// if nothing was drawn), so the bar knows where the drag area ends.
pub fn draw(deck: &mut Deck, ui: &Ui, rect: Rect) -> f32 {
    let usage_map = deck.usage();
    let now_ms = holt_core::util::now_ms();
    let mut rows = Vec::new();
    let available: Vec<String> = deck.agents.available.iter().map(|a| a.id.clone()).collect();
    for model in ordered_models(&available, &usage_map) {
        let Some(raw) = usage_map.get(&model) else { continue };
        if !has_quota(raw) {
            continue;
        }
        if let Some(at) = next_reset(raw, now_ms) {
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(at - now_ms + 50));
        }
        let groups = group_bars(raw, now_ms);
        let (shown, pinned) = shown_bars(bars(raw, now_ms), &groups, deck.usage_pins.get(&model).map(String::as_str));
        rows.push(Row {
            name: deck.agents.name(&model),
            color: deck.agents.tab_color(&model),
            bars: shown,
            groups,
            pinned,
            updated_ms: raw.updated_ms,
            model,
        });
    }
    if rows.is_empty() || rect.width() <= 0.0 {
        return rect.max.x;
    }

    let painter = ui.painter();
    let p = theme::pal();
    let item_w = ITEM_PAD + 12.0 + 5.0 + BAR_W + 4.0 + PCT_W + ITEM_PAD;
    let total = rows.len() as f32 * item_w + (rows.len() - 1) as f32 * ITEM_GAP;
    let start_x = rect.max.x - total;
    let cy = rect.center().y;
    let (row1, row2) = (cy - (LINE_H + 1.0) / 2.0, cy + (LINE_H + 1.0) / 2.0);
    let panel_key = Id::new("usage-panel");
    // Panel open in previous frame: (highlighted agent, panel rect).
    let open_before = ui.ctx().data(|d| d.get_temp::<(String, Rect)>(panel_key));
    let group = Rect::from_min_max(Pos2::new(start_x, cy - ITEM_H / 2.0), Pos2::new(rect.max.x, cy + ITEM_H / 2.0));
    let item_at =
        |i: usize| Rect::from_min_size(Pos2::new(start_x + i as f32 * (item_w + ITEM_GAP), cy - ITEM_H / 2.0), Vec2::new(item_w, ITEM_H));
    // Panel stays open while pointer is over an item, the panel, or the gap between them. It shows the agent
    // of the item under mouse; inside the panel, the last one hovered. Decided before painting: an item only highlights.
    let pointer = ui.ctx().pointer_hover_pos();
    let hovered = pointer.and_then(|ptr| (0..rows.len()).find(|&i| item_at(i).contains(ptr))).map(|i| rows[i].model.clone());
    let keep = open_before.as_ref().is_some_and(|(_, panel)| pointer.is_some_and(|ptr| group.union(*panel).contains(ptr)));
    let focus = hovered.or_else(|| open_before.filter(|_| keep).map(|(m, _)| m));
    let focus = focus.or_else(|| {
        let wanted = deck.debug_usage_panel.as_deref()?;
        Some(if wanted.is_empty() { rows[0].model.clone() } else { wanted.to_owned() })
    });
    for (i, row) in rows.iter().enumerate() {
        let item = item_at(i);
        ui.interact(item, Id::new(("usage-item", &row.model)), Sense::hover());
        if focus.as_deref() == Some(row.model.as_str()) {
            painter.rect_filled(item, 5.0, p.hover);
        }
        let mut ix = item.min.x + ITEM_PAD;
        paint_mark(deck, painter, Pos2::new(ix + 6.0, cy), &row.model, row.color, 12.0);
        ix += 12.0 + 5.0;
        paint_bar(painter, ix, row1, BAR_W, BAR_H, row.bars[0], row.color);
        paint_bar(painter, ix, row2, BAR_W, BAR_H, row.bars[1], row.color);
        ix += BAR_W + 4.0;
        for (bar, y) in [(row.bars[0], row1), (row.bars[1], row2)] {
            let r = Rect::from_center_size(Pos2::new(ix + PCT_W / 2.0, y), Vec2::new(PCT_W, LINE_H));
            super::text_in(painter, r, Align2::RIGHT_CENTER, &pct_text(bar), theme::meta(8.0), pct_color(bar, p.text_soft));
        }
    }
    match focus.and_then(|m| rows.iter().position(|r| r.model == m)) {
        Some(i) => {
            let panel = draw_panel(deck, ui.ctx(), item_at(i), &rows[i], now_ms);
            let model = rows[i].model.clone();
            // "renews in" and "read ago" advance with open panel, even with mouse stationary.
            ui.ctx().request_repaint_after(std::time::Duration::from_secs(30));
            ui.ctx().data_mut(|d| d.insert_temp(panel_key, (model, panel)));
        }
        None => ui.ctx().data_mut(|d| d.remove::<(String, Rect)>(panel_key)),
    }
    start_x.max(rect.min.x)
}

/// In an agent block in the panel: header (with space before first window), each window (with space
/// after it), model group name, and bottom margin.
const BLOCK_HEAD_H: f32 = 10.0 + 18.0 + 9.0;
const WINDOW_H: f32 = 42.0 + 9.0;
const GROUP_NAME_H: f32 = 16.0 + 2.0;
/// Extra space before a group that is not the first (separates from "renews" of the one above).
const GROUP_GAP: f32 = 4.0;
const BLOCK_PAD_B: f32 = 3.0;

/// Windows of a block, each with the group it belongs to: without groups, summary session and week
/// (always both, "not reported by source" for missing ones); with groups, only those that each group has.
fn block_lines(row: &Row) -> Vec<(Option<&str>, usize, Option<Bar>)> {
    if row.groups.is_empty() {
        return (0..2).map(|i| (None, i, row.bars[i])).collect();
    }
    let mut out = Vec::new();
    for (name, bars) in &row.groups {
        out.extend((0..2).filter(|&i| bars[i].is_some()).map(|i| (Some(name.as_str()), i, bars[i])));
    }
    out
}

/// Height of an agent block in the panel.
fn block_h(row: &Row) -> f32 {
    let groups = row.groups.len() as f32;
    BLOCK_HEAD_H + groups * GROUP_NAME_H + (groups - 1.0).max(0.0) * GROUP_GAP + block_lines(row).len() as f32 * WINDOW_H + BLOCK_PAD_B
}

/// Usage panel of an agent, below its item (`item`), right-aligned; clicking a group name
/// pins it to the bar (or unpins if already pinned). Returns its rect.
fn draw_panel(deck: &mut Deck, ctx: &egui::Context, item: Rect, row: &Row, now_ms: u64) -> Rect {
    let p = theme::pal();
    let h = 12.0 + super::SECTION_LABEL_H + 2.0 + block_h(row) + 6.0;
    let size = Vec2::new(PANEL_W, h);
    let pos = super::menus::keep_on_screen(ctx, Pos2::new(item.max.x - PANEL_W, item.max.y + 4.0), size);
    let rect = Rect::from_min_size(pos, size);
    let mut pin_click = None;
    egui::Area::new(Id::new("usage-panel-area")).order(Order::Foreground).fixed_pos(pos).constrain(false).show(ctx, |ui| {
        let painter = ui.painter();
        super::shadow(painter, rect, 10, 8, 24, p.shadow);
        super::frame(painter, rect, 10.0, p.popup, p.glass_border);
        let head = Rect::from_min_size(Pos2::new(rect.min.x + 4.0, rect.min.y + 12.0), Vec2::new(PANEL_W - 4.0, super::SECTION_LABEL_H));
        super::section_label(painter, head, t!("SUBSCRIPTION USAGE"));
        if let Some(t) = local_time(now_ms) {
            let when = t.format("%d/%m · %H:%M").to_string();
            super::text_in(painter, head.shrink2(Vec2::new(14.0, 0.0)), Align2::RIGHT_CENTER, &when, theme::meta(10.0), p.text_3);
        }
        let block = Rect::from_min_size(Pos2::new(rect.min.x + 6.0, head.max.y + 2.0), Vec2::new(PANEL_W - 12.0, block_h(row)));
        pin_click = agent_block(deck, ui, block, row, now_ms);
    });
    if let Some(group) = pin_click {
        deck.set_usage_pin(&row.model, (row.pinned.as_ref() != Some(&group)).then_some(group));
    }
    rect
}

/// An agent in the panel: logo, name, blocked label and "read ago"; session and week (for each model
/// group, when there are groups, under its name), each with percentage, a 6 px bar and renewal time.
/// The name of the group pinned to the bar carries the label "on bar". Returns the group clicked.
fn agent_block(deck: &mut Deck, ui: &Ui, block: Rect, row: &Row, now_ms: u64) -> Option<String> {
    let p = theme::pal();
    let painter = ui.painter();
    let mut clicked = None;
    let (x0, x1) = (block.min.x + 14.0, block.max.x - 14.0);
    let mut y = block.min.y + 10.0;
    // Agent header.
    let cy = y + 9.0;
    paint_mark(deck, painter, Pos2::new(x0 + 7.0, cy), &row.model, row.color, 14.0);
    let name_r = Rect::from_min_max(Pos2::new(x0 + 22.0, y), Pos2::new(x1, y + 18.0));
    let read = if row.updated_ms > 0 { format_read(now_ms.saturating_sub(row.updated_ms)) } else { String::new() };
    let read_w = super::text_in(painter, name_r, Align2::RIGHT_CENTER, &read, theme::meta(10.0), p.text_3).width();
    let name = super::galley_trunc(painter, &row.name, theme::ui_semibold(13.0), p.text_strong, (name_r.width() - read_w - 8.0).max(20.0));
    let name_end = super::place(painter, name_r, Align2::LEFT_CENTER, name, p.text_strong).max.x;
    if let Some(tag) = blocked_text(row.bars[0], now_ms) {
        let font = theme::ui_semibold(10.5);
        let w = super::text_w(painter, &tag, font.clone()) + 12.0;
        if name_end + 8.0 + w <= x1 - read_w - 8.0 {
            let chip = Rect::from_min_size(Pos2::new(name_end + 8.0, cy - 8.0), Vec2::new(w, 16.0));
            painter.rect_filled(chip, 4.0, theme::with_alpha(p.danger, 0.14));
            super::text_in(painter, chip, Align2::CENTER_CENTER, &tag, font, p.danger_text);
        }
    }
    y += 18.0 + 9.0;
    // Session and week (by group, if any).
    let mut group_before = None;
    for (group, i, bar) in block_lines(row) {
        if group.is_some() && group != group_before {
            if group_before.is_some() {
                y += GROUP_GAP;
            }
            let name = group.unwrap_or_default();
            let name_r = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + 16.0));
            let resp = super::clickable(ui, name_r.expand2(Vec2::new(6.0, 1.0)), Id::new(("usage-pin", &row.model, name)));
            let pinned = row.pinned.as_deref() == Some(name);
            if resp.hovered() {
                painter.rect_filled(resp.rect, 4.0, p.hover);
            }
            let tag = match (pinned, resp.hovered()) {
                (true, true) => t!("unpin from bar"),
                (true, false) => t!("on bar"),
                (false, true) => t!("pin to bar"),
                (false, false) => "",
            };
            let mut tag_w = 0.0;
            if !tag.is_empty() {
                let font = theme::meta(10.0);
                tag_w = super::text_w(painter, tag, font.clone()) + 12.0;
                let chip = Rect::from_center_size(Pos2::new(x1 - tag_w / 2.0, name_r.center().y), Vec2::new(tag_w, 16.0));
                if pinned {
                    painter.rect_filled(chip, 4.0, theme::with_alpha(row.color, 0.22));
                }
                super::text_in(painter, chip, Align2::CENTER_CENTER, tag, font, p.text_strong);
            }
            let g = super::galley_trunc(painter, name, theme::ui_semibold(11.5), p.text_2, name_r.width() - tag_w - 8.0);
            super::place(painter, name_r, Align2::LEFT_CENTER, g, p.text_2);
            if resp.clicked() {
                clicked = Some(name.to_owned());
            }
            let tip = if pinned {
                t!("The title bar shows this group. Click to return to the summary.")
            } else {
                t!("Show this group in the title bar")
            };
            super::tip(resp, tip);
            y += GROUP_NAME_H;
            group_before = group;
        }
        let (label, span) = if i == 0 { (t!("Session"), t!("5 h")) } else { (t!("Week"), t!("7 days")) };
        let line = Rect::from_min_max(Pos2::new(x0, y), Pos2::new(x1, y + 16.0));
        let l = super::text_in(painter, line, Align2::LEFT_CENTER, label, theme::ui(12.0), p.text_soft);
        let span_r = Rect::from_min_max(Pos2::new(l.max.x + 6.0, y), Pos2::new(x1, y + 16.0));
        super::text_in(painter, span_r, Align2::LEFT_CENTER, span, theme::meta(10.0), p.text_3);
        super::text_in(painter, line, Align2::RIGHT_CENTER, &pct_text(bar), theme::ui_semibold(13.0), pct_color(bar, p.text_strong));
        paint_bar(painter, x0, y + 16.0 + 4.0 + 3.0, x1 - x0, 6.0, bar, row.color);
        let note = Rect::from_min_max(Pos2::new(x0, y + 16.0 + 4.0 + 6.0 + 4.0), Pos2::new(x1, y + 42.0));
        super::text_in(painter, note, Align2::LEFT_CENTER, &reset_text(bar, i == 1, now_ms), theme::meta(10.0), p.text_3);
        y += WINDOW_H;
    }
    clicked
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::i18n::{with_lang, Lang};

    fn w(used: f32, resets: Option<u64>) -> Option<UsageWindow> {
        Some(UsageWindow { used, resets_at_ms: resets })
    }

    #[test]
    fn the_bar_keeps_the_agent_color_until_the_quota_runs_low() {
        let agent = theme::CLAUDE;
        assert_eq!(bar_color(0.0, agent), agent);
        assert_eq!(bar_color(0.79, agent), agent);
        assert_eq!(bar_color(0.80, agent), theme::pal().warning);
        assert_eq!(bar_color(0.999, agent), theme::pal().warning);
        assert_eq!(bar_color(1.0, agent), theme::pal().danger);
        assert_eq!(bar_color(1.5, agent), theme::pal().danger);
    }

    #[test]
    fn each_window_gets_its_own_bar() {
        let u = AgentUsage { session: w(0.28, Some(9_000)), weekly: w(1.7, None), updated_ms: 1, ..Default::default() };
        let [s, wk] = bars(&u, 100);
        assert_eq!(s, Some(Bar { used: 0.28, resets_at_ms: Some(9_000), faded: false }));
        // Values outside the range clamp to 100%.
        assert_eq!(wk.map(|b| b.used), Some(1.0));
        // Window that the source does not report.
        let only_session = AgentUsage { session: w(0.2, None), ..Default::default() };
        assert_eq!(bars(&only_session, 100)[1], None);
        assert_eq!(pct_text(None), "–");
        assert_eq!(pct_text(s), "28%");
        // Has quota: not local and with at least one window.
        assert!(has_quota(&only_session));
        assert!(!has_quota(&AgentUsage { local: true, ..Default::default() }));
        assert!(!has_quota(&AgentUsage::default()));
    }

    #[test]
    fn with_groups_the_panel_lists_each_group_and_only_the_windows_it_has() {
        let g = |name: &str, s: Option<UsageWindow>, wk: Option<UsageWindow>| UsageGroup { name: name.into(), session: s, weekly: wk };
        let u = AgentUsage {
            session: w(0.1, None),
            weekly: w(0.94, None),
            updated_ms: 1,
            groups: vec![g("Gemini Models", w(0.1, None), w(0.0, None)), g("Gemini 4 Argon", None, w(0.94, None))],
            ..Default::default()
        };
        let row = |groups| Row {
            model: "antigravity".into(),
            name: "Antigravity".into(),
            color: theme::ANTIGRAVITY,
            bars: bars(&u, 100),
            groups,
            pinned: None,
            updated_ms: 1,
        };
        let with = row(group_bars(&u, 100));
        let lines: Vec<(Option<&str>, usize)> = block_lines(&with).into_iter().map(|(g, i, _)| (g, i)).collect();
        assert_eq!(lines, [(Some("Gemini Models"), 0), (Some("Gemini Models"), 1), (Some("Gemini 4 Argon"), 1)]);
        assert_eq!(block_h(&with), BLOCK_HEAD_H + 2.0 * GROUP_NAME_H + GROUP_GAP + 3.0 * WINDOW_H + BLOCK_PAD_B);
        // Without groups: the two summary windows at standard height.
        let without = row(Vec::new());
        assert_eq!(block_lines(&without).len(), 2);
        assert_eq!(block_h(&without), 10.0 + 18.0 + 9.0 + 42.0 + 9.0 + 42.0 + 12.0);
    }

    #[test]
    fn the_title_bar_shows_the_pinned_group_while_it_exists() {
        let b = |used| Some(Bar { used, resets_at_ms: None, faded: false });
        let summary = [b(0.1), b(0.94)];
        let groups = vec![("Gemini Models".to_owned(), [b(0.1), b(0.2)]), ("Gemini 4 Argon".to_owned(), [None, b(0.94)])];
        assert_eq!(shown_bars(summary, &groups, None), (summary, None));
        assert_eq!(shown_bars(summary, &groups, Some("Gemini Models")), ([b(0.1), b(0.2)], Some("Gemini Models".to_owned())));
        // Argon has no session: bar shows "–" instead of picking another group's.
        assert_eq!(shown_bars(summary, &groups, Some("Gemini 4 Argon")).0, [None, b(0.94)]);
        // Group no longer returned by source: reverts to summary.
        assert_eq!(shown_bars(summary, &groups, Some("Gemini 3 Pro")), (summary, None));
    }

    #[test]
    fn a_window_past_its_reset_without_new_data_is_zeroed_and_faded() {
        let usage = AgentUsage { session: w(0.9, Some(1_000)), weekly: w(0.4, Some(9_000)), updated_ms: 500, ..Default::default() };
        // Before renewal: nothing changes.
        assert_eq!(current(&usage, 900), (usage.clone(), false, false));
        // After: session zeroes, loses reset date, and fades; weekly (renews later) remains.
        let [s, wk] = bars(&usage, 2_000);
        assert_eq!(s, Some(Bar { used: 0.0, resets_at_ms: None, faded: true }));
        assert_eq!(wk, Some(Bar { used: 0.4, resets_at_ms: Some(9_000), faded: false }));
        assert_eq!(with_lang(Lang::En, || reset_text(s, false, 2_000)), "renewed · awaiting new reading");
        assert_eq!(with_lang(Lang::PtBr, || reset_text(s, false, 2_000)), "renovou · aguardando leitura nova");
        // Reading made after renewal: fresh data, valid as is.
        let fresh = AgentUsage { updated_ms: 1_500, ..usage.clone() };
        assert!(!current(&fresh, 2_000).1);
        assert_eq!(next_reset(&usage, 2_000), Some(9_000));
    }

    #[test]
    fn the_panel_says_when_each_window_renews_and_who_is_blocked() {
        let now = 10_000_000;
        let session = Some(Bar { used: 0.34, resets_at_ms: Some(now + 7_800_000), faded: false });
        with_lang(Lang::En, || {
            assert!(reset_text(session, false, now).starts_with("renews in 2h10 · at "));
            let weekly = Some(Bar { used: 0.12, resets_at_ms: Some(now + 86_400_000), faded: false });
            assert!(reset_text(weekly, true, now).starts_with("renews "));
            assert_eq!(reset_text(None, true, now), "not reported by source");
            // Without reset date: nothing to report.
            assert_eq!(reset_text(Some(Bar { used: 0.3, resets_at_ms: None, faded: false }), false, now), "");
            // Exhausted session blocks (with time if source provides it); faded or below 100% does not.
            let full = Some(Bar { used: 1.0, resets_at_ms: Some(now + 60_000), faded: false });
            assert!(blocked_text(full, now).is_some_and(|t| t.starts_with("blocked until ")));
            assert_eq!(blocked_text(Some(Bar { used: 1.0, resets_at_ms: None, faded: false }), now).as_deref(), Some("blocked"));
            assert_eq!(blocked_text(Some(Bar { used: 1.0, resets_at_ms: None, faded: true }), now), None);
            assert_eq!(blocked_text(session, now), None);
        });

        with_lang(Lang::PtBr, || {
            assert!(reset_text(session, false, now).starts_with("renova em 2h10 · às "));
            let weekly = Some(Bar { used: 0.12, resets_at_ms: Some(now + 86_400_000), faded: false });
            assert!(reset_text(weekly, true, now).starts_with("renova "));
            assert_eq!(reset_text(None, true, now), "a fonte não informa");
            let full = Some(Bar { used: 1.0, resets_at_ms: Some(now + 60_000), faded: false });
            assert!(blocked_text(full, now).is_some_and(|t| t.starts_with("bloqueado até ")));
            assert_eq!(blocked_text(Some(Bar { used: 1.0, resets_at_ms: None, faded: false }), now).as_deref(), Some("bloqueado"));
        });
    }

    #[test]
    fn agents_follow_the_menu_order_and_the_unknown_come_last() {
        let usage: HashMap<String, AgentUsage> =
            ["zeta", "codex", "claude", "aider"].into_iter().map(|k| (k.to_owned(), AgentUsage::default())).collect();
        let menu = ["claude", "antigravity", "opencode", "codex"].map(String::from);
        assert_eq!(ordered_models(&menu, &usage), ["claude", "codex", "aider", "zeta"]);
    }

    #[test]
    fn time_formatting() {
        assert_eq!(format_duration(2 * 3600 * 1000 + 10 * 60 * 1000), "2h10");
        assert_eq!(format_duration(45 * 60 * 1000), "45 min");
        assert_eq!(format_duration(10 * 1000), "1 min");

        with_lang(Lang::En, || {
            assert_eq!(format_read(20 * 1000), "read just now");
            assert_eq!(format_read(60 * 1000), "read 1 min ago");
            assert_eq!(format_read(5 * 60 * 1000), "read 5 min ago");
            assert_eq!(format_read(3 * 3600 * 1000), "read 3 h ago");
            assert_eq!(format_read(3 * 86_400 * 1000), "read 3 days ago");
        });

        with_lang(Lang::PtBr, || {
            assert_eq!(format_read(20 * 1000), "lido agora");
            assert_eq!(format_read(60 * 1000), "lido há 1 min");
            assert_eq!(format_read(5 * 60 * 1000), "lido há 5 min");
            assert_eq!(format_read(3 * 3600 * 1000), "lido há 3 h");
            assert_eq!(format_read(3 * 86_400 * 1000), "lido há 3 dias");
        });
    }
}

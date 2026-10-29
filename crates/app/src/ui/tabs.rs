//! Tab strip of each window's header on the split screen (it used to be in the topbar). The same drawing
//! (scroll, ‹ › arrows, ▾ menu) serves the document strip of the Documents mode (`ui/docs.rs`): the strip
//! paints a list of [`Item`], and `strip` builds that list from the agent tabs.

use egui::{Align2, Area, Color32, Id, Order, Painter, Pos2, Rect, Sense, Stroke, Ui, Vec2};

use super::menus::{closes_on_outside, keep_on_screen};
use super::{clickable, dot, frame, galley_trunc, place, shadow, text_in, text_w, tip};
use crate::deck::Deck;
use crate::dictation::{self, ButtonVisual, MicTarget};
use crate::theme;
use holt_core::t;

/// What a click on the strip asks for. The numbers are positions in the item list (`strip` turns them into
/// the indexes of `Deck::tabs`).
pub enum StripClick {
    Activate(usize),
    Close(usize),
    Rename(usize),
    ContextMenu(usize, Pos2),
}

/// What comes before the tab name.
#[derive(Clone, Copy)]
pub enum Lead {
    /// The agent's dot (pulses while working, gets the ring when it finishes unseen).
    Dot,
    /// A sign of the document type.
    Glyph(&'static str),
}

/// One tab of the strip.
pub struct Item {
    /// Identifies the tab between frames (interaction ids).
    pub uid: u64,
    pub label: String,
    pub color: Color32,
    pub alive: bool,
    pub busy: bool,
    /// Finished a turn without being seen.
    pub done: bool,
    /// The dictation state in this tab.
    pub mic: ButtonVisual,
    pub lead: Lead,
    /// The hint when the mouse passes over it.
    pub tip: String,
    /// Has unsaved edits (document): a dot in place of the "✕" until the mouse reaches it.
    pub dirty: bool,
}

/// How the strip behaves: the id prefix (agent and document tabs do not mix), whether a double click
/// renames and a right click opens the tab menu, and whether the active pill takes the agent's color
/// (a document has no color: the active one stays `selected`).
#[derive(Clone, Copy)]
pub struct StripKind {
    pub scope: &'static str,
    pub tab_menu: bool,
    pub colored: bool,
}

const TAB_FONT: f32 = 11.5;
const LABEL_MAX: f32 = 140.0;
/// One tab's pill: 22 px tall, 8 px padding, 3 px between them and a 4 px margin in the strip.
const PILL_H: f32 = 22.0;
const PILL_PAD: f32 = 8.0;
const PILL_GAP: f32 = 3.0;
const STRIP_PAD: f32 = 4.0;
/// The dot box (7 px) and the "✕" box (16 px, the click target).
const DOT_BOX: f32 = 7.0;
const CLOSE_BOX: f32 = 16.0;
/// Space of the pulsing red indicator while the tab is recording audio.
const REC_INDICATOR_W: f32 = 14.0;
/// Scroll arrow of the hidden tabs: a 20×20 button (like the header ones) over a solid background,
/// followed by the gradient that links it to the tabs.
const ARROW_BTN: f32 = 20.0;
const ARROW_ZONE_W: f32 = 24.0;
const FADE_W: f32 = 18.0;
/// Space of the "▾" button (all tabs), reserved at the end of the strip when it scrolls.
const LIST_W: f32 = 26.0;
const LIST_MENU_W: f32 = 260.0;
const LIST_ITEM_H: f32 = 26.0;
const LIST_MAX_ROWS: usize = 14;
/// Slack when deciding whether a tab is entirely in view (avoids an arrow because of a pixel fraction).
const EDGE_EPS: f32 = 0.5;

/// Width of a pill: padding, dot, name (`label_w`), what comes after it (`extra_w`: dictation state),
/// the "✕" and the padding. The space up to the next one does not enter.
fn pill_width(label_w: f32, extra_w: f32) -> f32 {
    PILL_PAD + DOT_BOX + PILL_GAP + label_w + extra_w + PILL_GAP + CLOSE_BOX + PILL_PAD
}

/// Tabs that do not show up whole on each side of the strip (position in the window list).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
struct Hidden {
    /// The one closest to the left edge.
    left: Option<usize>,
    /// The one closest to the right edge.
    right: Option<usize>,
}

/// Which tabs are cut off or outside the area of width `view_w`, with the strip scrolled by `scroll`.
/// From a side that has a hidden tab the arrow comes in, covering `edge` px of the edge: there the tab
/// that only shows up beneath it also counts as hidden.
fn hidden_tabs(widths: &[f32], scroll: f32, view_w: f32, edge: f32) -> Hidden {
    let (mut cut_left, mut cut_right) = (false, false);
    let mut start = 0.0;
    for w in widths {
        cut_left |= start < scroll - EDGE_EPS;
        cut_right |= start + w > scroll + view_w + EDGE_EPS;
        start += w;
    }
    let mut hidden = Hidden::default();
    let mut start = 0.0;
    for (n, w) in widths.iter().enumerate() {
        let end = start + w;
        if cut_left && start < scroll + edge - EDGE_EPS {
            hidden.left = Some(n);
        }
        if cut_right && hidden.right.is_none() && end > scroll + view_w - edge + EDGE_EPS {
            hidden.right = Some(n);
        }
        start = end;
    }
    hidden
}

/// Minimum scroll, from `scroll`, that leaves tab `n` entirely in view, clear of the arrows (which
/// cover `edge` px of each edge where there are more tabs); limited to the content.
fn scroll_to_show(widths: &[f32], n: usize, scroll: f32, view_w: f32, edge: f32) -> f32 {
    let total: f32 = widths.iter().sum();
    let start: f32 = widths[..n].iter().sum();
    let end = start + widths[n];
    let left = if n > 0 { edge } else { 0.0 };
    let right = if n + 1 < widths.len() { edge } else { 0.0 };
    let target = if start - left < scroll {
        start - left
    } else if end + right > scroll + view_w {
        end + right - view_w
    } else {
        scroll
    };
    target.clamp(0.0, (total - view_w).max(0.0))
}

/// Paints the tabs `tabs` (indexes of `deck.tabs`) in `rect`, with horizontal scrolling when they do not
/// fit. `shown` is the visible tab of the window; `focused`, whether the window has the focus.
pub fn strip(deck: &mut Deck, ui: &Ui, rect: Rect, tabs: &[usize], shown: usize, focused: bool, scroll_id: Id) -> Option<StripClick> {
    let items: Vec<Item> = tabs
        .iter()
        .map(|&i| {
            let tab = &deck.tabs[i];
            let is_active = deck.active == Some(i) || (i == shown && focused);
            Item {
                uid: tab.uid,
                label: tab.label.clone(),
                color: tab.color,
                alive: tab.alive,
                busy: tab.busy,
                done: tab.done && !is_active,
                mic: dictation::button_visual(deck.mic_phase, MicTarget::Tab(tab.uid)),
                lead: Lead::Dot,
                tip: tab_tip(&tab.label, tab.model.as_deref()),
                dirty: false,
            }
        })
        .collect();
    let at = tabs.iter().position(|&i| i == shown).unwrap_or(0);
    let click = strip_items(ui, rect, &items, at, focused, scroll_id, StripKind { scope: "tab", tab_menu: true, colored: true })?;
    Some(match click {
        StripClick::Activate(n) => StripClick::Activate(tabs[n]),
        StripClick::Close(n) => StripClick::Close(tabs[n]),
        StripClick::Rename(n) => StripClick::Rename(tabs[n]),
        StripClick::ContextMenu(n, pos) => StripClick::ContextMenu(tabs[n], pos),
    })
}

/// The tab strip itself: paints `items`, with the one at position `shown` in view, and returns the click
/// (positions in `items`).
pub fn strip_items(ui: &Ui, rect: Rect, items: &[Item], shown: usize, focused: bool, scroll_id: Id, kind: StripKind) -> Option<StripClick> {
    let ctx = ui.ctx().clone();
    let painter = ui.painter_at(rect);
    let p = theme::pal();
    let widths: Vec<f32> = items
        .iter()
        .enumerate()
        .map(|(n, item)| {
            let font = if n == shown { theme::ui_semibold(TAB_FONT) } else { theme::ui(TAB_FONT) };
            let lw = text_w(&painter, &item.label, font).min(LABEL_MAX);
            let sw = match item.mic {
                ButtonVisual::Recording => REC_INDICATOR_W,
                ButtonVisual::Opening => 6.0 + text_w(&painter, t!("Opening…"), theme::ui(TAB_FONT)),
                ButtonVisual::Transcribing => 6.0 + text_w(&painter, t!("Transcribing…"), theme::ui(TAB_FONT)),
                _ => 0.0,
            };
            pill_width(lw, sw) + PILL_GAP
        })
        .collect();
    let total: f32 = widths.iter().sum();

    let overflow = total + STRIP_PAD > rect.width();
    // With the strip scrolling, its end is left for the "▾" button; the tabs only show up in `view`.
    let view = if overflow { Rect::from_min_max(rect.min, Pos2::new(rect.max.x - LIST_W, rect.max.y)) } else { rect };

    // Horizontal scrolling (no visible bar). The visible tab is brought into view when it changes (or
    // when the width changes); otherwise the user scrolls freely, with the wheel or the arrows.
    let mut scroll = ctx.data(|d| d.get_temp::<f32>(scroll_id)).unwrap_or(0.0);
    if overflow {
        let over = ui.interact(rect, scroll_id.with("area"), Sense::hover());
        if over.hovered() {
            let d = ui.input(|i| i.smooth_scroll_delta);
            scroll -= d.x + d.y;
        }
        let key = (shown, view.width().round() as i32);
        if ctx.data(|d| d.get_temp::<(usize, i32)>(scroll_id.with("seen"))) != Some(key) {
            ctx.data_mut(|d| d.insert_temp(scroll_id.with("seen"), key));
            if shown < items.len() {
                scroll = scroll_to_show(&widths, shown, scroll, view.width(), ARROW_ZONE_W);
            }
        }
        scroll = scroll.clamp(0.0, (total - view.width()).max(0.0));
    } else {
        scroll = 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(scroll_id, scroll));

    let mut x = rect.min.x + STRIP_PAD - scroll;
    let mut click = None;
    for (n, item) in items.iter().enumerate() {
        let w = widths[n];
        let r = Rect::from_center_size(Pos2::new(x + (w - PILL_GAP) / 2.0, rect.center().y), Vec2::new(w - PILL_GAP, PILL_H));
        x += w;
        if r.max.x < view.min.x || r.min.x > view.max.x {
            continue;
        }
        let vis = r.intersect(view);
        let is_shown = n == shown;
        let resp = ui.interact(vis, Id::new((kind.scope, item.uid)), Sense::click());
        let close_rect = Rect::from_center_size(Pos2::new(r.max.x - PILL_PAD - CLOSE_BOX / 2.0, r.center().y), Vec2::splat(CLOSE_BOX));
        let close_resp = ui.interact(close_rect.intersect(view), Id::new((kind.scope, "close", item.uid)), Sense::click());

        // The active pill takes the agent's color (a document has no color: `selected`); with the window
        // without focus, the active one stays neutral and only the dot keeps the color.
        let solid = is_shown && focused && kind.colored;
        let fill = if solid {
            Some(item.color)
        } else if is_shown {
            Some(p.selected)
        } else if resp.hovered() || close_resp.hovered() {
            Some(p.hover)
        } else {
            None
        };
        if let Some(fill) = fill {
            if solid {
                shadow(&painter, r, 5, 2, 10, p.shadow);
            }
            painter.rect_filled(r, 5.0, fill);
        }

        let color = if solid {
            p.on_accent
        } else if is_shown {
            p.text_strong
        } else if resp.hovered() {
            p.text
        } else {
            p.text_2
        };
        // Without a process, the tab stays at 40%.
        let lead_color = if solid {
            p.on_accent
        } else if item.alive {
            item.color
        } else {
            theme::with_opacity(item.color, 0.4)
        };
        let dot_pos = Pos2::new(r.min.x + PILL_PAD + DOT_BOX / 2.0, r.center().y);
        match item.lead {
            Lead::Glyph(g) => {
                let cell = Rect::from_center_size(dot_pos, Vec2::splat(14.0));
                text_in(&painter, cell, Align2::CENTER_CENTER, g, theme::mono_bold(11.0), if solid { p.on_accent } else { color });
            }
            Lead::Dot if item.busy => {
                ctx.request_repaint_after(std::time::Duration::from_millis(33));
                let t = ctx.input(|inp| inp.time);
                let wave = (t * 4.8).sin() as f32 * 0.5 + 0.5;
                painter.circle_filled(dot_pos, 3.5 + 3.5 * wave, theme::with_opacity(lead_color, 0.45 * (1.0 - wave)));
                painter.circle_filled(dot_pos, 3.5, lead_color);
            }
            // Finished unseen: the dot becomes a 1.5 px ring.
            Lead::Dot if item.done => {
                painter.circle_stroke(dot_pos, 3.25, Stroke::new(1.5, lead_color));
            }
            Lead::Dot => {
                painter.circle_filled(dot_pos, 3.5, lead_color);
            }
        }
        let label_x = dot_pos.x + DOT_BOX / 2.0 + PILL_GAP;
        let font = if is_shown { theme::ui_semibold(TAB_FONT) } else { theme::ui(TAB_FONT) };
        let g = galley_trunc(&painter, &item.label, font, color, LABEL_MAX);
        let lr = place(
            &painter,
            Rect::from_min_max(Pos2::new(label_x, r.min.y), Pos2::new(label_x + LABEL_MAX, r.max.y)),
            Align2::LEFT_CENTER,
            g,
            color,
        );
        if !item.alive {
            painter.hline(lr.min.x..=lr.max.x, lr.center().y + 0.5, Stroke::new(1.0, color));
        }
        let after_x = lr.max.x;
        // Dictation state indicator on the tab (the button lives in the panel's header).
        match item.mic {
            ButtonVisual::Recording => {
                let alpha = super::right_panel::blink_alpha(ctx.input(|inp| inp.time), 1.2);
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
                let rec_pos = Pos2::new(after_x + 10.0, r.center().y);
                painter.circle_filled(rec_pos, 5.5, theme::with_opacity(p.danger, alpha * 0.28));
                painter.circle_filled(rec_pos, 3.6, theme::with_opacity(p.danger, alpha));
            }
            ButtonVisual::Opening | ButtonVisual::Transcribing => {
                let st = if item.mic == ButtonVisual::Opening { t!("Opening…") } else { t!("Transcribing…") };
                let scolor = if solid { theme::with_opacity(p.on_accent, 0.75) } else { p.text_2 };
                let sg = galley_trunc(&painter, st, theme::ui(TAB_FONT), scolor, (close_rect.min.x - after_x - 8.0).max(10.0));
                let x0 = after_x + 6.0;
                place(
                    &painter,
                    Rect::from_min_max(Pos2::new(x0, r.min.y), Pos2::new(close_rect.min.x - 2.0, r.max.y)),
                    Align2::LEFT_CENTER,
                    sg,
                    scolor,
                );
            }
            _ => {}
        }

        let close_hot = close_resp.hovered();
        if close_hot {
            painter.rect_filled(close_rect, 4.0, if solid { theme::with_alpha(p.on_accent, 0.15) } else { p.separator });
        }
        if close_hot || resp.hovered() {
            ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        // The "✕" is drawn (Inter does not have it). Dark over the agent's color.
        let x_color = match (solid, close_hot) {
            (true, true) => p.on_accent,
            (true, false) => theme::with_opacity(p.on_accent, 0.6),
            (false, true) => p.text,
            (false, false) => p.text_3,
        };
        let c = close_rect.center();
        let h = 3.0;
        if item.dirty && !close_hot {
            painter.circle_filled(c, 3.5, if solid { p.on_accent } else { p.warning });
        } else {
            painter.line_segment([c + Vec2::new(-h, -h), c + Vec2::new(h, h)], Stroke::new(1.3, x_color));
            painter.line_segment([c + Vec2::new(-h, h), c + Vec2::new(h, -h)], Stroke::new(1.3, x_color));
        }

        if tip(close_resp, if item.dirty { t!("Close (has unsaved changes)") } else { t!("Close") }).clicked() {
            click = Some(StripClick::Close(n));
        } else if kind.tab_menu && resp.double_clicked() {
            click = Some(StripClick::Rename(n));
        } else if kind.tab_menu && resp.secondary_clicked() {
            let pos = ui.input(|inp| inp.pointer.latest_pos()).unwrap_or_else(|| r.center());
            click = Some(StripClick::ContextMenu(n, pos));
        } else if tip(resp, item.tip.clone()).clicked() {
            click = Some(StripClick::Activate(n));
        }
    }

    if overflow {
        let hidden = hidden_tabs(&widths, scroll, view.width(), ARROW_ZONE_W);
        let mut new_scroll = None;
        if let Some(n) = hidden.left {
            let zone = Rect::from_min_size(rect.min, Vec2::new(ARROW_ZONE_W, rect.height()));
            if edge_arrow(ui, &painter, zone, Dir::Left, &items[n].label, scroll_id.with("arrow-left")) {
                new_scroll = Some(scroll_to_show(&widths, n, scroll, view.width(), ARROW_ZONE_W));
            }
        }
        if let Some(n) = hidden.right {
            let zone = Rect::from_min_max(Pos2::new(view.max.x - ARROW_ZONE_W, rect.min.y), Pos2::new(view.max.x, rect.max.y));
            if edge_arrow(ui, &painter, zone, Dir::Right, &items[n].label, scroll_id.with("arrow-right")) {
                new_scroll = Some(scroll_to_show(&widths, n, scroll, view.width(), ARROW_ZONE_W));
            }
        }
        if let Some(s) = new_scroll {
            ctx.data_mut(|d| d.insert_temp(scroll_id, s));
            ctx.request_repaint();
        }
        let list = Rect::from_min_max(Pos2::new(view.max.x, rect.min.y), rect.max);
        if let Some(n) = tab_list(ui, &painter, list, items, shown, scroll_id.with("list"), kind.scope) {
            click = Some(StripClick::Activate(n));
        }
    } else {
        ctx.data_mut(|d| d.remove::<bool>(scroll_id.with("list")));
    }
    click
}

#[derive(Clone, Copy, PartialEq)]
enum Dir {
    Left,
    Right,
    Down,
}

/// A thin arrow (chevron) centered on `c`.
fn chevron(painter: &Painter, c: Pos2, dir: Dir, color: Color32) {
    let (a, b, d) = match dir {
        Dir::Left => (Vec2::new(1.5, -4.0), Vec2::new(-1.5, 0.0), Vec2::new(1.5, 4.0)),
        Dir::Right => (Vec2::new(-1.5, -4.0), Vec2::new(1.5, 0.0), Vec2::new(-1.5, 4.0)),
        Dir::Down => (Vec2::new(-4.0, -1.5), Vec2::new(0.0, 1.5), Vec2::new(4.0, -1.5)),
    };
    let s = Stroke::new(1.5, color);
    painter.line_segment([c + a, c + b], s);
    painter.line_segment([c + b, c + d], s);
}

/// Horizontal gradient from the strip color (`strip_solid`) (at `solid_x`) to transparent (at `far_x`), between `top` and `bottom`.
fn fade(painter: &Painter, top: f32, bottom: f32, solid_x: f32, far_x: f32) {
    let (opaque, clear) = (theme::strip_solid(), theme::with_alpha(theme::strip_solid(), 0.0));
    let mut mesh = egui::Mesh::default();
    mesh.colored_vertex(Pos2::new(solid_x, top), opaque);
    mesh.colored_vertex(Pos2::new(far_x, top), clear);
    mesh.colored_vertex(Pos2::new(far_x, bottom), clear);
    mesh.colored_vertex(Pos2::new(solid_x, bottom), opaque);
    mesh.add_triangle(0, 1, 2);
    mesh.add_triangle(0, 2, 3);
    painter.add(mesh);
}

/// Arrow of one edge of the strip (there are hidden tabs on that side): solid background, gradient and button.
/// Returns whether it was clicked; `target` is the name of the nearest hidden tab, which the hint mentions.
fn edge_arrow(ui: &Ui, painter: &Painter, zone: Rect, dir: Dir, target: &str, id: Id) -> bool {
    painter.rect_filled(zone, 0.0, theme::strip_solid());
    let (solid_x, far_x) = if dir == Dir::Left { (zone.max.x, zone.max.x + FADE_W) } else { (zone.min.x, zone.min.x - FADE_W) };
    fade(painter, zone.min.y, zone.max.y, solid_x, far_x);
    let btn = Rect::from_center_size(zone.center(), Vec2::splat(ARROW_BTN));
    let resp = clickable(ui, btn, id);
    if resp.hovered() {
        painter.rect_filled(btn, 4.0, theme::pal().hover);
    }
    chevron(painter, btn.center(), dir, if resp.hovered() { theme::pal().text } else { theme::pal().text_2 });
    tip(resp, t!("Show “{target}”", target)).clicked()
}

/// "▾" button at the end of the strip and the menu with all the tabs of the window. Returns the chosen tab.
fn tab_list(ui: &Ui, painter: &Painter, zone: Rect, items: &[Item], shown: usize, id: Id, scope: &'static str) -> Option<usize> {
    let ctx = ui.ctx();
    painter.rect_filled(zone, 0.0, theme::strip_solid());
    let btn = Rect::from_center_size(zone.center(), Vec2::splat(ARROW_BTN));
    let resp = clickable(ui, btn, id.with("btn"));
    let mut open = ctx.data(|d| d.get_temp::<bool>(id)).unwrap_or(false);
    let hot = resp.hovered() || open;
    if hot {
        painter.rect_filled(btn, 4.0, theme::pal().hover);
    }
    chevron(painter, btn.center(), Dir::Down, if hot { theme::pal().text } else { theme::pal().text_2 });
    if tip(resp, if scope == "tab" { t!("All tabs") } else { t!("All documents") }).clicked() {
        open = !open;
    }

    let mut chosen = None;
    if open {
        let rows = items.len().min(LIST_MAX_ROWS);
        let (w, h) = (LIST_MENU_W, LIST_ITEM_H * rows as f32 + 8.0);
        let pos = keep_on_screen(ctx, Pos2::new(btn.max.x - w, btn.max.y + 4.0), Vec2::new(w, h));
        let menu = Rect::from_min_size(pos, Vec2::new(w, h));
        Area::new(id.with("menu")).order(Order::Foreground).fixed_pos(pos).show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::new(w, h), Sense::hover());
            shadow(ui.painter(), rect, 8, 8, 24, theme::with_alpha(theme::pal().shadow, 0.55));
            frame(ui.painter(), rect, 6.0, theme::pal().popup, theme::pal().glass_border);
            let inner = rect.shrink(4.0);
            ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
                egui::ScrollArea::vertical().max_height(inner.height()).auto_shrink([false, false]).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    for (n, item) in items.iter().enumerate() {
                        let (r, _) = ui.allocate_exact_size(Vec2::new(inner.width(), LIST_ITEM_H), Sense::hover());
                        let resp = clickable(ui, r, id.with(("item", item.uid)));
                        let p = ui.painter();
                        if resp.hovered() || n == shown {
                            p.rect_filled(r, 4.0, theme::pal().separator);
                        }
                        let lead = Pos2::new(r.min.x + 14.0, r.center().y);
                        match item.lead {
                            Lead::Dot => dot(p, lead, item.color),
                            Lead::Glyph(g) => {
                                let cell = Rect::from_center_size(lead, Vec2::splat(14.0));
                                text_in(p, cell, Align2::CENTER_CENTER, g, theme::mono_bold(11.0), item.color);
                            }
                        }
                        let color = if item.alive { theme::pal().text } else { theme::pal().text_2 };
                        let text_x = r.min.x + 28.0;
                        let g = galley_trunc(p, &item.label, theme::ui(12.5), color, r.max.x - text_x - 8.0);
                        let area = Rect::from_min_max(Pos2::new(text_x, r.min.y), Pos2::new(r.max.x - 8.0, r.max.y));
                        let lr = place(p, area, Align2::LEFT_CENTER, g, color);
                        if !item.alive {
                            p.hline(lr.min.x..=lr.max.x, lr.center().y + 0.5, Stroke::new(1.0, color));
                        }
                        if resp.clicked() {
                            chosen = Some(n);
                        }
                    }
                });
            });
        });
        if chosen.is_some() || ctx.input(|i| i.key_pressed(egui::Key::Escape)) || closes_on_outside(ctx, menu, btn) {
            open = false;
        }
    }
    ctx.data_mut(|d| d.insert_temp(id, open));
    chosen
}

/// Tab hint: the name, the model (whole) and how to rename it.
fn tab_tip(label: &str, model: Option<&str>) -> String {
    match model {
        Some(model) => t!("{label}\nModel: {model}\nDouble click or right click to rename", label, model = model),
        None => t!("{label}\nDouble click or right click to rename", label),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::i18n::{self, Lang};

    /// Five 100 px tabs.
    const W: [f32; 5] = [100.0; 5];

    #[test]
    fn nothing_is_hidden_when_the_tabs_fit() {
        assert_eq!(hidden_tabs(&W, 0.0, 500.0, 24.0), Hidden::default());
        assert_eq!(hidden_tabs(&[], 0.0, 250.0, 24.0), Hidden::default());
    }

    #[test]
    fn the_cut_tab_counts_as_hidden_on_the_side_it_is_cut() {
        // At the start: there is only something to see on the right; the third is cut (200..300 in 0..250).
        assert_eq!(hidden_tabs(&W, 0.0, 250.0, 0.0), Hidden { left: None, right: Some(2) });
        // In the middle (scrolled 120): the second is cut on the left; the fourth is cut on the right (300..400 in 120..370).
        assert_eq!(hidden_tabs(&W, 120.0, 250.0, 0.0), Hidden { left: Some(1), right: Some(3) });
        // At the end (scrolled 250): there is only something to see on the left; the nearest is the third, cut.
        assert_eq!(hidden_tabs(&W, 250.0, 250.0, 0.0), Hidden { left: Some(2), right: None });
    }

    #[test]
    fn a_fraction_of_a_pixel_does_not_make_a_tab_hidden() {
        assert_eq!(hidden_tabs(&W, 0.3, 300.0, 0.0), Hidden { left: None, right: Some(3) });
        assert_eq!(hidden_tabs(&W, 100.0, 300.2, 0.0), Hidden { left: Some(0), right: Some(4) });
        assert_eq!(hidden_tabs(&W, 200.2, 300.0, 0.0), Hidden { left: Some(1), right: None });
    }

    #[test]
    fn a_tab_under_the_arrow_counts_as_hidden() {
        // Scrolled 100: the second starts at the edge (100..200), but the 24 px arrow covers it; the first is the hidden one.
        assert_eq!(hidden_tabs(&W, 100.0, 250.0, 24.0).left, Some(1));
        // With no tab cut on that side there is no arrow and nothing is covered: the fifth ends at the right edge.
        assert_eq!(hidden_tabs(&W, 250.0, 250.0, 24.0).right, None);
    }

    #[test]
    fn the_arrow_scrolls_to_the_nearest_hidden_tab() {
        // To the left: the hidden tab touches the left edge.
        assert_eq!(scroll_to_show(&W, 1, 120.0, 250.0, 0.0), 100.0);
        // To the right: the hidden tab touches the right edge.
        assert_eq!(scroll_to_show(&W, 2, 0.0, 250.0, 0.0), 50.0);
        // Already in view: it does not move.
        assert_eq!(scroll_to_show(&W, 1, 50.0, 250.0, 0.0), 50.0);
        // It never goes past the end of the content nor the start.
        assert_eq!(scroll_to_show(&W, 4, 0.0, 250.0, 0.0), 250.0);
        assert_eq!(scroll_to_show(&W, 0, 250.0, 250.0, 0.0), 0.0);
    }

    #[test]
    fn the_shown_tab_stays_clear_of_the_arrows() {
        // With tabs on both sides, the arrow width is left along the edge; at the ends, it is not.
        assert_eq!(scroll_to_show(&W, 2, 0.0, 250.0, 24.0), 74.0);
        assert_eq!(scroll_to_show(&W, 2, 400.0, 250.0, 24.0), 176.0);
        assert_eq!(scroll_to_show(&W, 0, 250.0, 250.0, 24.0), 0.0);
        assert_eq!(scroll_to_show(&W, 4, 0.0, 250.0, 24.0), 250.0);
    }

    #[test]
    fn clicking_the_arrows_reaches_every_tab() {
        for edge in [0.0, 24.0] {
            let (mut scroll, view) = (0.0, 250.0);
            let mut clicks = 0;
            while let Some(n) = hidden_tabs(&W, scroll, view, edge).right {
                scroll = scroll_to_show(&W, n, scroll, view, edge);
                clicks += 1;
                assert!(clicks < 10);
            }
            assert_eq!(scroll, 250.0);
            while let Some(n) = hidden_tabs(&W, scroll, view, edge).left {
                scroll = scroll_to_show(&W, n, scroll, view, edge);
                clicks += 1;
                assert!(clicks < 20);
            }
            assert_eq!(scroll, 0.0);
        }
    }

    #[test]
    fn a_pill_is_as_wide_as_its_parts() {
        // padding 8 + dot 7 + 6 + name + 6 + ✕ 16 + padding 8.
        assert_eq!(pill_width(50.0, 0.0), 8.0 + 7.0 + 3.0 + 50.0 + 3.0 + 16.0 + 8.0);
        // The dictation state ("Transcribing…") widens the pill.
        assert_eq!(pill_width(50.0, 20.0) - pill_width(50.0, 0.0), 20.0);
    }

    #[test]
    fn the_tip_names_the_full_model() {
        let tip = || tab_tip("Claude · Dev", None);
        assert_eq!(i18n::english(tip), "Claude · Dev\nDouble click or right click to rename");
        assert_eq!(i18n::with_lang(Lang::PtBr, tip), "Claude · Dev\nDuplo clique ou botão direito para renomear");
        let with_model = || tab_tip("OpenCode", Some("opencode/glm-5.3-flash-free"));
        assert_eq!(i18n::english(with_model), "OpenCode\nModel: opencode/glm-5.3-flash-free\nDouble click or right click to rename");
        assert_eq!(
            i18n::with_lang(Lang::PtBr, with_model),
            "OpenCode\nModelo: opencode/glm-5.3-flash-free\nDuplo clique ou botão direito para renomear"
        );
    }
}

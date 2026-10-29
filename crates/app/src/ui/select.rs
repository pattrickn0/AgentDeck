//! Text selection on the hand-painted screens (the diff and the large file, in read-only mode). egui only selects
//! what is a `Label` or a `TextEdit`, and these screens only paint the lines that show up (`show_viewport`/`show_rows`): here the selection is
//! a pair of positions (line, byte inside the line's text) kept between frames, and whoever paints asks, line by
//! line, what part of it is selected ([`Selected::on_line`]).
//!
//! Dragging selects (scrolling the screen when the pointer goes past the edge), a click clears it, Ctrl+A selects all and Ctrl+C
//! copies. The small file does not go through here: egui's `TextEdit` already does all of that.

use std::ops::Range;

use egui::{Event, Id, Key, Modifiers, Pos2, Rect, Sense, Ui, Vec2};

/// A position in the text: the line and the byte inside its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default)]
pub struct Pos {
    pub line: usize,
    pub col: usize,
}

/// The selection in progress: where the drag started and where it is now (the head may come before the anchor).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Drag {
    anchor: Pos,
    head: Pos,
}

/// A non-empty selection, with the start before the end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Selected {
    pub start: Pos,
    pub end: Pos,
}

impl Selected {
    /// What part of line `line`, of `len` bytes, is selected.
    pub fn on_line(&self, line: usize, len: usize) -> Option<Range<usize>> {
        if line < self.start.line || line > self.end.line {
            return None;
        }
        let a = if line == self.start.line { self.start.col.min(len) } else { 0 };
        let b = if line == self.end.line { self.end.col.min(len) } else { len };
        Some(a..b.max(a))
    }
}

fn ordered(d: Drag) -> Option<Selected> {
    (d.anchor != d.head).then(|| Selected { start: d.anchor.min(d.head), end: d.anchor.max(d.head) })
}

/// How many pixels from the edge the drag starts scrolling the screen, and how much it scrolls per frame at most.
const EDGE: f32 = 24.0;
const SCROLL_STEP: f32 = 22.0;

/// Handles the mouse and the keyboard of the `rect` area (the visible part, in screen coordinates) and returns the frame's selection.
/// `hit` turns a screen point into a text position (any point, even outside the area: the drag goes past the edge);
/// `end` is the end of the text (for Ctrl+A); `text` returns the text of a selection (for Ctrl+C).
/// It must come before any button drawn on top of the area, so that the button gets the click.
pub fn drive(
    ui: &mut Ui,
    id: Id,
    rect: Rect,
    end: Pos,
    hit: impl Fn(Pos2) -> Pos,
    text: impl FnOnce(Selected) -> String,
) -> Option<Selected> {
    let ctx = ui.ctx().clone();
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let mut d: Drag = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
    if resp.drag_started() {
        if let Some(origin) = ctx.input(|i| i.pointer.press_origin()) {
            d.anchor = hit(origin);
            d.head = d.anchor;
        }
        resp.request_focus();
    } else if resp.clicked() {
        d = Drag::default();
        resp.request_focus();
    }
    if resp.dragged() {
        if let Some(p) = ctx.input(|i| i.pointer.interact_pos()) {
            d.head = hit(p);
            let dy = if p.y < rect.min.y + EDGE {
                (p.y - (rect.min.y + EDGE)).max(-SCROLL_STEP * 3.0) / 3.0
            } else if p.y > rect.max.y - EDGE {
                (p.y - (rect.max.y - EDGE)).min(SCROLL_STEP * 3.0) / 3.0
            } else {
                0.0
            };
            if dy != 0.0 {
                // `scroll_with_delta` moves in the direction of the content: dragging past the bottom edge scrolls down.
                ui.scroll_with_delta(Vec2::new(0.0, -dy));
                ctx.request_repaint();
            }
        }
    }
    if resp.has_focus() {
        if ctx.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::A)) {
            d = Drag { anchor: Pos::default(), head: end };
        }
        let copy = ctx.input(|i| i.events.iter().any(|e| matches!(e, Event::Copy)));
        if copy {
            if let Some(sel) = ordered(d) {
                ctx.copy_text(text(sel));
            }
        }
    }
    ctx.data_mut(|w| w.insert_temp(id, d));
    ordered(d)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(line: usize, col: usize) -> Pos {
        Pos { line, col }
    }

    #[test]
    fn the_selection_is_ordered_whatever_the_drag_direction() {
        let forward = ordered(Drag { anchor: p(1, 2), head: p(3, 0) }).expect("selection");
        let backward = ordered(Drag { anchor: p(3, 0), head: p(1, 2) }).expect("selection");
        assert_eq!(forward, backward);
        assert_eq!((forward.start, forward.end), (p(1, 2), p(3, 0)));
        // A click without dragging selects nothing.
        assert_eq!(ordered(Drag { anchor: p(2, 4), head: p(2, 4) }), None);
    }

    #[test]
    fn each_line_reports_its_selected_part() {
        let sel = Selected { start: p(1, 2), end: p(3, 4) };
        assert_eq!(sel.on_line(0, 10), None);
        assert_eq!(sel.on_line(1, 10), Some(2..10));
        assert_eq!(sel.on_line(2, 10), Some(0..10));
        assert_eq!(sel.on_line(2, 0), Some(0..0));
        assert_eq!(sel.on_line(3, 10), Some(0..4));
        assert_eq!(sel.on_line(4, 10), None);
        // The end past the line length (the line got shorter) is cut.
        assert_eq!(sel.on_line(3, 2), Some(0..2));
        // Start and end on the same line.
        let one = Selected { start: p(5, 1), end: p(5, 3) };
        assert_eq!(one.on_line(5, 10), Some(1..3));
        assert_eq!(one.on_line(5, 2), Some(1..2));
        assert_eq!(one.on_line(5, 0), Some(0..0));
    }
}

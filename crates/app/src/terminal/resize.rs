//! Grid resizing as expected by ConPTY (Windows).
//!
//! `alacritty_terminal` keeps the cursor on the same screen line and, to do that, **pulls lines from history to the top**
//! when the screen grows in rows or when the reflow of a larger width joins lines. ConPTY does not do this: the top of
//! its window stays at the same content, leftover lines remain blank at the bottom, and right after the resize it repaints
//! the entire window starting from `ESC[H` (measured with PowerShell and Claude Code). The repaint overwrites the lines
//! that the grid pulled, causing them to disappear: lines numbered 144 to 171 disappeared from the conversation (and 28 lines remained blank
//! at the bottom) in a window that went from 58 to 30 and back to 58 rows; the same happened when widening wrapped text.
//! Therefore, after each resize, the content that was at the top of the screen is moved back to the top, and what was pulled moves back to
//! history. This file does not depend on the rest of the app: `tools/pty-probe` includes it via `#[path]` for measurements.

use alacritty_terminal::event::EventListener;
use alacritty_terminal::grid::Dimensions;
use alacritty_terminal::index::{Column, Line};
use alacritty_terminal::term::{Term, TermMode};

/// Zero-width character (private use area) marking the first cell on screen during resizing.
const MARK: char = '\u{E000}';

/// `term.resize(size)`; with `keep_top` the content that was on the first screen line remains there (see module doc).
pub fn resize<T: EventListener>(term: &mut Term<T>, size: impl Dimensions, keep_top: bool) {
    if !keep_top || term.grid().history_size() == 0 || term.mode().contains(TermMode::ALT_SCREEN) {
        term.resize(size);
        return;
    }
    let saved = term.grid()[Line(0)][Column(0)].clone();
    term.grid_mut()[Line(0)][Column(0)].push_zerowidth(MARK);
    term.resize(size);

    let grid = term.grid_mut();
    let marked = |l: i32, c: usize| grid[Line(l)][Column(c)].zerowidth().is_some_and(|z| z.contains(&MARK));
    let rows = grid.screen_lines() as i32;
    // The mark is either on screen (pulled down) or just above it (pushed to history).
    let top = -(grid.history_size() as i32);
    let Some((line, col)) = (top..rows).rev().find_map(|l| (0..grid.columns()).find(|&c| marked(l, c)).map(|c| (l, c))) else { return };

    // Remove the mark: the cell reverts to what it was (only `extra`, private, comes from the saved clone).
    let cell = &mut grid[Line(line)][Column(col)];
    let mut restored = saved;
    (restored.c, restored.fg, restored.bg, restored.flags) = (cell.c, cell.fg, cell.bg, cell.flags);
    *cell = restored;

    // Mark in the middle of a line (the continuation of a wrapped line was joined to the previous one during reflow): ConPTY starts the
    // window on the next line, so this entire line goes to history.
    let line = if col > 0 { line + 1 } else { line };
    if line > 0 {
        grid.scroll_up(&(Line(0)..Line(rows)), line as usize);
        grid.cursor.point.line = Line((grid.cursor.point.line.0 - line).max(0));
        grid.saved_cursor.point.line = Line((grid.saved_cursor.point.line.0 - line).max(0));
    }
}

---
title: Claude Code duplicates conversation on resize (no ESC[3J on Windows)
summary: Each new width creates a copy in the history; therefore, the PTY only resizes when dimensions settle.
touches: [crates/app/src/deck/**, crates/app/src/terminal/**]
---
When width changes, Claude Code only clears the screen and reprints the conversation. Outside Windows, it sends `ESC[3J`
(clears scrollback history); on Windows, it does not, so the previous copy remains in history at the old width. This was measured
with a probe (portable-pty + alacritty_terminal + real `claude.exe`): one extra copy per width change, same with
Windows Terminal's `OpenConsole.exe`.

**Why:** clearing our scrollback before resizing does not work: upon widening, Claude Code only redraws
the visible screen, and the conversation would disappear.
**How to apply:** the PTY only resizes once the mouse is released and the size remains settled for 150 ms (`RESIZE_DEBOUNCE`,
`resize_due` in `deck.rs`): a drag becomes a single copy. Switching tabs in Highlight changes the width of panes
without dragging: there the delay is 600 ms (`HIGHLIGHT_RESIZE_DEBOUNCE`, marked in `Deck::activate` via
`highlight_switch_until`). Reverting to the size the PTY already has within the debounce cancels the pending resize (`Deck::fit_tab`).
After applying a resize, the next one for the same tab waits for its output to remain quiet for 300 ms
(`RESIZE_SETTLE`), capped at 3 s (`RESIZE_SETTLE_MAX`) for an agent that writes continuously: without this, rapid tab
switches stacked redraws and Claude's tab spent ~15 s rendering in 38 columns (t51, 2026-10-24).
Do not shorten these delays or resize the PTY every frame. The underlying issue remains open in `ideias.md`
("Known limitations").

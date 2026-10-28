---
title: On Windows, the grid must not pull history when expanding: ConPTY repaints over it and lines vanish (t37)
summary: When gaining lines (or joining lines during reflow) alacritty pulls history into the screen; ConPTY repaints from ESC[H and deletes what was pulled. terminal/resize.rs undoes this.
touches: [crates/app/src/terminal/**, crates/app/src/deck/**, tools/pty-probe/**]
---
Defect reported by user (23/10): "sometimes, in terminals" the screen ends up with blank lines and truncated text, across any CLI. Reproduced **without
Claude**, using `powershell` + `tools/pty-probe`: 200 numbered lines, window 58→30→58 lines, and lines 144 to 171 vanished from history
(28 = lines gained) with 28 blank lines at the bottom; in a 400-line wrapped session, 120→60→120 columns lost 29 lines; with 24
random size changes, 155 lines lost. This is cause 2/3 from the request: a mismatch between ConPTY and `alacritty_terminal`.

**Cause:** when gaining lines, or when joining wrapped lines (reflow at a wider width), `alacritty_terminal` keeps the cursor on the same
screen line and **pulls history lines into the top**. Conhost does not: its window top remains at the same content, newly added lines stay
blank at the bottom, and right after resizing it repaints the entire window starting from `ESC[H` (seen in bytes: `ESC[8;lines;columnst`,
`ESC[H`, the lines, `ESC[K`). The repaint overwrites the pulled lines, which disappear from history and the screen. Shrinking was already identical between both.
CLIs using the alternate screen buffer (OpenCode) have no scrollback and repaint themselves: they are unaffected.

**Fix:** `terminal/resize.rs` (used by `Emulator::resize`, on Windows only). It marks the first cell on screen with a zero-width character,
allows `alacritty_terminal` to resize, and if the mark moved down (lines pulled), pushes those lines back to history (`scroll_up`, moving cursor
along); if the mark landed mid-line (continuation was joined to the previous line), the entire line goes to history. The result matches
ConPTY: whatever was at the top stays at the top and extra lines remain blank at the bottom. Measured with `powershell`: 0 lines lost across
24 random resizes (was 155). What remains on width changes is a repetition of 1 to 3 lines in history (the same duplicate-copy issue as in
`claude-code-sem-esc3j-no-windows.md`).
**Tests:** `record.rs` replays two real conhost recordings (`testdata/conhost-powershell-*.bin` + `.events.txt`), with the fix enabled
(nothing vanishes) and disabled (lines vanish).

**How to investigate the next occurrence:** the probe `tools/pty-probe` (README) runs any CLI, applies size changes (`RESIZE`), and checks
numbering (`L00001 ...`) and gaps; `PROBE_PULL=1` disables the fix for comparison; `PROBE_WATCH=L00372,...` shows where each line is. In the app,
the tab context menu item "Record terminal (diagnostics)" (right-click; or `AGENTDECK_DEBUG_PTY=1`) writes to `dados/pty/` the raw bytes (`.bin`),
size changes (`.events.txt`), and the grid 1.5 s after last output (`.grid.txt`); "Save grid snapshot now" immediately captures the grid
and opens the directory. A grid with text and a blank pane = rendering bug; a blank grid, and `.bin` replayed in a fresh `Term` also blank = CLI/ConPTY.
The `.bin` contains everything displayed by the CLI: do not share without reviewing.

**Why:** ConPTY does not communicate with the emulator about what each performed during a resize; any discrepancy in model results in misplaced content
without producing an error. **How to apply:** changes to resize/reflow touch here; measure with the probe (shell and CLI) before and after. The case of
"text at bottom and gap at top" from the user screenshot has not yet been reproduced; if the fix is insufficient, capture their session.

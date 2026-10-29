---
title: In Highlight, the large pane is derived from the active tab (stateless) and organization has two fields in Config
summary: Nothing stores "who is in highlight"; switching tabs swaps panes, and the PTY only resizes when dimensions settle. `organization` governs, `splitEnabled` is only legacy fallback.
touches: [crates/app/src/split.rs, crates/app/src/deck/**, crates/app/src/ui/panes.rs, crates/app/src/ui/sidebar.rs, crates/core/src/config.rs]
---
The **Highlight** mode (`Organization::Highlight`) has no state of its own other than ratio (`Config::highlight_ratio`, global,
not per-project): `Deck::plan_panes` places the **active tab's** group first (`split::highlight_order`), and `assign` +
`highlight_layout` handle the rest. Other groups remain in `group_order`, which does not change. Therefore, clicking a
small pane, activating a tab via the sidebar or shortcut, and the "Bring to highlight" button are all the same `Deck::activate`;
and the handle for swapping panes does not exist in Highlight (small pane ordering follows `group_order`; highlight is not a fixed slot).

**Why:** storing "the highlight" separately from the active tab introduces two conflicting states (activating a tab via shortcut,
task, or `ask-agent` would need to update both). Changing the highlight resizes panes once; the PTY follows the standard
rule (`fit_tab` + settled size, see `claude-code-without-esc3j-on-windows.md`), using 600 ms on a tab switch in Highlight instead of
the 150 ms drag debounce, and the next tab resize only executes after reprint completion
(300 ms quiet output, capped at 3 s). The test `the_highlight_follows_the_active_tab_and_the_pty_resizes_once_per_switch`
(`tasks_flow_tests.rs`) verifies that steady frames do not request resizes. Do not animate pane sizes: each animation
frame would produce a new size for the PTY.

**Organization in Config:** `organization` (`Option<Organization>`: `tabs`, `split`, `highlight`) is the active field; legacy
`splitEnabled` continues to be read (files without `organization` → `Config::organization()` infers from it) and written alongside
(`set_organization`) so older versions understand the file. Never read `split_enabled` directly: use `organization()`.

**Highlighting (Split and Highlight):** the "62%" dimming of other panes is a 38% `bg` color tint over the pane (egui lacks
group opacity), animated via `animate_bool_with_time`; hovering removes it. The 3 px halo is an outer stroke
(`StrokeKind::Outside`) painted using the entire `ui.painter()`, not the pane's `painter_at(rect)`, which would clip it.

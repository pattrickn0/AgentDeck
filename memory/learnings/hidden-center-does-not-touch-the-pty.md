---
title: When terminals are hidden (Documents mode), nothing touches them
summary: No fit_tab, no keyboard or IME, and the end of a turn counts as unseen; switching views is only detected one frame later.
touches: [crates/app/src/deck/**, crates/app/src/docs.rs, crates/app/src/ui/mod.rs, crates/app/src/ui/panes.rs, crates/app/src/ui/docs.rs]
---
With the center pane showing Documents, `ui::draw` does not call `panes::draw`; terminals continue running, but nothing draws, resizes,
or feeds them. Three rules follow: (1) **no `fit_tab`** while the center pane shows documents, otherwise the PTY resizes while
hidden and Claude Code duplicates the conversation on Windows (see `claude-code-without-esc3j-on-windows.md`); upon returning, the usual `fit_tab`
adjusts the size, with a 150 ms debounce; (2) the terminal receives **no keyboard or IME**: `handle_key` and `handle_ime` check
`docs.view` directly; (3) the end of a turn marks `Tab.done` even on the active tab (no one is seeing it), and activating a tab without switching back to
agents does not clear it: `Deck::agents_badge` feeds the badge.

**Why:** `Deck::center_changed` (which turns off `term_focus`, clears IME, and marks the active tab as seen upon return) only runs in
`before_frame` following the switch, and keystrokes arrive between two frames: the terminal keyboard cannot depend on `term_focus`
alone. The state of `Docs::view` is what governs; whoever opens a tab or clicks an agent must call `Deck::show_agents`.
**How to apply:** a new mode that hides terminals repeats these three rules; the test
`reading_documents_keeps_the_terminals_running_unresized_and_marks_finished_turns_as_unseen` (`tasks_flow_tests.rs`) covers the PTY, the badge, and returning.

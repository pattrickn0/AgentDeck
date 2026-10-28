---
title: Hidden or minimized windows do not paint, and minimized windows have 0×0 dimensions
summary: Deck background work without frames runs through Deck::background; no painting or fit_tab occurs with a hidden window.
touches: [crates/app/src/app.rs, crates/app/src/deck/**]
---
When the window is minimized (on Windows) or hidden, winit delivers no `RedrawRequested` (the OS sends no `WM_PAINT`), and a
minimized window reports an `inner_size` of 0×0. Two consequences that can cause bugs: (1) everything previously running inside frame rendering
(`Deck::before_frame` → `tick`: `ask-agent` delivery, "typed later" answers, scheduled Enter, end of turn,
timers) would stall; (2) if a frame still arrives, drawing into 0×0 causes egui to panic in debug builds
(`Negative child size` in `placer.rs`, reproduced when minimizing a debug build of base) and, in `panes::draw`, `fit_tab`
would request a zero-area terminal, breaking the CLI (see `claude-code-sem-esc3j-no-windows.md`).

**Why:** non-rendering background processing was split into `Deck::work`; `before_frame` calls it with `Context`, and
`Deck::background` without it. `App::redraw` with `is_hidden` (minimized, not visible, or zero area) only runs `background` and returns;
`Wake` and deadlines turn into `Background::wake`/`next` (`app.rs`), using `ControlFlow::WaitUntil` until the next deadline and `Wait` when no deadline exists.
**How to apply:** anything `tick` requires from egui (`Context`: clipboard access, reading mouse state) sits behind
`Option<&Context>` and is deferred rather than executed blindly. Any new mechanism hiding the window (like the system tray) must call `Background::wake`
upon hiding because winit does not fire events on `set_visible(false)`. The frameless background test is
`a_hidden_window_still_types_the_queued_messages_and_the_late_answers_and_never_resizes_the_terminals` (`tasks_flow_tests.rs`).

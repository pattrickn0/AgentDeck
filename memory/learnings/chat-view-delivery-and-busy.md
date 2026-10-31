---
title: Chat tabs take messages from the event stream, not from the terminal heuristics
summary: A chat tab is "hooked" (busy/idle from events), its Tab.session PTY is unused, deliveries become user messages, and the terminal-only code paths (resize, keys, IME) must skip it.
touches: [crates/app/src/deck/**, crates/app/src/ui/panes.rs, crates/app/src/ui/chat.rs, crates/core/src/delivery.rs]
---
A tab in the chat view has `Tab.chat = Some(..)`; its `Tab.session` (the PTY emulator) still exists but nothing is spawned in it.
Whatever touches the terminal must check `tab.chat`: `Deck::tab_state` (delivery gate), `start`, `set_term_focus`/`handle_key`/IME (a chat
tab leaves `term_focus` off so egui's text field gets the keyboard), the exit handler (no "press Enter to restart" in the PTY), the
`idle_deadline` release (cleared: the stream says when a turn ends) and `fit_tab` (not called).

The turn counter lives in the transcript (`push_user` +1, `result` −1): a message sent while busy is queued by the CLI and answered
later, so a boolean would free the tab too early. `ChatProcess` exit resets it (`process_ended`).

Switching between the chat and the terminal view kills the process and starts the other mode on the same session (a live
process cannot change mode). During a turn that would cut the turn off, so `Deck::set_tab_view` keeps the choice in
`Tab::view_after_turn` and `apply_waiting_views` (every frame, before deliveries) applies it once `tab_state` is no longer busy.

Do not put a `Sense::click()` widget over the whole pane to detect "the pane was clicked": it is on top and swallows the clicks of the
buttons and the text field. Read the pointer press instead (`ui/chat.rs`).

**Why:** the first version of the flow tests failed on four terminal tests once Claude tabs defaulted to Chat; those tests now open
terminal tabs (`Env::start()` sets `chat_by_default: false`, `Env::start_with(true)` is the chat one).
**How to apply:** a new feature that reads `tab.session` or sends bytes to the PTY needs a `chat` branch (`chat_insert`, `chat_deliver`).

---
title: Changing the model or the effort of a chat tab restarts its process on the same session
summary: The composer pickers save to ProjectCfg (tab_models / tab_efforts), drop the ChatProcess and start_chat resumes the tab's session; Antigravity's effort lives in the model name.
touches: [crates/core/src/models.rs, crates/core/src/agents.rs, crates/app/src/deck/chat.rs, crates/app/src/ui/chat.rs]
---

The CLIs take `--model` / the effort only at launch, so a pick in the chat composer (`Deck::chat_set_model`, `chat_set_effort`)
is: `Core::set_tab_model` / `set_tab_effort` (saved per tab) → take and drop `ChatTab.proc` (the old receiver goes with it, so no
"process ended" notice arrives) → `start_chat`, which asks `spawn_spec_chat` for the launch; that reads the saved model and
`Core::instance_effort` and resumes the tab's session (`tab_sessions`) like any restart. The Terminal view launches through
`spawn_spec_for`, which reads the same two values, so the choice follows the tab in both views and survives a Holt restart.

- **Where each effort lives:** Claude and Codex: `ProjectCfg::tab_efforts` (id → `low|medium|high`; Claude `--effort`, Codex
  `-c model_reasoning_effort=…`). Antigravity: only the suffix of the model name (`gemini-4-argon-high`), so its effort picker
  swaps the suffix in `tab_models` and the model picker collapses the variants (`models::agy_bases`/`agy_variant`) and runs the
  chosen base at the effort in force. A tab effort wins over the task's effort (`instance_effort`).
- **Codex has no model list** on this machine (`ModelList::Unlisted`), so the model stays plain text there; its effort works.
- The pickers are disabled while a turn runs (restarting would kill it).

**Why:** the first idea, "apply on the next turn" without a restart, is impossible for these CLIs.
**How to apply:** a new backend only needs `effort_args` / `model_args` in its `ChatBackend`; the tests are in `crates/core/tests/chat.rs`
(`a_model_and_effort_picked_in_the_chat_restart…`, the Antigravity and Codex ones).

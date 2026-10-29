---
title: OpenCode TUI requires --standalone to read model and environment instructions
summary: OPENCODE_CONFIG_CONTENT is only read by the process launching the server; the TUI always launches with --standalone and build_agent_env cleans up inherited variables.
touches: [crates/core/src/models.rs, crates/core/src/agents.rs]
---
OpenCode's TUI lacks a `--model` flag or command-line option for system instructions (only `opencode run` and `mini`
support `-m`, and the `model#high` variant only exists under `run -m`). In interactive tabs, the model and
`data/holt-instructions.md` are passed in `OPENCODE_CONFIG_CONTENT` (`models::opencode_tui`), which overrides
user configuration and appends instructions to the project's `AGENTS.md`.

**Why:** `OPENCODE_CONFIG_CONTENT` is only read by the process that boots the OpenCode server. Without
`--standalone`, the TUI reuses an existing background shared server that ignores the new tab's environment. Conversely,
child processes spawned from inside an OpenCode tab would inherit `OPENCODE_CONFIG_CONTENT` from that tab.

**How to apply:** every OpenCode tab launches with `--standalone` (`models::opencode_tui`), with or without a selected model, and
strips the `#variant` suffix from the model name. In `agents::build_agent_env`, keep `OPENCODE_CONFIG_CONTENT` in the `REMOVE`
list to avoid leaking into child processes or bridge tests; `spawn_spec_for` injects the new tab's configuration after this filter.

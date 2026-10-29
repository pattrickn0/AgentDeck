---
title: Only CommandDeck config is still migrated; all other legacy names have been removed
summary: Variables COMMANDDECK_*, x-commanddeck-token, marker commanddeck:, and .commanddeck.lock are no longer supported (t32); only the copy from %APPDATA%/CommandDeck/config.json remains in config.rs.
touches: [crates/core/src/env.rs, crates/core/src/workspace.rs, crates/core/src/bridge.rs, crates/core/src/config.rs, crates/core/src/lib.rs, crates/core/src/agents.rs, crates/cli/src/bin/**]
---
The app was renamed from CommandDeck to AgentDeck (task `t20`), initially retaining legacy names for compatibility. Task `t32`
(2026-10-23, requested by the user prior to promoting `dev` → `main`) eliminated all of them, along with their tests:

- **Environment variables:** only `AGENTDECK_*` (`agentdeck_core::env_var`, `env.rs`); `build_agent_env` filters strictly to this prefix,
  with `ASK_WAIT` as an exception.
- **Bridge:** only `x-agentdeck-token` (`bridge::token_header`); `ask-agent` sends only this header.
- **`AGENTS.md`:** `workspace.rs::marker_re` recognizes `agentdeck` as well as legacy `orquestra` and `jwic`, but no longer `commanddeck`.
  An existing `commanddeck:` block remains in the file alongside the new block (remove it manually).
- **Lock:** only `data/.agentdeck.lock`.

**One single exception remains**: `config::legacy_config_files` starts with `%APPDATA%/CommandDeck/config.json`. If
`AgentDeck/config.json` is missing, the app **copies** CommandDeck's file (leaving the old one untouched) and preserves projects
**and tabs** (the ones from "CommandDeck Terminal"/Electron and "orquestra" still clear tabs). Under `AGENTDECK_HOME`, the app runs isolated
and does not read any user configuration.

**Why:** only in this case does omitting migration carry real cost: users upgrading who never opened AgentDeck would lose their project
list. Other legacy names, when missing, only require closing and reopening a tab (variables, header, lock) or removing an old
block from `AGENTS.md`.
**How to apply:** do not delete config copying until a release following promotion to `main` (item in `backlog.md`);
when removing it, clean up `legacy_config_files`, `is_commanddeck_config`, the copy logic in `Config::load_from`, and `*_commanddeck_*` tests.
Never reintroduce legacy variable prefixes or headers "just for backwards compatibility".

---
title: The AgentDeck → Holt rename keeps old names only for migration, listed in MAINTENANCE.md §6
summary: What an existing AgentDeck install needs (config copy, data/ renames, markers, worktree roots, env vars, header, menu entry) is kept in one list with "remove after 0.4"; never add new legacy names "for compatibility".
touches: [crates/core/src/env.rs, crates/core/src/paths.rs, crates/core/src/config.rs, crates/core/src/activation.rs, crates/core/src/workspace.rs, crates/core/src/bridge.rs, crates/core/src/lib.rs, crates/core/src/tasks/**, crates/cli/src/bin/**, crates/app/build.rs, scripts/install-desktop.sh]
---
The app was renamed CommandDeck → AgentDeck (`t20`, with every legacy name dropped in `t32`) and then AgentDeck → Holt
(`t1`, 2026-10-29). The second rename kept the user's projects, tabs, tasks and history **without manual steps**, so the old
names stay in the code, each marked `Migration (remove after 0.4, memory/MAINTENANCE.md §6)`:

- **Config:** `config::legacy_config_files` starts with `<config dir>/AgentDeck/config.json`; if `Holt/config.json` is
  missing it is **copied** (tabs included; the old file stays). The CommandDeck / "CommandDeck Terminal" candidates were
  dropped; `orquestra` is still read, without tabs.
- **`data/` entries:** `paths::LEGACY_DATA_ENTRIES` renames `.agentdeck.lock`, `.agentdeck.bridge`, `agentdeck.log`,
  `agentdeck-instructions.md` (like `dados/` before). While an old AgentDeck holds `.agentdeck.lock`, nothing moves.
  On Linux with an unwritable app folder, `~/.local/share/AgentDeck` moves to `Holt` (`paths::adopt_legacy_home`).
- **Second launch:** `activation` also looks at `.agentdeck.lock`/`.agentdeck.bridge` (and sends `x-agentdeck-token` to
  that bridge); `lock_data_dir` refuses to start while the old lock is held.
- **`AGENTS.md`:** `workspace::marker_re` accepts `agentdeck:` blocks, so `ensure_agent_context` **replaces** them and the
  merge helpers (`only_marker_blocks_differ`, `restore_marker_block`) treat both markers as the app's block.
- **Task worktrees:** tasks keep the `.agentdeck/worktrees/<id>` path stored in their JSON; `tasks::safe_worktree` takes the
  repository root and accepts `.holt/` and `.agentdeck/`. New tasks go to `.holt/worktrees`; `.holt/` is added to
  `info/exclude` (the old line stays).
- **Env vars / header:** `env::env_var_os` falls back to `AGENTDECK_*`, `env::is_app_var` knows both prefixes, and the
  bridge accepts `x-agentdeck-token`: tabs opened before the update keep working until reopened.
- **Linux menu:** `build.rs` and `install-desktop.sh` delete `agentdeck.desktop` / `agentdeck.png` when installing Holt's.

**Why:** the CommandDeck → AgentDeck rename left several things manual (an old `AGENTS.md` block beside the new one, tabs
to reopen, an unmoved lock), and the user had to clean them up.
**How to apply:** tests are named `*_agentdeck_*`; when removing the migration (after 0.4) delete every item above, those
tests and the §6 list in one go. Do not add new legacy names or prefixes "for compatibility": rename, migrate once, list it
in §6. `COMMANDDECK_*`, `x-commanddeck-token` and the `commanddeck:` marker are gone for good.

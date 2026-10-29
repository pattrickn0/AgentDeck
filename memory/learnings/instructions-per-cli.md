---
title: Each CLI receives AgentDeck instructions through its own channel
summary: Claude, Codex, OpenCode, Aider, and Interpreter via CLI flags/config; Antigravity via AGENTS.md in data/ with --add-dir; Goose and Copilot only via project AGENTS.md (created with app marker block only).
touches: [crates/core/src/agents.rs, crates/core/src/models.rs, crates/core/src/workspace.rs, crates/core/src/headless.rs, AGENTS.md]
---
AgentDeck instructions (rule against internal subagents, `ask-agent`, shared memory) cannot depend on the
user's `AGENTS.md`: a new project lacks one, and an agent lacking instructions launches its CLI's built-in subagents.

- **Claude:** `--append-system-prompt`. **Codex:** `-c developer_instructions=<TOML>`; in a `.cmd` shim quotes are lost
  (`cmd_safe_arg`), so raw text is passed, which Codex accepts as a string when not valid TOML. Not installed on the
  development machine: unverified in practice.
- **OpenCode:** `{"instructions": ["<file>"]}` in `OPENCODE_CONFIG_CONTENT` (merged with user configuration). Verified: with
  `opencode run --standalone` file rules were respected. Only the tab's dedicated server reads this variable, so the TUI
  now **always** starts with `--standalone`, with or without a model.
- **Aider:** `--read <file>`. **Interpreter:** `--custom_instructions` (unverified).
- **Antigravity:** no system option available, but `agy` reads `AGENTS.md` in each folder passed via `--add-dir`, **combined** with
  the project's file. Verified on 2026-10-24 (`agy` 1.4.7, Flash): a rule in an external `AGENTS.md` applied only when
  `--add-dir` was present, alongside rules from the project `AGENTS.md`. The app writes `data/agy/<instance>/AGENTS.md` (with name and
  role, similar to Claude) and passes that folder (`write_agy_instructions`): nothing added to the user's repository.
- **Goose, Copilot:** only read the project `AGENTS.md`. The app generates one containing solely the marker block when the folder lacks one, notifies in
  Chat, and never modifies an existing file (`Core::ensure_agents_file`). Unverified (not installed locally).
- Generic instructions (file and block) do not list installed agents: otherwise the file would vary across machines and
  dirty `git status`.

**Why:** new user report (23/10): agents were not using `ask-agent` and were spawning CLI subagents.
**How to apply:** for a new CLI = find its instruction flag (`--help`, documentation) and add to `instruction_args`;
before falling back to `reads_only_agents_md`, test whether it reads `AGENTS.md` from an added directory (like `agy`): use an easily
observable rule ("start answer with PINEAPPLE-7") in an `AGENTS.md` outside the project, with and without the option. What quota limits
prevent testing goes to `memory/manual-tests.md`.

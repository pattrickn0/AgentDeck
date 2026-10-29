---
title: Each CLI stores folder trust in a different place and with different rules
summary: Claude inherits from the parent folder (~/.claude.json), Codex from repository root and accepts -c, Antigravity only by exact path (settings.json).
touches: [crates/core/src/trust.rs, crates/core/src/agents.rs, crates/core/src/tasks/**]
---
Verified on 2026-10-25 (Claude Code 2.1.310, Codex 0.164.0, `agy` 1.4.7), opening each CLI in a PTY inside a new directory:

- **Claude:** `projects["C:/pasta"].hasTrustDialogAccepted` in `~/.claude.json` (forward slashes). An accepted parent directory applies
  to child folders, so a task worktree inherits trust from the project. There is no CLI flag for this.
- **Codex:** `[projects.'c:\pasta'] trust_level = "trusted"` in `config.toml`, keyed by the **repository root** (the worktree's root
  is that of the main repository). `-c projects={'<pasta>'={trust_level='trusted'}}` solves it without persisting anything; case
  does not matter. **Duplicate keys in TOML crash Codex** ("invalid type: string ... expected a map"): the project tab passes
  only a single key.
- **Antigravity:** `trustedWorkspaces` in `~/.gemini/antigravity-cli/settings.json`, matched against the **exact path string**:
  neither parent directories (`C:\` and `C:\Users\pattr` were present and it still prompted), nor lowercase, nor `/`.
  `--add-dir` does not interfere (the earlier hypothesis in pending work was wrong).

**Why:** background tabs (`ask-agent`, tasks) were hanging on the trust prompt unnoticed.
**How to apply:** the app configures trust before launching (`trust::prepare`); the home directory and drive root are never added. When testing,
use a new directory and remove test entries from `~/.claude.json` and `agy`'s `settings.json` afterwards. Sending Enter
via PTY does not accept `agy`'s prompt (raw Enter does not reach it), so test via the list rather than keystrokes. For a new CLI: identify
where it persists trust and add it to `trust::prepare`.

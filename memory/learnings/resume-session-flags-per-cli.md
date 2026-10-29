---
title: Resuming a tab's conversation: the session id is the tab's own, flags checked per CLI
summary: Claude --session-id/--resume (+ SessionStart hook), OpenCode --session ses_...; never --continue; a conversation with no message yet cannot be resumed (Antigravity and Codex: see resume-antigravity-and-codex.md).
touches: [crates/core/src/resume.rs, crates/core/src/agents.rs, crates/core/src/bridge.rs, crates/cli/src/bin/ask-agent.rs, crates/app/src/deck/tabs.rs, crates/core/src/config.rs]
---
Each tab keeps the CLI's session id in `ProjectCfg.tab_sessions` (tab id → id), saved as soon as it is known and dropped
when the tab is closed for good (`Core::close_instance`: the tab's ×, a finished or discarded task, a removed project).
Quitting the app keeps it. `resume.rs` holds the rules; `agents::spawn_spec_for` asks for the plan (`Core::plan_launch`).

Checked on this machine (2026-10-29, Claude Code 2.1.312, OpenCode 2.0.32):

- **Claude:** `--session-id <uuid>` (must be a UUID) names a new conversation; `--resume <uuid>` continues it with the same
  id. `--session-id` on an id that already exists fails ("Session ID ... is already in use", exit 1); `--resume` on one that
  does not exist fails ("No conversation found with session ID", exit 1). The transcript is
  `<CLAUDE_CONFIG_DIR or ~/.claude>/projects/<cwd slug>/<id>.jsonl`, written with the first message. So a tab that was
  closed before any message is launched again with `--session-id` and the same id (`resume::claude_has_session`), not
  `--resume`. The `SessionStart` hook gets JSON on stdin with `session_id`, `source` (`startup`, `resume`, `clear`, `compact`)
  and `transcript_path`; `/clear` gives a new id, so the hook (`ask-agent --signal session`, which posts to `POST /signal`
  `{"state":"session","session":"<id>"}`) rewrites the saved one. `--continue`/`-c` is "the most recent conversation in
  the folder": with two tabs of one CLI it would pick the other tab's, so it is never used.
- **OpenCode:** `--session <id>` ("session ID to continue, or to create if it does not exist"), also in the TUI with
  `--standalone`. The id must start with `ses` ("Expected a string starting with ses"): we generate `ses_` + 26 hex digits and
  `opencode run --session ses_...` created it; an unknown id in the TUI opens an empty conversation (nothing is saved until
  the first message). Sessions belong to the folder (`opencode session list` is per project). **Not tracked:** `/new` inside the
  TUI starts a session we do not hear about, so the next reopening resumes the older one (OpenCode has no hook channel here).
- **A resume that fails right away:** the process ended within `resume::FAIL_WINDOW` (10 s) with a non-zero code after a launch
  that continued a saved id (`SpawnSpec::resumed`) → `Deck::reopen_without_resume` forgets the id, starts the tab again
  as a first launch, and puts one notice in the chat. A task's tab also gets its boot line again
  (`Core::set_restart_boot_prompt`: it points to the brief file), because a fresh conversation knows nothing of the task.
  The new launch has a new id and `resumed == false`, so it cannot loop.

**Why:** backlog item 3 (T4): every tab used to open an empty conversation after closing or crashing the app.
**How to apply:** a new CLI joins by adding its flag to `resume::launch_args`, `supports` and `new_session_id` and, if the id
is not known up front, a way to report it (Antigravity from its `--log-file`, Codex from `notify`: see
`resume-antigravity-and-codex.md`). Test the
arguments in `resume.rs` and the flow with the fake agent in `tests/bridge.rs`
(`a_claude_tab_resumes_its_own_conversation_and_a_refused_resume_starts_over`). Never add `--continue`.

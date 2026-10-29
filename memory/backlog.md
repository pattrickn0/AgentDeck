# Pending items

The **next steps**, at most 10, in the order they will be done (reviewed with the user on 2026-10-24). When completing
an item, **remove it from here** and add a line of what was done to [history.md](history.md) (document rules in
[OVERVIEW.md](OVERVIEW.md)). This file does not keep completed items.

What is not among the next steps stays in [ideas.md](ideas.md): undated ideas and known limitations. An idea
only enters here when another item leaves (the list never exceeds 10). **Manual tests** stay in [manual-tests.md](manual-tests.md); a
test that fails becomes an item here. Product direction and decision-makers are in [AGENTDECK.md](AGENTDECK.md).

## Now

1. [ ] **Validation in use (user).** The terminal without blank lines after resizing (`t37`; if it returns,
   "Record the terminal (diagnostics)" in the tab menu and send `data/pty/`), the document editor (`t39`), quota refusal
   (`ask-agent --models antigravity`, `--team`), and choice by model capability (`t52`: task level,
   "capability" block in `--models`; see manual-tests.md). When the metric has enough tasks, review the `capability.json` table and
   default models per agent (`defaults`). Also from the user: rename the local `commanddeck-dev` folder (the
   GitHub repository is already named `AgentDeck`).
2. [ ] **Background and system tray** (decided on 2026-10-24). T1a (`t44`): `Deck::tick` was moved out of rendering and
   `Deck::background` runs from the event loop with the window hidden or minimized. T1b (`t48`, done, needs validation in use):
   the X hides the window to the tray, without asking (Windows: `tray-icon`; Linux: `ksni`; without `StatusNotifierWatcher` the X
   quits); the tray has Open and Exit (Exit prompts if there is an unsaved edit or a tab mid-turn); opening the
   program again brings the instance back (`POST /activate`, see ARCHITECTURE.md, "Single instance and
   system tray"). Remaining to verify in use: the walkthrough is in manual-tests.md ("Tray and second instance"), especially a long turn
   ending with the window hidden and the icon on Linux (vanilla GNOME tray lacks `StatusNotifierWatcher`; requires
   AppIndicator extension), which was only compiled, not run.
3. [ ] **Resume each tab's conversation, part B: Antigravity and Codex** (T4). Part A (t76) is done: `ProjectCfg.tab_sessions`,
   `resume.rs` (plan per CLI: first / resume / fallback), `ask-agent --signal session` + `POST /signal` for a CLI to report its id,
   the fallback in `Deck::reopen_without_resume`, Claude and OpenCode (see `learnings/resume-session-flags-per-cli.md`). Left:
   Antigravity `--conversation <id>`, with the id read from the tab's `--log-file`; Codex `codex resume <id>`, with the id
   from `notify` (its JSON argument; `ask-agent --signal session <json>` already accepts a JSON argument with `session_id`, extend
   `signal_body` for Codex's key), and on Windows resolve the npm shim to the executable so Codex gets hooks. Add each CLI to
   `resume::supports`/`launch_args`/`new_session_id`.
4. [ ] **Measured memory, then the graph** (T3, then T7). Automatic measurement over completed tasks: ground truth is
   learnings from files the task actually changed; prediction is what each method would deliver reading only the brief (today;
   today + one-line index of all; graph); score = hits and delivered size, with learnings as they were at the
   task's base. The graph begins with co-changes from `git log` (no merges, no large commits, no `memory/**`), which
   also suggests task scopes; only adopted for memory if it outperforms the index.
5. [ ] **Task phase and "idle for N min"** (T5), solely through hooks: Claude's `PostToolUse` (the last tool used
   becomes "editing", "testing"…); other CLIs display working/idle and for how long.
6. [ ] **Durable mail** (T6). Messages up to ~200 characters on a single line continue being typed; long ones are saved to
   disk and only a notice is sent to the terminal (`ask-agent --read <n>`); the notice is repeated at turn end if unread;
   reply deadline counts from when read; pending messages survive app exit.
7. [ ] **Worktree folder not removed upon validation: find root cause.** Logging already exists (2026-10-24, `file_lock.rs`): on
   every `remove_task_worktree` failure, `agentdeck.log` and the task warning report which files remained and which process holds
   them open (Restart Manager), or if only empty directories remained (a process with its current directory inside it). Awaiting
   the next real occurrence: read the notice (search for "worktree" in log) and fix the cause.
8. [ ] **Retire integrated Conversation.** The per-task timeline already exists (t43, MESSAGES section in detail view, stored in
   `Task::messages`); needs validation in use and then removal of Conversation. What the timeline lacks: user messages to the
   tab (`FeedKind::User`) and system notices.

## Later (on the list, not yet)

9. [ ] **Installer for Windows**, and in the same version remove config migration from `CommandDeck/` (`config::legacy_config_files`;
   list and rationale in [MAINTENANCE.md](MAINTENANCE.md), section 6).

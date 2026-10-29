# Pending items

The **next steps**, at most 10, in the order they will be done (reviewed with the user on 2026-10-24). When completing
an item, **remove it from here** and add a line of what was done to [history.md](history.md) (document rules in
[OVERVIEW.md](OVERVIEW.md)). This file does not keep completed items.

What is not among the next steps stays in [ideas.md](ideas.md): undated ideas and known limitations. An idea
only enters here when another item leaves (the list never exceeds 10). **Manual tests** stay in [manual-tests.md](manual-tests.md); a
test that fails becomes an item here. Product direction and decision-makers are in [HOLT.md](HOLT.md).

## Now

1. [ ] **Validation in use (user).** The terminal without blank lines after resizing (`t37`; if it returns,
   "Record the terminal (diagnostics)" in the tab menu and send `data/pty/`), the document editor (`t39`), quota refusal
   (`ask-agent --models antigravity`, `--team`), and choice by model capability (`t52`: task level,
   "capability" block in `--models`; see manual-tests.md). When the metric has enough tasks, review the `capability.json` table and
   default models per agent (`defaults`). Also from the user, after the rename to Holt (`t1`): rename the GitHub repository
   `pattrickn0/AgentDeck` → `pattrickn0/Holt` (update `origin`) and, optionally, the local folder `~/AgentDeck` → `~/Holt`
   (then rebuild so the menu entry points at the new path); steps in [MAINTENANCE.md](MAINTENANCE.md), section 6.
2. [ ] **Background and system tray** (decided on 2026-10-24). T1a (`t44`): `Deck::tick` was moved out of rendering and
   `Deck::background` runs from the event loop with the window hidden or minimized. T1b (`t48`, done, needs validation in use):
   the X hides the window to the tray, without asking (Windows: `tray-icon`; Linux: `ksni`; without `StatusNotifierWatcher` the X
   quits); the tray has Open and Exit (Exit prompts if there is an unsaved edit or a tab mid-turn); opening the
   program again brings the instance back (`POST /activate`, see ARCHITECTURE.md, "Single instance and
   system tray"). Remaining to verify in use: the walkthrough is in manual-tests.md ("Tray and second instance"), especially a long turn
   ending with the window hidden and the icon on Linux (vanilla GNOME tray lacks `StatusNotifierWatcher`; requires
   AppIndicator extension), which was only compiled, not run.
3. [ ] **Measured memory, then the graph** (T3, then T7). Automatic measurement over completed tasks: ground truth is
   learnings from files the task actually changed; prediction is what each method would deliver reading only the brief (today;
   today + one-line index of all; graph); score = hits and delivered size, with learnings as they were at the
   task's base. The graph begins with co-changes from `git log` (no merges, no large commits, no `memory/**`), which
   also suggests task scopes; only adopted for memory if it outperforms the index.
4. [ ] **Task phase and "idle for N min"** (T5), solely through hooks: Claude's `PostToolUse` (the last tool used
   becomes "editing", "testing"…); other CLIs display working/idle and for how long.
5. [ ] **Durable mail** (T6). Messages up to ~200 characters on a single line continue being typed; long ones are saved to
   disk and only a notice is sent to the terminal (`ask-agent --read <n>`); the notice is repeated at turn end if unread;
   reply deadline counts from when read; pending messages survive app exit.
6. [ ] **Worktree folder not removed upon validation: find root cause.** Logging already exists (2026-10-24, `file_lock.rs`): on
   every `remove_task_worktree` failure, `holt.log` and the task warning report which files remained and which process holds
   them open (Restart Manager), or if only empty directories remained (a process with its current directory inside it). Awaiting
   the next real occurrence: read the notice (search for "worktree" in log) and fix the cause.
7. [ ] **Retire integrated Conversation.** The per-task timeline already exists (t43, MESSAGES section in detail view, stored in
   `Task::messages`); needs validation in use and then removal of Conversation. What the timeline lacks: user messages to the
   tab (`FeedKind::User`) and system notices.

## Later (on the list, not yet)

8. [ ] **Installer for Windows**, and in the same version (0.4) remove the AgentDeck → Holt migration (config copy, `data/`
   renames, old env vars/header/marker/worktree root; the list is in [MAINTENANCE.md](MAINTENANCE.md), section 6).

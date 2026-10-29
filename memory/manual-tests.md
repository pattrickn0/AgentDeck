# Manual test script

Tests to perform **manually, on Windows, with real agents**. Automated tests (`cargo test`) and Linux screenshots
have already passed; what is here cannot be verified without your machine: ConPTY, Git for Windows,
Credential Manager, Windows fonts, installed CLIs, and actual agent behavior.

Remaining development work is tracked in [backlog.md](backlog.md); overview in [OVERVIEW.md](OVERVIEW.md).

**How to use**
- Mark `[x]` on what passed. On anything that fails, note below the item: what you did, what was expected, what happened,
  a screenshot, and if the app crashed, the end of `data/crash.log`.
- Numbers in parentheses (e.g. 202, 409) are reference codes for fixes.
- Items with ★ are the most important (major changes not yet verified on Windows).
- "(real use, t1–t16)" marks what the task desk itself has already performed in practice, verified in task events
  (`data/tasks/<project>.json`): counts as a passed test, but repeat if the area changes.
- The script is organized by **area** (the same areas as in [ideas.md](ideas.md)). Start with Preparation and areas with ★.

---

## Preparation

- [x] Compile and launch: `cargo build --workspace --release` and `.\target\release\agentdeck.exe` (all three `.exe` in the same directory).
- [x] Have installed, if possible: Claude Code, Antigravity, Git for Windows. Optional: Codex (`npm i -g @openai/codex`), OpenCode.
- [x] (Optional) Compile and launch on **Linux** following the "Build and run (Linux)" section of the README; note distribution and whether X11 or Wayland.
- [x] First launch (without `config.json`) and relaunching after closing **all** tabs: opens **without tabs**; no agent launches automatically.
- [x] Open a personal project that is a git repository ("+" in the PROJECTS section on the sidebar).

## Tasks desk ★

Via CLI (`ask-agent --task`):

- [x] (real use, t1–t16) In a git project, from an orchestrator tab: `ask-agent --task new "test" --start -` with request on stdin. The task tab opens in the background, its terminal is at `<repo>\.agentdeck\worktrees\t1`, the request and rules arrive typed, and the main repository's `git status` remains clean.
- [x] (real use, t1–t16; `--boss` was not verified) In the task tab, make a commit and run `ask-agent --task done t1 -`. `--task show t1` shows the report and diff, and the task's `--boss` reaches whoever created it.
- [x] (real use, t1–t16) In the task tab, write the report to a file with accented characters (e.g. `C:\Temp\relatorio.md`) and run `ask-agent --task done t1 --file C:\Temp\relatorio.md`: `--task show t1` displays full accents (without `?`).
- [x] (real use, t1–t16) `--task return t1 "fix it"` types the feedback into the tab. Afterwards, `--task merge t1` creates the merge commit and the task transitions to **awaiting validation**: the tab, folder, and branch remain, and the task branch points to the same commit as the base.
- [x] (real use, t1–t16) `--task validate t1` (awaiting validation) closes the tab and deletes the folder and branch, even with the agent process still running; the task becomes "completed".
- [ ] `--task validate` outside "awaiting validation" rejects (409).
- [ ] `--task reject t1 "does not save"` (awaiting validation) returns the task to the same agent: text arrives in the tab explaining that the work was already merged along with the rejection reason. With the tab closed and/or folder deleted manually, the tab reopens in the worktree (recreated). Then `done` → `merge` → `validate` complete the cycle.
- [ ] **Reject → done without commit → merge → validate** (t18): on a task awaiting validation, `--task reject t1 "might not break"`. In the tab, the agent verifies, changes nothing, and runs `ask-agent --task done t1 --file <file>` (task goes to review). `--task merge t1` (or "Merge") does not reject: task returns to **Awaiting validation**, history states "no new commits since last merge (<hash>); awaiting validation again", and base `git log` gains no new merge commit. "Validate" completes.
- [ ] **`--task done` while awaiting validation** (t18): with the task awaiting validation, make a commit directly in its tab and run `ask-agent --task done t1 --file <file>`: the task transitions to **In review** ("agent submitted more work after merge"). Without new commits, the same command returns 409 stating the task was already merged and that user decision is needed (Reject).
- [ ] **Validate with deleted base branch** (t18): with the task awaiting validation, manually delete the base branch (`git branch -D <base>` after switching branches) and validate: completes, with the note "base … no longer exists" in history.
- [ ] With main repository on another branch, or with a modified tracked file, merge rejects with an explanation and task stays in review.
- [x] (real use, t1–t16) Conflict (same line modified in base and task): merge is aborted, task stays in review, and main repository remains clean.
- [x] (real use, t1–t16) `--task discard` on an in-progress task closes the tab and deletes the worktree and branch.
- [ ] `--task discard` with worktree folder already deleted manually also cleans up branch and record.
- [ ] Relaunch the app with an in-progress task: tab reopens in worktree, and next task number is not repeated.
- [ ] With "Open agent tabs" disabled, `--task start` rejects with explanation.
- [ ] **Initial prompt on launch (Antigravity, Claude, OpenCode):** `--start` one task for each. The agent starts on its own without anyone typing "you can start" (in Antigravity, even with "Verbosity now defaults to medium" on screen), and reads `data/tasks/<project>-<id>-brief.md`. Check if OpenCode reads the file from outside workspace without asking for permission. Rejecting with tab closed reopens with the reason (`-feedback.md`).

Via the **Tasks** tab on the right panel:

- [ ] Opening the app displays Tasks as the first tab on the right panel. In a git project without tasks, shows explanation and "+ New task".
- [ ] **Quota-based selection (t49):** in `+ New task`, check the percentage next to Claude, Antigravity, and Codex and verify the one with lowest usage is preselected; manually choosing another agent and reopening confirms manual choice persists. Create a task without `--agent` and check history for the reason with candidate quotas.
- [ ] **Level and capability (t52):** in `+ New task`, "Level" (Simple | Medium | Complex; Medium by default). With automatic agent, choosing Complex changes recommendation to a capable model (with Codex defaulted to `gpt-6-luna`, which only handles simple, it is never recommended for medium or complex tasks); manually choosing a model below the level (e.g. Claude with `haiku` and Complex) shows "Capability warning" without blocking. `--task new "x" --level complex --file <p>` records the level (`--task show` displays "level: complex") and, without `--agent`, history shows reason "complex: claude sonnet, capable and with N% free; …". `--level huge` returns "invalid level".
- [ ] **`--models` with capability (t52):** after agents, the "capability" block lists each model with level and source ("Coding Agent Index 59.3 (2026-10)"); after 3 tasks at a level, the line shows measured performance ("medium: 4 of 5 first try, …"). A `data/capability.json` with `{"models":[{"name":"GPT-6 Luna","aliases":["gpt-6-luna"],"score":50,"source":"test","date":"2026-11"}]}` upgrades Luna to `medium` on the next call.
- [ ] **Quota warning (t49):** with target agent above 80% and another installed official CLI below 50%, run `ask-agent <agent> "message"`: warning appears before response and message still delivers; with another agent only at 50% or more, no warning appears.
- [ ] "+ New": with empty title, button is disabled; base defaults to current branch; non-existent base gives inline error; Ctrl+Enter creates. With "Start now", detail opens in-progress and agent tab appears in worktree with prompt typed. Without it, task stays in draft and "Start" begins it.
- [ ] **Effort in form** (t25): in "+ New", the EFFORT field defaults to "Default". With Claude, choose "High" and create with "Start now": card and detail show "Claude · <model> · high" and tab opens with `--effort high` (Claude shows effort in statusline or in `/model`; if not shown, check process command line). With Antigravity, choose a `…-argon-low` model and "High": task model becomes `…-argon-high`; with no model chosen selector is disabled with tooltip on hover. With OpenCode (and Codex) selector is disabled with tooltip. `ask-agent --task new "x" --effort high --start --file p.md` accepts `low|medium|high` (or aliases baixo|medio|alto) and `--effort max` errors out.
- [ ] **Scope and queue** (t45): in "+ New", SCOPE field accepts one path per line. (1) Create task A with scope `crates/core` and "Start now"; create task B with scope `crates/core/src/git.rs` and "Start now": B opens in detail as **Queued** ("Queued · waiting for t1" on card, yellow banner "Queued: scope overlaps with task t1…" in detail), without tab or folder in `.agentdeck/worktrees`, only the Discard button. (2) Merge A via button: B starts automatically, its tab opens, and worktree already contains what A introduced (`git log` in its folder). Repeat discarding A instead of merging. (3) Two tasks with `crates/app` and `crates/cli` start together. (4) Two tasks that only mention `memory/backlog.md` (and `Cargo.lock`) beyond their scopes start together. (5) A task without scope starts immediately alongside an in-progress task. (6) A task that delivers changes outside its scope shows "Modified outside scope: <file>" banner in review and remains mergeable. (7) With B queued, close and reopen the app: it remains queued (card and `ask-agent --task list` show "queued" and "(waiting for t1)") and starts when A finishes. (8) `ask-agent --task new "x" "brief" --scope crates/core memory/OVERVIEW.md --start` displays scope on creation; `--scope 'crates/*.rs'` and `--scope C:\x` error out; `--task show` lists scope and, when queued, what it waits for. A task awaiting validation does not hold the queue: with A in "Awaiting validation", B (same scope) starts immediately.
- [ ] Close the tab of an in-progress task: detail displays "Reopen tab", reopening it in the worktree with task model.
- [ ] Agent runs `ask-agent --task done tN -`: card automatically transitions to "In review", icon badge increments, and "N files +X −Y" appears.
- [ ] In review detail: report, changes per file, and "View full diff" opens diff **in center** (Documents mode, t19: file list on left, colors, long lines soft-wrapped, no horizontal scrolling); right panel remains on detail with review buttons visible. **Open in Documents** under Brief and Report opens each formatted.
- [ ] **"View merged changes"** (t18): in awaiting validation (and later in completed), detail displays short merge hash next to `branch → base` and changes button reads "View merged changes"; it opens (in center) only what the merge introduced (excluding earlier base changes) and "←" goes back. A task merged before this version (without hash) does not show button. In review and in progress button remains "View full diff".
- [ ] "Return" with Ctrl+Enter: task returns to in-progress and text arrives in agent tab, reopening it if closed.
- [ ] "Merge into <base>" requests confirmation. If main repo is dirty, shows refusal and task stays in review. If clean, merges and moves to **Awaiting validation** (icon badge keeps counting); agent tab remains open.
- [ ] **Merge refusal with resolution options** (t35). In review, with dirty main repository (tracked file modified, uncommitted), "Merge" → "yes": warning explains in plain language ("You have uncommitted changes…") and offers **Stash my changes and merge** and **View uncommitted changes** (lists all files). The first option stashes (`git stash`), merges, and restores: task moves to "Awaiting validation", your changes remain in directory, and amber banner notes they were restored; `git stash list` is empty (or contains only pre-existing stashes). If your change touches the same line as the task, merge succeeds, working tree remains clean (no conflict markers), and notice states changes remain stashed with instructions to recover (`git stash list` and `git stash pop`); verify `git stash show -p` shows your text. An untracked file with the same name as one created by the task rejects before touching anything (no button; move the file). `ask-agent --task merge` continues to reject without stashing anything.
- [ ] **Ask agent** (t35): with uncommitted work in task directory, refusal offers **Ask agent to commit**; with conflict, **Ask agent to resolve conflict**. Clicking returns task to agent tab with prepared request (files listed, task folder, and on conflict, `git merge <base>`); after resubmission, "Merge" passes. Detached HEAD, different branch, ongoing git operation, or missing branch: message only, no action button.
- [ ] Awaiting validation: "Validate" completes without prompting (closes tab, deletes worktree, moves to Completed). "Reject" opens reason field (Ctrl+Enter sends): task returns to in-progress and reason arrives at same agent.
- [ ] "Discard" requests confirmation (in awaiting validation, warns that already merged changes remain in base); then tab closes and worktree and branch disappear.
- [ ] At minimum width (280 px), nothing clips or overflows in list, detail, or form; switching projects displays tasks of that project.
- [ ] A task created by another agent (`ask-agent --task new --start`) appears without clicking refresh.
- [ ] **Attachments in "New task" form** (t11): "Attach files" opens picker; files become removable chips (× removes chip) and, on creation, paths (copies in `data/attachments`) are appended to end of request.
- [ ] **Drag files over open "New task" form** (t11): right panel highlights "Drop to attach to new task" and dropping adds chips; with form closed, highlight targets agent window under cursor.

## Models ★

- [ ] `ask-agent --model sonnet claude@dev "say hello"` from inside a tab: opens "Claude · Dev" with `sonnet` suffix in header and sidebar; Claude reports Sonnet (`/status`).
- [ ] Repeat with `--model opus` while tab is open: 409 `claude@dev is already open with sonnet; use --new to open another with opus`. With same model or without `--model`, delivery is normal. With `--new --model opus`, opens `claude@dev@ana` with opus.
- [ ] `ask-agent --model opencode/big-pickle opencode "hello"`: OpenCode tab opens with "Big Pickle" in statusline. Without `--model`, opens with default.
- [ ] `ask-agent --model x aider "hello"`: 400 "agent aider does not support choosing a model". `--model "a b"`: "invalid model".
- [ ] `ask-agent --list` and `--team` show `claude@dev (sonnet)`.
- [ ] `"roleModels": {"claude@dev": "sonnet"}` in config.json: opening `claude@dev` defaults to sonnet, `claude@qa` falls back to default, and explicit `--model` overrides.
- [ ] Relaunch app: tabs reopen with same model. Renaming preserves model, and closing tab deletes entry from `tabModels`.
- [ ] "+ New agent" › Claude submenu lists Default, opus, sonnet, haiku, fable. OpenCode submenu first shows providers (`opencode`, `opencode-go`) with model counts; selecting one lists only its models, with `-free` first marked "free", and header "‹" returns to providers. With only one connected provider, menu navigates directly to its models without provider step. Antigravity submenu shows "Loading models…" then the list (without CLI or network, just "Default"). Clicking name opens with default.
- [ ] "New task" › Model with OpenCode: menu starts with providers (Default + list), then shows provider models, with `-free` first; "‹ Providers" goes back and "Default" clears model. With single provider, goes directly to models (with "Default"), without "‹ Providers". Model saved on task retains provider prefix (`opencode/…`).
- [ ] Right-click on tab with model shows "Model: <m>" at top of menu.
- [ ] Antigravity with `--model claude-sonnet-5-5-medium`: agy footer shows "Claude Sonnet 5.5 · medium".
- [ ] Codex with `--model <m>` (if installed): opens with model and turn-end notification continues working.
- [ ] **`ask-agent --models`** (t14): from inside a tab, lists a block per installed agent, in `--list` order, with name as accepted by `--agent` and models as accepted by `--model`; `-free` models appear first with "(free)"; OpenCode is grouped by provider (`opencode/: …`); Codex appears as "does not list models" and CLIs without model choice (aider…) as "no model choice".
- [ ] **`ask-agent --models <agent>`** (t14), e.g. `--models opencode` or `--models claude`: lists only that agent; uninstalled agent returns error (404) and invalid name returns "invalid agent". With unresponsive CLI, line indicates "could not list models" and remaining output proceeds.

## Memory ★

- [ ] **`ask-agent --learn <file>`** (t13): in a task tab, `ask-agent --learn crates/core/src/tasks.rs` prints learnings relevant to that file (global ones and those matching `touches`); absolute path within worktree works identically; file without specific learning prints only globals.
- [ ] **`ask-agent --learn` without arguments** (t15): prints global learnings in full and index (one line, `file — summary`) of all others. Outside a task, reads `memory/learnings/` from project repository.
- [ ] **Bridge instructions** (t15): a new agent (tab opened after this version) is aware of `--learn` and `--models` without explicit instruction (instructions say to run `ask-agent --learn <file>` before modifying files).
- [ ] **Learnings at task start** (t13): task brief mentions a file with specific learning (e.g. `crates/core/src/workspace.rs`); message the agent receives (`data/tasks/<project>-<id>-brief.md`) contains, under "What the project has learned", full text of globals and that learning, plus index of the rest.
- [ ] **"Current situation" at task start** (t13/t16): with another task in progress having modified files in branch and **uncommitted files in worktree**, the new task receives "Current situation" section with the other task (id, title, state) and all those files, with warning if any are mentioned in the brief, plus latest base commits. Without other tasks, shows "none". Verify that `git add`/`commit` from other task agent doesn't fail due to this (no `index.lock`).
- [ ] **Learnings in feedback** (t16): return (or reject) a task with feedback referencing a file with specific learning the agent hasn't received: message includes "What the project has learned about referenced files"; a file already in the brief or prior feedback is not repeated. Return also repeats previous feedbacks (up to 5).
- [ ] **Learning created by agent** (t13): task agent creates a file in `memory/learnings/` on branch and cites it in report; after merge, `ask-agent --learn` in another task displays it. File with invalid frontmatter is ignored with log warning without crashing task.
- [ ] **Instructions in every tab, in new project without `AGENTS.md`** (t38): open a new folder (without `AGENTS.md`) as a project and open a tab for each installed CLI. Automated tests cover arguments with mock agents; verify with real CLI that it **received and obeys** (ask "what did AgentDeck instruct you to do when you need another agent?" or ask for review by another agent and check if it opens a tab instead of a subagent):
  - Claude (`--append-system-prompt`), OpenCode (`instructions` in `OPENCODE_CONFIG_CONTENT`; verified with `opencode run --standalone`, TUI remaining);
  - **Codex** (`-c developer_instructions=...`; not installed on dev machine): verify value enters and, with npm installation on Windows (shim `.cmd`, raw text without quotes), `codex` does not reject option;
  - **Aider** (`--read`) and **Open Interpreter** (`--custom_instructions`): verify flags exist in installed version;
  - **Antigravity** (quota exhausted at development time): folder receives `AGENTS.md` containing only app block, Conversation warns once, and agent knows `ask-agent` and subagent rules; reopening does not modify file or warn.
- [ ] `git status` of new project: with Claude, Codex, OpenCode, and Aider, clean; with Antigravity/Goose/Copilot, only `AGENTS.md` (once, without changing on each launch).
- [ ] Project that already has its own `AGENTS.md`: user text remains untouched, only gains app block (standard case), and Conversation does not announce creation.
- [ ] `ask-agent --learn` in project without `memory/learnings/` shows "project does not yet have learnings" and file format (`title`, `summary`, `touches`).

## Bridge and instances

Inter-agent conversation ★

- [x] Ask Claude: *"ask antigravity what is the capital of France"*. The question is **typed into Antigravity tab** and answer returns to Claude.
- [x] Typed message ends with `ask-agent --reply <number> "your reply"`, and agent responds accordingly (reply arrives).
- [x] **Slow response:** request a time-consuming task (over 90 s) from another agent. Claude's `ask-agent` finishes stating response will come later; when it arrives, it is **typed into Claude tab** as a new message.
- [ ] In a **Terminal** tab (shell), run `ask-agent claude "hi"` and let Claude take time: command **waits until completion** (nothing is typed into your shell).
- [x] Role delegation: `ask-agent claude@dev "task"` opens **Claude · Dev** tab in background.
- [ ] Closing tab of caller during wait: nothing hangs; other tab proceeds normally.
- [ ] Long chain: A asks B, B asks C, and C tries to ask a 4th agent → rejected with "maximum depth" (limit is 2 hops).
- [ ] `ask-agent claude@dev "/review src"` via Claude Bash arrives with `/review` intact.
- [ ] Bridge: question with deferred response (202); rename caller tab and destination tab before response arrives. Response is typed into caller's renamed tab.
- [ ] Bridge: after a 202, destination runs `ask-agent <caller> "question"`. Arrives as new question, while original keeps waiting for `--reply N`.
- [ ] Bridge: replying via legacy alias (`ask-agent agy "..."` for Antigravity question) delivers reply, no 409.
- [ ] Headless ("Open agent tabs" disabled): ask a question and kill agentdeck.exe via Task Manager during response. Headless claude/node process terminates alongside it.

When message is typed into tab ★

- [ ] While **you are typing** (without Enter) in an agent tab, another agent's question is **not typed into the middle** (waits ~20 s without keystrokes).
- [ ] While Claude is **mid-response** (or in a long thinking pause), question waits for it to finish (Claude hooks).
- [ ] With Claude paused on a **permission prompt**, question is not typed over the prompt.
- [ ] Pressing Enter in a Claude **menu** (e.g. `/model`) does not jam the queue: subsequent question is still delivered.
- [ ] Typing right after a message is sent to tab: your keystrokes appear **after** the message, never interleaved.

Team and hierarchical agents ★

- [ ] "+ New agent" at bottom of active project list opens agent menu; Ctrl+T with collapsed sidebar opens menu under titlebar.
  - Verified: "+ New agent". Remaining: Ctrl+T with collapsed sidebar.
- [ ] Arrow ▸/▾ on each project and double-arrow button in header collapse/expand agents; state persists across app restarts.
- [ ] Clicking agent from another project switches project and activates tab (including project not yet opened in this session).
- [ ] Model logo replacing dot: pulsing halo when working, ring upon completion when unseen, dimmed when terminated.
- [ ] Agent calling `ask-agent claude@xyz` (new role) spawns new tab beneath it; its first message states "you report to …".
- [ ] Dragging agent onto another: project-colored outline and ghost "→ team of …"; on drop, tree updates and persists across restarts.
- [ ] Dragging manager onto member of their own team: red outline, forbidden cursor, no change. Esc cancels drag.
- [ ] Dropping onto project row returns agent to root.
- [ ] In reorganized agent tab, `ask-agent --team` shows correct manager and team; `ask-agent --boss "hi"` reaches manager; without manager, `--boss` explains it reports to user.
  - Verified: on manager, `--team` shows team and `--boss` explains reporting to user. Remaining: verify in subordinate tab.
- [ ] After reorganization, next message delivered to agent ends with "[AgentDeck] Your position in the team has changed…" (once only; if it ran `--team` earlier, omitted).
- [ ] Closing manager tab: their team moves up to their manager (or root). Renaming tab with team preserves hierarchy.
- [ ] Team: changing agent manager and closing their tab before next message does not consume notice. Closing subordinate without team notifies manager on next message.

Security, environment, and new agents

- [ ] Open **second window** of AgentDeck: exits silently and first window comes to front (see "Tray and second instance"). Only if first is hung does "another window is already using data folder" appear.
- [ ] With `AGENTDECK_HOME` pointing to another folder, second window opens (separate instance).
- [ ] Launching AgentDeck **from inside a tab** of another AgentDeck: parent tab does not crash.
- [ ] Data folder with `&` in path (`AGENTDECK_HOME=C:\tmp\Ana&Bia`): Claude tab (npm shim) opens and turn hooks work.
- [ ] crash.log: normal app usage appends nothing; `AGENTDECK_DEBUG_CRASH=heap` records crash; crash.log exceeding 256 KB rolls to crash.log.1 on launch.
- [ ] **Gemini no longer appears** in "+" or `ask-agent --list` (Antigravity is the Google agent).
- [ ] **Codex**: opens via "+" tab; upon turn completion, question queue advances (`notify` notification). If it prompts for approval to run `ask-agent`, note it.
- [ ] **OpenCode**: opens via "+" tab and responds to `ask-agent opencode "hi"`.
- [ ] Agent installed **via npm without `.exe`** (e.g. Codex or OpenCode, `codex.cmd`/`opencode.cmd`): reply to `ask-agent` without open tab (headless) with message containing quotes, `&`, `|`, and newlines → response returns and **nothing executes** in cmd.
- [ ] Codex, OpenCode, Aider, Goose, Copilot, Amazon Q, Open Interpreter (whichever available): upon receiving a question in a tab, do they prompt for approval to run `ask-agent`? Note which ones prompt.

## Terminal

- [ ] Paste (Ctrl+V) large multi-line text into agent terminal: treated as text, not separate commands.
- [ ] Paste (Ctrl+V) into shell with *bracketed paste* containing `ESC[201~`: arrives as paste, nothing executes.
- [ ] Large paste (~5 MB) into shell tab running `timeout /t 30` (not reading input): window remains responsive and other tabs work. When command finishes, text arrives intact.
- [ ] **Real IME**: Chinese/Japanese/Korean keyboard or `Win+.` emoji picker in terminal — composition appears underlined at cursor and committed text arrives in program.
- [ ] Portuguese accented characters in terminal.
- [ ] Enter on empty prompt in Claude/terminal: tab doesn't show "working" (halo stops in ~8 s, no bell).
- [ ] Ctrl+C with Russian/Greek keyboard layout interrupts terminal program.
- [ ] In background tab or different project, `printf '\e]52;c;%s\a' $(printf test|base64)` does not alter clipboard; in active tab, it does.
- [ ] In vim/htop with mouse enabled, Ctrl+wheel and Alt+wheel arrive with modifier. Shift+wheel scrolls terminal history, with or without mouse-aware program.
- [ ] Slowly dragging right panel border or splitter with Claude open: tab does not repaint during drag; on release, Claude repaints once at new width (history retains at most one old copy per drag).
- [ ] With Claude working, open third agent type (screen becomes 2+1) and drag splitters: Claude tab repaints promptly, without going black or freezing.
- [ ] **Blank lines after resize (t37)**: in PowerShell tab, run `1..3000 | % { "line $_ " * 12 }`, then resize window repeatedly (Focus, splitter, maximize), smaller and larger; scroll history to top: lines stay in order without holes, and nothing disappears when window grows (bottom may have trailing blank space: matches Windows Terminal behavior). Tab right-click: "Record terminal (diagnostics)" enables recording (menu item toggles to "Stop recording terminal" and Conversation indicates files location, `data/pty/`); "Save grid snapshot now" dumps screen and history text and opens folder. Without enabling, nothing is written to `data/pty/`.

## Interface

Right panel ★

- [x] Integrated Conversation appears on the **right**, occupying full height; terminal windows occupy full height in center.
- [x] Dragging left border resizes panel (between ~280 px and ~half window width; default is now 330 px, slightly narrower); **cannot** be collapsed.
- [x] Close and reopen app: chosen width persists.
- [x] Shrink window to minimum (~820 px width): nothing overlaps; terminal remains usable.
- [x] "+" menu (new tab) near right window edge: opens within bounds and selection works.
- [x] Whisper download progress (if active): progress banner sits directly below panel top.
- [x] Icons: **speech bubbles** (Conversation) and **branch** (Git). Hover shows name of each.
- [x] Icons render crisp (no strange jagged edges) and branch icon shows **open circles**.
- [x] Staying on Git while messages arrive: **counter badge** appears over Conversation icon; switching to Conversation clears it.
- [x] "Clear" only appears on Conversation.
- [ ] **Read-only Conversation** (t11): right panel has no input box or "Send to"; feed only. To write to an agent, use its tab terminal.

Conversation

- [ ] Messages sent to an agent via "Send to agent" (GitHub tab) enter feed with vertical green indicator (`theme::USER = #10b981`), readily distinguished from AI messages.
- [ ] Long and multi-line messages: auto-wrap in card without overflowing panel width and dynamic height on color bar.
- [ ] Renaming tab (claude → claude@dev) while "thinking" in Conversation: older messages show new name, and "thinking" indicator clears with its next message.

Documents in center ★ (t19)

- [ ] **The toggle**: top of sidebar displays **Agents** and **Documents 0** (active in agent color); collapsing sidebar (Ctrl+B) moves toggle to titlebar immediately after logo; with narrow sidebar (150 px) second option becomes "Docs"; "Documents" is dimmed and unclickable without open documents. Hover displays tooltip with shortcut.
- [ ] **Opening diff via task desk**: in review task, "View full diff". Center automatically switches to Documents with a `t1 · diff` tab; counter becomes "Documents (1)"; Merge, Return, Validate, and Reject buttons remain on right panel. Clicking "View full diff" again focuses tab (no duplicate opened). After merge, "View merged changes" opens another tab (`t1 · merge`).
- [ ] **Diff file list**: each file marked with status letter (A/M/D/R), name, and `+`/`−`; clicking file scrolls diff to it; selection highlights. In multi-file diff, verify list matches "N files" from detail.
- [ ] **Badge and hidden agents**: with terminals running, switch to Documents. Terminals do not disappear or restart: returning to Agents reveals everything written in the interim. When an agent finishes a turn while center is in Documents, "Agents" shows a counter badge (including for the active tab agent); if actively working, dot pulses. Returning to Agents clears badge for active tab.
- [ ] **Terminal does not duplicate conversation** (Claude Code, Windows): with Claude conversing, toggle Agents ↔ Documents multiple times, and resize window or right panel with Documents visible. Returning to Agents shows conversation is **not** duplicated in terminal history (PTY only resizes after window dimensions stabilize).
- [ ] **Shortcut Ctrl+Shift+D** toggles Agents ↔ Documents, whether terminal is focused or unfocused; with no open documents does nothing; with open modal (Ctrl+/) does nothing. With Documents in view, typed keys do not leak into hidden terminal (including `Win+.` emoji).
- [ ] **Open and close**: opening any document switches to Documents; clicking "Agents", an agent in sidebar, "Go to tab" in task desk, or Ctrl+T returns; closing last document (×) returns to Agents automatically. Switching projects shows documents of the other project (or Agents if it has none); switching back restores previous set.
- [ ] **Document tab strip**: with many documents, ‹ › scroll arrows and ▾ dropdown appear (matching agent tabs); Ctrl+Tab / Ctrl+Shift+Tab cycle through documents.
- [ ] **Double-click `.rs` file** in explorer: opens viewer (monospace, line numbers, two-way scrolling). **On `.md` file**: opens formatted (headings, lists, `- [ ]`, tables, blockquotes, code). Single click continues inserting `@file` into active tab. "Open in default application" opens file in Windows program.
- [ ] **File changes and reloads**: with file open, ask agent (or external editor) to modify it: view updates automatically within moments without flickering. Deleting file: tab displays error; recreating file: restores view. "Reload" re-reads file.
- [ ] **Large or binary files**: double-clicking `.exe`/`.png` or file over 1 MB displays explanation instead of content, without hanging.
- [ ] **Brief and report** (task desk): "Open in Documents" on task Brief and Report opens each formatted (`- [x]` renders as checked box, markdown table renders as table). Report button on task without report indicates this.
- [ ] **Git tab**: on diff of modified file and commit detail in History, "Open in Documents" opens diff read-only; staging hunks remains on right panel. With narrow panel (including default), button becomes an icon (box with arrow) next to + and ↶ with tooltip; widening panel restores text.
- [ ] **Minimum window width (~820 px)**: with Documents in view, nothing overflows center or titlebar (compact toggle fits next to logo with collapsed sidebar; agent usage shrinks); diff stacks file list on top of diff when center is narrow.

Documents: editor, syntax highlighting, and search ★ (t39)

- [ ] **Edit and save**: double-click project `.rs` file, **Edit**, type. Amber dot appears on tab replacing ×, title reads "● Unsaved"; **Ctrl+S** (or **Save**) writes file and dot disappears. Verify in Windows Explorer / `git diff` that only edited lines changed. Tab indents with 4 spaces, Shift+Tab dedents (entire selection when multi-line), Ctrl+Z undos.
- [ ] **Line endings and BOM**: file with CRLF (common on Windows) saves with CRLF; file with LF saves with LF; file with BOM keeps BOM; file without BOM does not gain one. Mixed CRLF/LF file or non-UTF-8 file (e.g. ANSI `.txt` with accents) opens read-only with explanatory banner (no Edit button).
- [ ] **Close with unsaved changes**: tab × prompts (Save / Don't save / Cancel) with expected behavior; **Reload** over unsaved changes confirms before discarding; closing app (window ×, Alt+F4) with unsaved changes prompts, and "Save all and exit" saves all files before closing.
- [ ] **Agent modifies open file**: with unsaved edits, ask agent to modify same file. Banner appears: "File changed on disk (an agent?)…" with **Keep mine** and **Reload from disk**; typed text remains intact until choice; Ctrl+S and Save are blocked until choice is made. "Keep mine" followed by save overwrites disk; "Reload from disk" loads agent version. Without unsaved edits, file updates automatically. Deleting file with unsaved edits: warning shown, edits preserved, Ctrl+S recreates it.
- [ ] **Syntax highlighting**: open files of various types (Rust, Markdown, TOML, JSON, YAML, JS/TS, Python, PowerShell, shell): keywords, strings, numbers, comments, types, and brackets colored; typing remains responsive on ~250 KB file (editor limit); larger file (or line exceeding 4,000 characters like minified JSON) opens read-only with banner and scrolls smoothly. In diff, code uses file language colors, and multiline `/* … */` opened on one side does not color the other.
- [ ] **Search (Ctrl+F)**: in file, large file, and diff: field gains focus, matches highlighted (current match emphasized), "n of m", Enter/Shift+Enter and ‹ › navigate and scroll to match, Esc closes. In formatted `.md`, Ctrl+F reveals raw code.
- [ ] **Select and copy**: drag in file selects (double-click word) and Ctrl+C copies; in diff and large file, dragging selects (scrolling automatically when passing top/bottom edge), Ctrl+A selects all, and Ctrl+C copies code only without line numbers or `+`/`−`; pasting into external editor preserves tabs.

Split screen ★

- [x] "+" in Claude header: 2nd Claude tab opens in **same pane** (does not split again).
- [x] Open Codex (or Terminal) via topbar "+": view splits into **2 on top + 1 wide on bottom**. With 4th type, 2×2 grid.
- [x] `claude@dev` stays in **same pane** as Claude, in another tab.
- [x] Clicking a pane: gains **agent-colored outline** and keyboard focus.
- [x] Mouse wheel scrolls pane under cursor even without focus.
- [x] Ctrl+Tab switches tabs **within** focused pane only; Ctrl+Alt+← → ↑ ↓ shifts focus to neighboring pane (note if graphics driver intercepts shortcut to rotate display).
- [x] Dragging splitter between panes resizes; double-click equalizes; persists across restarts.
- [x] Dragging handle **⋮⋮** of pane onto another: displays "Drop to swap positions" and swaps on drop. Order persists across restarts.
- [x] Closing last tab of a type: pane closes and remaining panes expand. Closing tab with others of same type: focus remains in same pane.
- [x] Agent invoking another not currently open (`ask-agent codex "hi"`): Codex pane appears **without stealing focus** from active typing area. If Codex exists, adds tab to its pane.
- [x] **Split screen** button in topbar: toggled off, returns to single window with all tabs; toggled on, restores split layout.
- [x] Minimum window (~820 px) with 3 or more types: panes stack; overflow becomes tabs in last pane without loss.
- [ ] With Git diff open, panel **maintains width** (content scrolls internally) and panes **do not resize**.
- [x] With 4–6 panes and agents responding concurrently: app remains fluid (note if freezing occurs).
- [ ] **Highlight mode** (t28), with 3 open CLIs (Claude, Antigravity, Codex): in ORGANIZATION, "Highlight" places selected agent pane taking 2/3 of center on left, with remaining two stacked on right at equal height with 6 px gap.
- [ ] **Highlight mode: changing the highlighted agent** (t28): clicking terminal or header of smaller pane, clicking another agent in sidebar, Ctrl+Tab/shortcut to another CLI, and "Bring to highlight" button (expand icon left of microphone) bring its CLI to large pane. Two agents of same CLI stay as tabs in same pane. After switch, Claude Code does not duplicate conversation (PTY resizes once after stabilization).
- [ ] **Highlight mode: splitter** (t28): dragging gap between large pane and column resizes width (both maintain ~260 px minimum); double-click resets to 2/3; proportion persists across restarts.
- [ ] **Highlight mode: narrow constraints** (t28): shrink app window until only one column fits (<~526 px center): keeps only large pane with other CLIs as tabs. Reduce height until small panes cannot fit (140 px each): overflow becomes tabs in last small pane. With single CLI, pane occupies entire center without halo.
- [ ] **Active pane highlight** (t28), in Split and Focus: active tab pane has 1 px border and 3 px halo in agent color; others dimmed (62%) and restore to 100% on mouse hover (continuing live updates). In Tabs mode nothing changes.
- [ ] **Migration** (t28): config with `"splitEnabled": false` opens in Tabs, and `true` in Split; new choice saved in `organization`.
- [ ] Split screen: drag splitter, shrink window until grid reformats, and widen again. Dragged proportions restore, including after app restart.
- [ ] **Hidden tabs: gradient and arrows** (t12): in pane with more tabs than fit (narrow window or open multiple tabs of same type), side with hidden tab shows short gradient and arrow ‹ or ›. Clicking arrow scrolls strip to nearest hidden tab; arrow tooltip names it.
- [ ] **Hidden tabs: nothing shown when all visible** (t12): with all tabs in view, no arrows or gradient; widening window removes them.
- [ ] **Hidden tabs: ▾ menu** (t12): with scrolling strip, ▾ button appears at end; opens menu listing all pane tabs and clicking one reveals it (strip scrolls to it). Scrolling away from visible tab does not force it back into view every frame; only when switching tabs.

Dictation and attachments ★

- [ ] **Optional dictation: clean initial launch** (t34): in data folder without `whisper/models/` and without `config.json` (use fresh `AGENTDECK_HOME`; executable's sibling `whisper/` must not have model), app launches normally and shows modal "Voice dictation (optional)" with download size (~1.6 GB). Nothing downloaded before user answers (right panel shows no progress).
- [ ] **Optional dictation: "Not now"** (t34): modal closes (Esc also closes), nothing downloads, `config.json` receives `"dictation": "no"`, and on restart prompt does not reappear. Window microphone (and `Ctrl+Shift+M`) opens same modal instead of recording; microphone tooltip reads "Voice dictation not installed: click to download".
- [ ] **Optional dictation: download** (t34): "Download" in modal (or later via ☰ Settings → "Download (~1.6 GB)") saves `"dictation": "yes"`, displays progress banner on right panel, and when finished status in Settings becomes "installed"; microphone records and transcribes as before. During download microphone informs in Conversation that download is in progress (and button tooltip says same).
- [ ] **Optional dictation: remove** (t34): ☰ Settings → "Remove from disk" asks confirmation ("Confirm: delete from disk"; closing menu cancels confirmation); confirming deletes `whisper/models/ggml-large-v3-turbo.bin` (and `bin-cuda/`), status becomes "not installed", config saves `"no"`, and microphone offers download again. While dictation is actively recording, removal is rejected with warning.
- [ ] **Optional dictation: existing Whisper installations** (t34): with model already downloaded and config lacking `dictation` field (upgrading users), app prompts nothing and dictation functions as before; `config.json` acquires `"dictation": "yes"`.
- [ ] **Optional dictation: screenshot mode** (t34): `AGENTDECK_SMOKE` continues without downloading or prompting; `AGENTDECK_DEBUG_UI=dictation-ask` displays modal to inspect UI.
- [ ] **Tab-scoped dictation (`Ctrl+Shift+M` or microphone icon on tab)**: clicking tab microphone records (blinks red), clicking again transcribes ("Transcribing…" on tab) and types text into PTY of that tab **without Enter**; switching tabs during recording/transcription keeps target on starting tab; closing tab before completion discards text with notice in Conversation.
- [ ] **Microphone and attachment in pane header** (t11): each pane has microphone and file attachment buttons next to "+"; both operate on the **visible tab of that pane** (not focused pane or first pane): with two panes, recording in second types into visible tab of second; changing visible tab updates target. Button is disabled when visible tab is dead (or while transcribing) and tab shows only indicator (pulsing red dot, "Opening…" or "Transcribing…").
- [ ] **Attach via header** (t11): attachment button copies file to `data/attachments` and types `@path` in Claude (raw path in other CLIs) into visible tab **without Enter**; duplicate filename does not overwrite existing file.
- [ ] **Drag file over window** (t11): dragging file from Explorer darkens pane under cursor with "Drop to attach to <tab>" and highlight tracks cursor across panes; dropping types path into visible tab of **that** pane (without Enter). Pasting image (Ctrl+V) attaches to active tab.
- [ ] Microphone: window does not freeze while opening, second click cancels. Recording over 5 min stops automatically with notice and transcribes.
- [ ] Whisper download: disconnect Wi-Fi mid-download. Retry appears in ~30 s without hanging for hours. With active antivirus, 1.6 GB download finishes without rename errors.
- [ ] Whisper CUDA: kill app during extraction. On next launch, no partial `bin-cuda` exists, `bin-cuda.staging` is cleaned up, and `.zip` in `whisper\` is reused.
- [ ] Dictation twice in rapid succession: no leftover `audio-*.wav` in `data\attachments`. Closing app during transcription leaves no running `whisper-cli.exe`.

## Tabs and shortcuts

- [ ] Double-click tab title or right-click → "Rename".
- [ ] Enter new role/name (e.g. `dev`, `dev@ana` or `claude@dev@ana`).
- [ ] Empty name: restores original model name without breaking.
- [ ] Duplicate name: modal rejects with red warning.
- [ ] Name with accents (e.g. non-ASCII) or special characters (`!`, spaces): rejected friendly with expected format message (`model@role@name`).
- [ ] Persistence: new name persists across restarts (`config.custom_names`) and propagates in feed and `ask-agent`.
- [ ] With open modal (rename tab or project, Ctrl+/), clicking tabs, sidebar, or terminal does nothing, and typed keys do not leak to terminal. Esc closes and restores focus.
- [ ] Ctrl+/ with focused terminal toggles shortcuts screen.
- [ ] Focused shell: Ctrl+L clears screen, Ctrl+B reaches program (tmux), and Ctrl+T opens tab. With "App shortcuts take precedence over terminal" enabled, Ctrl+B collapses sidebar. Setting persists across restarts.

## Titlebar, agent usage, and sidebar ★

- [x] Window no longer has Windows titlebar: at top, ☰, panel toggle button (collapses/expands sidebar), "AgentDeck" (no logo), and +; on right, smaller minimize, maximize/restore, and close buttons (close turns red on hover).
- [ ] Dragging titlebar moves window; double-click toggles maximize/restore; dragging to screen edge snaps; Win+arrows work.
- [ ] Resizing from 4 edges and 4 corners (cursor changes); when maximized, borders are inactive and window does not cover taskbar. On Linux, verify on X11 and Wayland.
- [ ] Closing via × preserves window size as before.
- [ ] ☰ opens settings aligned to left.
- [ ] Usage: each agent with quota displays logo, thin session bar (top), and thick weekly bar (bottom) with percentages; hover opens usage panel (all agents, "read ago", window renewal times, "locked until" when session exhausted) and stays open while mouse is over item or panel.
- [ ] Usage without open window: with no Antigravity tab (installed and logged in), its usage appears within ~3 min. Restarting app preserves last Claude usage (`data/usage.json`), with "read ago" in panel.
- [ ] Open Antigravity: usage appears within ~3 min; without login or network, dashed ring and "usage unavailable".
  - Verified: with login, Antigravity usage appears. Remaining: case without login/network.
- [ ] Demo: `AGENTDECK_DEBUG_UI=usage-demo` (Claude 34%/12%, Antigravity with weekly warning, Codex with exhausted session) and `usage-panel` (same with panel open).
- [ ] Explorer without repeated root row; "Refresh" re-reads tree and "Collapse all" closes folders.
- [x] **ORGANIZATION** section in sidebar: **Tabs | Split | Focus** control selects window layout (Tabs = single pane).
- [ ] Usage: once session renewal time passes without new reading, session bar shows faded 0% and panel reads "renewed · awaiting new reading".
- [ ] Antigravity with expired token in Credential Manager and valid token in `~\.gemini\oauth_creds.json`: quota appears. Without login, circle reappears within 20 s after login.
- [ ] Codex: open new tab (rollout without `rate_limits`): prior session usage remains visible.

## Projects ★

- [x] **Migration:** first launch with legacy config converts each folder in list to project; open tabs reopen in active folder project.
- [ ] "+" in PROJECTS section: selecting folder creates project (name = folder) and activates it; center displays "No agents open".
- [ ] Open Claude in new project and click previous project: its terminals restore; Claude in new project **keeps running** (project displays logo and count).
- [x] Active project lists its tabs; clicking activates tab. Other projects display agent logos and count.
- [ ] Each terminal runs in its project folder (`pwd`/`Get-Location`), and tree and Git tab update with project.
- [ ] Conversation displays active project only; conversation between agents in another project appears when brought back to screen.
- [ ] `ask-agent claude@dev "hi"` in a project opens `claude@dev` in **that** project (hidden if not current); other project does not see instance.
- [ ] Two projects with a `claude` each: each responds only to its project.
- [ ] Restart app: only active project tabs open; other project tabs open when clicking project (nothing lost).
- [ ] Right-click → Rename (name changes, folder does not), Open folder, Remove (confirms, closes project tabs; folder remains).
- [ ] Removing last project: center prompts "Open folder…".
- [ ] Screenshot: `AGENTDECK_DEBUG_UI=project:<id>` switches to project `<id>` on launch.
- [ ] Collapsible `PROJECTS` section at top of file tree, with indicator dot on active project.
- [ ] `+` button: folder picker adds new workspace to list and activates it.
- [ ] `×` button (appears on hover with 2+ projects): visible under cursor, click removes project from list; if active, automatically switches to first in list.
- [ ] Non-existent or inaccessible folder: does not crash application; displays friendly error in conversation.
- [ ] Project list persists in config file (`config.projectList`).
- [ ] Right-click project → Remove: window continues rendering with prompt open. "Yes" removes, "No" keeps.
- [ ] Projects: remove project with ongoing question between its tabs. Caller receives error, nothing reappears in removed project.
- [ ] Open project via path with different drive letter casing (`c:\...`): tree and Git update.

## Maximized window (t30)

- [ ] Middle button on titlebar: maximizes; glyph becomes "Restore" (two squares) and tooltip reads "Restore"; clicking again restores previous size.
- [ ] Double-click titlebar toggles maximize/restore; Win+↑ and Win+↓ as well, glyph updates accordingly.
- [ ] Maximized: cursor on edges and corners does not become resize arrow; restored: resizes normally.
- [ ] Close maximized and reopen: opens maximized; upon restore, window has size from before maximizing (not full screen).
- [ ] Resize to arbitrary size, maximize, close and reopen: restore returns to that arbitrary size.

## Git and GitHub ★

Git — with working folder in a git repository:

- [ ] Redundant branch button **removed from topbar**. Branch status and change count appear at top of Git tab (`dev · N ▾`), in Git icon tooltip, and in icon changes badge.
- [ ] Clean topbar (no redundant branch chip); detached HEAD displays `{hash} (detached) · N ▾`.
- [ ] Git tab lists changes in sections; status letters colored (M amber, A green, D red…).
- [ ] Clicking file opens **diff** (spanning panel width with horizontal scrolling), numbered lines and syntax colors; **←** returns.
- [ ] Stage (+) and unstage (−) file and "all" via section buttons.
- [ ] Discard (↶) prompts **confirmation**; on untracked file, warns it will **delete**.
- [ ] Commit with message (button and **Ctrl+Enter**); "Amend last commit" populates previous message.
- [ ] Commit without configured `user.name`/`user.email`: explains how to configure them.
- [ ] Branch menu: search, switch branch, create new branch (enter name + Enter).
- [ ] Detached HEAD: branch button displays `{hash} (detached)` with change count.
- [ ] **Push** to GitHub repository over HTTPS: prompts **Git Credential Manager** login and completes push. Without credentials: error message **without hanging**.
- [ ] New branch: button becomes **Publish** and sets upstream.
- [ ] Fetch (⟳) and Pull (↓n) after commit made elsewhere.
- [ ] Make commit **via terminal** (or an agent makes one): Git tab refreshes automatically.
- [ ] Paths with **accents and spaces** display properly and diff opens.
- [ ] Folder that is not a repository: displays "Initialize repository" and works. Repository without git: displays explanatory notice and initialize button.
- [ ] (If possible) git not installed / outside PATH: displays message with installation link.
- [ ] Note: folder `AGENTS.md` appears as modified (known limitation, tracked in pendencias).
- [ ] Git: open "Discard" on file in project A and switch to project B via sidebar: confirmation closes and nothing discarded in B.
- [ ] Git: in branch list, click remote branch (`origin/x`) whose local already exists: switches to local without error.
- [ ] Project that is a git worktree (`git worktree add`) or subfolder of repository: commit, `git cherry-pick`/`git revert` conflict, and `git rebase -i` via terminal update Git tab without reloading project.
- [x] Repository with `core.autocrlf=true` and `AGENTS.md` in CRLF: opening app leaves file clean in `git status`.
  - Verified on `commanddeck-dev`: content matches HEAD hash. The `M` previously appearing was stale index; clean `git add AGENTS.md` cleared it.

Git — hunks, history, stash, merge/rebase, and conflicts:

- [ ] Diff of file with two modified regions: "Stage" on one hunk leaves file in both sections (staged and unstaged); "Unstage" returns it; "Discard" on other hunk prompts confirmation and keeps first. Buttons do not overlap `@@` header text.
- [ ] Same hunk test on file with CRLF line endings (`autocrlf=false`).
- [ ] Branch menu, "Merge" branch with conflict: displays "Merge · N conflict(s)" banner and "Conflicts" tab. "Continue" remains disabled until all resolved; "Abort" prompts confirmation and restores state.
- [ ] Conflicts: "Ours" on one block and "Theirs" on another; "Mark as resolved" only enables afterwards; "Continue" completes merge; "Open" opens file in default application.
- [ ] Same workflow with "Rebase" (Continue and Abort).
- [ ] Conflict "we deleted / they modified": "Ours" or "Theirs" on entire file resolves; ↶ in Conflicts section discards with confirmation.
- [ ] History: list with author and date; "Load more" fetches next 50; click opens detail (message, files, diff); after new commit list re-reads.
- [ ] Stash: "Stash" with message and "Include untracked" clears files from working copy (new file disappears and returns on Apply). "Apply and drop" removes from list; "Drop" prompts confirmation.
- [ ] gpgsign: with `commit.gpgsign=true` and locked key, commit fails in under ~5 s with explanation, without password dialog. After `echo test | gpg --clearsign` in a tab, commit succeeds.
- [ ] Push/pull over SSH with password-protected key outside ssh-agent: "authentication failure" with step-by-step ssh-agent instructions.
- [ ] Discard (↶) untracked folder containing `.git`: deletes folder.

GitHub:

- [ ] GitHub tab (4th icon on right panel) in project with GitHub `origin`: displays `owner/repo`, green dot (GCM login), and PRs / Issues / Actions lists with counts; icon badge indicates count of open PRs.
- [ ] Project without `origin` or with remote outside GitHub: tab states "No GitHub repository" with explanation; "Try again" works after adding remote.
- [ ] Clicking PR opens detail below (checks/jobs). With failing job, tail of log opens automatically, scrolls two-way without widening panel, and highlights error lines. "Log", "Fix ▾", and job ↗ work; X returns to list.
- [ ] "Refresh" (or reopening tab after 60 s) re-reads data. Without network, previous list remains and red dismissible error banner appears.
- [ ] Without login: banner "Not logged in (60 per hour)". In private repository, 404 message explaining how to log in. Job log prompts login.
- [ ] "Open in browser" (PR, issue, run, job, repository) opens correct URL.
- [ ] "Send to agent ▾" on PR, issue, or failing job: choosing open tab types prepared message when agent becomes free (not mid-typing); choosing model + role (e.g. claude@review) opens tab if absent and delivers; message appears in Integrated Conversation.
- [ ] Switching projects with tab open clears list and reads new project repository.
- [ ] Right panel at minimum (280 px): nothing clips beyond ellipsis; low window height does not crash app.

## Tray and second instance ★

Windows, with app open and an agent tab:

- [ ] Click × (or Alt+F4): window hides without confirmation and AgentDeck icon sits in system tray; `AgentDeck` process continues in Task Manager and tab CLIs keep running.
- [ ] Before ×, request long turn in a tab (e.g. "count to 200 slowly with sleep"): with window hidden turn completes normally; upon reopening response is present and terminal is correct size (no 0×0 or duplicate lines).
- [ ] Left-click icon: window restores and focuses. Right-click: menu with **Open** and **Quit**; Open does the same.
- [ ] Minimize (titlebar button) then **Open** from icon: restores and focuses.
- [ ] With window hidden (or minimized), launch `agentdeck.exe` again: no second process or error message; original window comes to front (does not just flash on taskbar).
- [ ] **Quit** with nothing pending: closes everything (icon vanishes, process exits, tab CLIs terminate).
- [ ] **Quit** with unsaved edited file: window appears prompting "There are unsaved changes".
- [ ] **Quit** with agent mid-turn: window appears with "Agents are currently working" (Cancel keeps app running).
- [ ] Linux with tray (KDE, or GNOME with AppIndicator): same test script with `ksni`. Without `StatusNotifierWatcher` (pure GNOME): × **minimizes**, and app is never left invisible without an icon.

## Name migration (CommandDeck → AgentDeck) ★

With data directory and config from CommandDeck users (`%APPDATA%\CommandDeck\config.json`), without `AGENTDECK_HOME`:

- [ ] Launch `agentdeck.exe`: window is named AgentDeck, projects and open tabs from legacy version appear, and `%APPDATA%\AgentDeck\config.json` was created (`CommandDeck\config.json` remains intact).
- [ ] Existing tasks (`data/tasks/`) and history continue on task desk.
- [ ] In new agent tab, `ask-agent --list` works and `echo %AGENTDECK_PORT%` (or `$env:AGENTDECK_PORT`) has a value.

## Final checklist

- [ ] Take a screenshot of main screen (with right panel) to replace `docs/screenshots/main.png`.

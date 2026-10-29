# History

What has already been done, newest to oldest. It is the destination of every completed item from
[backlog.md](backlog.md): one line per item (rules in [OVERVIEW.md](OVERVIEW.md)); technical details remain in the commit and
current state in [ARCHITECTURE.md](ARCHITECTURE.md).

## Completed

- [x] **Flaky Fedora test: data folder lock released on `shutdown`** (2026-10-25): a live `Core` copy in a thread held `data/.agentdeck.lock` and reopening was refused; `Core::shutdown` now releases the lock.
- [x] **`crates/app/src/deck.rs` split into modules** (2026-10-25, `t54`): the ~2,900-line file became `crates/app/src/deck/` (`mod`, `frame`, `tabs`, `keyboard`, `sidebar`, `projects`, `feed`, `attachments`, `dictation`), without changing behavior.
- [x] **Codex on Windows: pasted message no longer stalls in input field** (2026-10-25): Enter waits 1 ms longer per pasted character (10 s ceiling), preventing it from becoming a newline before Codex reads the entire paste.
- [x] **`tasks.rs` split into modules** (2026-10-25, `t53`): `crates/core/src/tasks.rs` became `crates/core/src/tasks/` (`mod`, `text`, `store`, `lifecycle`, `merge`, `finish`, `git_ops`, `stash`), without changing the API.
- [x] **Demo scenarios separated from `app.rs`** (2026-10-25, `t55`): `debug_ui::apply` and fictional capture states moved to `crates/app/src/debug_ui.rs`.
- [x] **No "Do you trust this folder?" prompt in tabs** (2026-10-25): before opening a tab, the app registers folder trust in Claude, Codex, and Antigravity (`trust.rs`), without hanging background tabs.
- [x] **Desk chooses by model capability, then by quota** (2026-10-24, `t52`): task level (`simple`, `medium`, `complex`), benchmark table + completed task history, and selection of the capable model with the most headroom.
- [x] **Highlight: consecutive tab switches do not accumulate resizes** (2026-10-24, `t51`): 600 ms debounce on highlight switch, cancellation when returning to the same size, and waiting for repaint to finish before the next resize.
- [x] **Re-added project reuses id** (2026-10-24, `t50`): removing a project stores id and folder in `removedProjects`, and re-adding the same folder recovers tasks in `data/tasks/<id>.json`.
- [x] **Task agent choice by quota** (2026-10-24, `t49`): `--task new` without `--agent` and the form recommend the official agent with the lowest quota used, and `ask-agent` warns when the destination exceeds 80%.
- [x] **Write scope per task, with queue** (2026-10-24, `t45`): tasks declare paths (`--scope`); if they overlap another running or in-review task, they enter `In queue` and start automatically when the previous one finishes.
- [x] **Bridge tests also on Windows** (2026-10-24, `t42`): the 17 tests in `tests/bridge.rs` now run on Windows using the `fake_agent` test binary instead of shell scripts.
- [x] **Antigravity without `AGENTS.md` created in repository** (2026-10-24, leftover from `t38`): the app writes `data/agy/<instance>/AGENTS.md` and passes the folder in `--add-dir`; only Goose and Copilot still create `AGENTS.md` in the project.
- [x] **Worktree not removed: log shows who holds it** (2026-10-24): `file_lock.rs` lists in the log and task warning remaining files and, on Windows, processes holding them open (Restart Manager).
- [x] **`config.json` edited in PowerShell** (2026-10-24): configuration loading accepts UTF-8 with BOM and UTF-16, preserving projects when the file is saved by PowerShell 5.1.
- [x] **`main` became the Rust rewrite** (2026-10-24): the old TypeScript `main` branch was replaced by `dev` in Rust, with green CI on Windows, Ubuntu, and Fedora.
- [x] **Quota per model for agents** (2026-10-24): `/ask` rejects with 409 exhausted models; `--models` orders agents by headroom and shows quota per group, and `--team`/`--list` warn next to the model.
- [x] **Merge with stash and dictation: three review findings from `t35` and `t34`** (2026-10-23): warning of where stash stayed on conflict, return of stash by SHA hash, and clear warning in non-audio builds.
- [x] **Memory, phase 2: `ask-agent --outline <file>…`** (2026-10-23, `t40`): returns file outline (signatures and line numbers, without bodies, in Rust and Markdown) for agents to read only the necessary snippet.
- [x] **Memory, phase 2: current situation in any tab and file-in-use warning** (2026-10-23, `t41`): `ask-agent --learn` without arguments includes running tasks, and with files warns if another task touches them.
- [x] **Editor: three review findings from `t39`** (2026-10-23): closed search does not highlight lines in large files, save failure on exit explains reason, and reload/discard adopts disk line endings and BOM.
- [x] **Removing a project warns of unsaved edits** (2026-10-23, leftover from `t39`): project removal confirmation counts documents with unsaved changes and warns before discarding them.
- [x] **File editor in Documents mode, with syntax highlighting** (2026-10-23, `t39`/`t36`): editing files up to 256 KB with `Ctrl+S` (preserving CRLF/LF and BOM), highlighting in 9 languages, search (`Ctrl+F`), selection in diff, and external change notice.
- [x] **Optional voice dictation** (2026-10-23, `t34`): the app prompts before downloading Whisper (~1.6 GB) on first launch or microphone use, and allows downloading later or removing in ☰ Settings.
- [x] **Antigravity quota on Linux and WSLg via X11** (2026-10-23, `t33`): `agy` OAuth token reading from Secret Service (D-Bus) and file on Linux, and window forced to open via X11 under WSLg.
- [x] **Merge: resolve refusal right there in the window** (2026-10-23, `t35`): merge refusal offers buttons to stash local changes and merge, view dirt, or return to agent to commit/resolve conflict.
- [x] **AgentDeck instructions in every tab, no hidden subagents** (2026-10-23, `t38`): each CLI receives instructions through its own channel (without depending on prior `AGENTS.md`), forbidding CLI-internal subagents by default.
- [x] **Legacy CommandDeck names removed** (2026-10-23, `t32`): removed `COMMANDDECK_*`, `x-commanddeck-token`, `commanddeck:` marker, and `.commanddeck.lock`; only initial `config.json` migration remains.
- [x] **Dedicated Ollama agent removed** (2026-10-23, `t32`): removed `ollama-agent` binary, `core::ollama` module, and `ollama` agent from menus, bridge, and configuration.
- [x] **Terminal: lines disappeared and went blank when resizing window (Windows)** (2026-10-23, `t37`): `terminal/resize.rs` undoes history pull of `alacritty_terminal` to match ConPTY repaint, plus diagnostic recording.
- [x] **Ultrareview findings** (2026-10-23): delivery confirmation in `POST /inbox/delivered` after printing, inbox preserved on tab rename, and backslash kept as filename character on Unix.
- [x] **`/code-review` findings** (2026-10-23): minimized window does not overwrite maximized state, refusal opens task detail from any screen, quota renewal redraws topbar, and `agy --version` only runs when reading quota.
- [x] **Log without `ureq` noise** (2026-10-23, `t31` finding): log level for `ureq`/`ureq_proto` set to `WARN`, avoiding logging headers and IPs in `agentdeck.log`.
- [x] **Action refusal from list card now shown** (2026-10-23): a refused action ("Merge"/"Validate") clicked directly on the card opens task detail to display the red error banner.
- [x] **Whisper binaries pruned** (2026-10-23, `t31` finding): removed from git 25 example/test executables and unused DLLs from `whisper/bin-cpu/Release/`, keeping only `whisper-cli.exe` and its DLLs.
- [x] **Maximized window restores to windowed mode** (2026-10-22, `t30`): populated `ViewportInfo` before each frame to restore maximized window, toggle button icon, and preserve normal size in `Config`.
- [x] **Validation before `main`** (2026-10-22, `t31`): verified README, CONTRIBUTING, Whisper third-party licenses, `.gitignore` for `data/` and `/.agentdeck/`, and clean initial launch without configuration.
- [x] **Verification of Linux support (Ubuntu and Fedora)** (2026-10-22, `t29`): audit in `LINUX.md`, separator fix in `read_dir`, fallback to `~/.local/share/AgentDeck`, Fedora fonts, and `fedora` CI job without GTK3.
- [x] **Highlight mode in ORGANIZATION and active agent highlighting** (2026-10-22, `t28`): third mode (`Tabs | Split | Highlight`) with active pane at 2/3 on left and others stacked on right, plus colored halo and fading.
- [x] **Effort per task** (2026-10-22, `t25`): effort selector (`Default | Low | Medium | High`) in the form and `--effort` in CLI, translated as `--effort` in Claude and model suffix in Antigravity.
- [x] **Lost reply: disk log and `ask-agent --inbox`** (2026-10-22, `t27`): rotating log in `data/agentdeck.log`, inbox per instance (`--inbox [--all]`), and notice in Conversation when terminal delivery fails.
- [x] **Cleanups: Ctrl+L hint, statusline padding, and demo models** (2026-10-22, `t26`): removed leftovers from old composer, preserved user Claude statusline `padding`, and updated demo models.
- [x] **Desk: merge with app's `AGENTS.md` and dead-end-free merge failure** (2026-10-22, `t24`): merge ignores and restores app block in `AGENTS.md`, and external failures leave task in review (or allow returning).
- [x] **Visual C, part 3: right sidebar, menus, and modals** (2026-10-21, `t23`): pill tabs in right sidebar, 3-line active task cards with progress bar, history filter, translucent Conversation, and opaque modals.
- [x] **Visual C, part 2: sidebar and center** (2026-10-21, `t22`): `Agents | Documents` toggle at top of sidebar, compact agents with model tag, explorer with Git letter, and pill tab strip.
- [x] **Visual C, part 1: the foundation** (2026-10-21, `t21`): semantic palette `theme::pal()`, active agent accent color, embedded Inter and IBM Plex Mono fonts, glass islands, new title bar, and app icon.
- [x] **Rename to AgentDeck** (2026-10-21, `t20`): crates, `agentdeck.exe` binary, `AGENTDECK_*` variables, `x-agentdeck-token` header, `AGENTS.md` markers, and automatic CommandDeck configuration migration.
- [x] **Documents in center: Agents | Documents toggle** (2026-10-20, `t19`): central view to read diffs, files, and markdown (`Ctrl+Shift+D`) with activity badge on agents and hidden terminals running without resizing.
- [x] **Desk without dead ends after merge** (2026-10-20, `t18`): recording `merge_commit`, "View merged diff" button, `--task done` accepted in validation when new commits exist, and `OPENCODE_CONFIG_CONTENT` isolation.
- [x] **Memory: learnings and uncommitted files on return and rejection** (2026-10-20, `t16`): feedbacks deliver learnings for newly cited files, and Current situation includes uncommitted files from worktrees.
- [x] **Memory: `--learn` without arguments and in bridge instructions** (2026-10-20, `t15`): `ask-agent --learn` without files shows global learnings and index, and instructions mandate checking it before editing.
- [x] **Agents and models inventory** (2026-10-20, `t14`): `ask-agent --models [<agent>]` lists installed agents and their models (free marked and first; OpenCode grouped by provider).
- [x] **Two-layer memory, phase 1** (2026-10-19, `t13`): learnings in `memory/learnings/*.md` with `touches` globs (`--learn` and task start) plus "Current situation" (other tasks, base commits, and feedback history).
- [x] **Hidden tabs: indicator and access** (2026-10-19, `t12`): when tabs do not fit in the window, the strip gains gradient, `‹`/`›` arrows to scroll to hidden tabs, and `▾` menu with all tabs.
- [x] **Read-only Conversation, mic and attachment in pane header, and attachments in "New task"** (2026-10-19, `t11`): removed Conversation composer; mic and attachment moved to each agent pane header, and "New task" accepts attachments.
- [x] **Tasks desk: "Waiting for validation" stage** (2026-10-18, `t10`): merge keeps tab, worktree, and branch alive in `Waiting for validation` until user clicks `Validate` (cleans up) or `Reject` (returns to same agent).
- [x] **Tasks desk: agent does not start on its own** (2026-10-18, `t6`/`t10`): explicit start instruction at end of brief and initial brief delivery via file on command line when opening tab (`claude`, `agy -i`, `opencode --prompt`).
- [x] **Dictation bound to each CLI tab** (2026-10-18, `t5`): microphone (`Ctrl+Shift+M`) records, transcribes with local Whisper, and pastes text into tab where recording started, without sending Enter.
- [x] **OpenCode: choose provider before model** (2026-10-18, `t3`): model menus group OpenCode by provider (`opencode`, `opencode-go`…), highlighting and listing `-free` models first.
- [x] **Task rules: task agent does not ask for merge** (2026-10-17, `t2`): initial task instruction forbids agent from asking for or running `--task merge`, concluding its work with `--task done`.
- [x] **Agent map removed** (2026-10-17, `t1`): removed Map tab from right sidebar and `agent_map` module, already covered by agent tree in sidebar.
- [x] **Tasks desk, phase B: UI** (2026-10-17, `feat/mesa-ui`): Tasks tab in right sidebar with per-state list, "+ New" form, detail view with brief/report/diff, and actions to start, review, merge, return, and discard.
- [x] **Tasks desk, phase A: core, bridge, and CLI** (2026-10-17, `feat/mesa-tarefas`): task lifecycle with dedicated worktree and branch in `.agentdeck/worktrees/`, storage in `data/tasks/`, and `ask-agent --task` commands.
- [x] **LLM model per instance** (2026-10-17, `feat/modelo-por-agente`): model choice per tab (`ask-agent --model`, "+ New agent" menu, and `roleModels`), 409 conflict if already open with another model, and display in UI and bridge.
- [x] **Review of `core/github.rs` by free OpenCode models** (2026-10-16): remote credential protected in error messages, API quotas separated per login, and fix for 401 test on Windows.
- [x] **GitHub integration** (2026-10-16, `claude/github`): GitHub tab with PRs and checks, issues, Actions, and tail of failed job logs, authentication via Git Credential Manager/`gh`, and "Send to agent…" button.
- [x] **Complete Git and git audit bugs** (2026-10-16, `claude/git`): stash, stage/discard per hunk, paginated history, merge/rebase via branches menu, conflicts tab, and clear warnings for GPG and SSH.
- [x] **Screenshot without Whisper download** (2026-10-16): `COMMANDDECK_SMOKE` no longer triggers automatic 1.6 GB Whisper download in temp folders.
- [x] **Audit: workspace, download, Whisper, and usage** (2026-10-16, `claude/infra`): watcher on real git dir, read timeout on download, atomic Whisper extraction, and improvements to Claude, Codex, and Antigravity quota reading.
- [x] **Audit: UI** (2026-10-16, `fix/interface`): split screen ratios preserved on narrow window, modals blocking background, microphone opened off UI thread, and `crash.log` rotation.
- [x] **Audit: bridge and team** (2026-10-16, `fix/ponte`): questions tracked by number even when renaming tab, cleanup on project removal, team change notice attached to typing, and 64 MB budget on bridge.
- [x] **Audit: terminal and processes** (2026-10-16, `fix/terminal`): per-tab `pty-write` write queue without freezing UI, `cmd_safe_arg` escaping for `.cmd` shims, OSC 52 restricted to active tab, and Job Object on Windows.
- [x] **Terminal froze when resizing panes** (2026-10-16): `Emulator::expire_sync` enforces 150 ms deadline for synchronized update (`ESC[?2026h`), avoiding frozen screen when end sequence is lost during redraw.
- [x] **Verified in use** (2026-10-15 and 16): validated by user and live across bridge projects, split screen, title bar with quotas, questions/replies (synchronous and 202), parallel roles, and accented characters.
- [x] **2026-10-15 audit (regression + fixes)**: atomic config write, Git state isolated per project generation, tab queue without freezing on empty Enter, ESC filter on paste, and path hardening.
- [x] **Reorganizable agent team known to agents** (2026-10-15): draggable lead/crew hierarchy in sidebar (`team.rs`), `ask-agent --team` and `--boss`, and automatic notice when position changes.
- [x] **Agents from all projects in a tree with logos** (2026-10-15): per-project agent tree in sidebar with model logo, active/completed turn indicator, and project switch on clicking an agent.
- [x] **New agent inside project** (2026-10-15): the "+" button moved from title bar to "+ New agent" row inside active project in sidebar.
- [x] **Projects as main unit** (2026-10-14): each project is a folder with its own tabs, Conversation, and Git; switching projects keeps other terminals running in background.
- [x] **Custom title bar, agent usage, and new sidebar** (2026-10-14): window without system decoration, usage indicators (session and week) in title bar, and project selector in sidebar.
- [x] **Split screen by agent type** (2026-10-14): automatic grid with one pane per CLI type, draggable dividers, repositioning via `⋮⋮` handle, and inter-pane shortcuts (`Ctrl+Alt+arrows`).
- [x] **Narrower right sidebar** (2026-10-14): default width reduced from 380 to 330 px (minimum 280 px).
- [x] **Gemini removed** (2026-10-14): replaced by Antigravity (`agy`) throughout agent catalog, prompts, and documentation.
- [x] **Git tab and icons in right sidebar header** (2026-10-13): local Git tab (status, diff, stage, commit, branches, pull/push) with automatic reload via `.git/` watcher.
- [x] **Fixed right sidebar with Conversation** (2026-10-13): integrated Conversation moved from beneath terminals to a fixed right sidebar of adjustable width.
- [x] **Bridge and tab security** (2026-10-13): dedicated token per agent process, reply restricted to target (`--reply`), ESC cleanup on paste, `Host` validation, and body and connection limits.
- [x] **Depth and orphan inter-tab questions** (2026-10-13): delegation depth limit applied between tabs, and discard of pending questions when closing caller's tab.
- [x] **Predictable tab delivery** (2026-10-13): `delivery.rs` respects user typing, Claude and Codex turn start/end hooks, and delayed delivery (HTTP 202) when response exceeds 90 s.
- [x] **Codex and OpenCode in agent catalog** (2026-10-12): support for Codex and OpenCode CLIs in interactive tabs and headless mode.
- [x] **Isolated environment** (2026-10-12): internal app variables do not leak to child agents and data folder is protected by `.lock` lock.
- [x] **Machine-independent tests** (2026-10-12): `system_dirs = false` in tests to isolate installed CLIs, and `rustfmt.toml` + `cargo fmt --check` in CI.
- [x] **Robust Whisper download** (2026-10-12): resumption via `Range`, retries with exponential backoff, SHA-256 verification, progress bar, and Linux support (`download.rs`).
- [x] **Mouse in terminal** (2026-10-12): sends click, drag, and scroll to TUI programs in modes 1000/1002/1003/1006, with `Shift` reserved for local text selection.
- [x] **IME in terminal** (2026-10-12): support for accent composition and CJK anchored to terminal cursor, displaying underlined pre-edit.
- [x] **Instances with identity and real inter-tab conversation** (2026-10-12): tabs addressable as `model[@role[@name]]`, automatic typing into PTY via bridge, and wait cycle detection.
- [x] **Pasting on Windows crashed app** (2026-10-11): clipboard reading restricted to UI thread, eliminating Windows heap corruption.
- [x] **Initial Windows validation** (2026-10-11): tested by hand tabs, ConPTY, microphone/Whisper, sound, file drag-and-drop, image pasting, and session restore.

## 2026-10-15 audit

Comprehensive review of the codebase by four Claude instances and Antigravity, covering atomic
configuration persistence, Git isolation on project switch, bridge queue and security, safe
pasting in terminals, and workspace path hardening. All findings were resolved (except automatic
Whisper GPU-to-CPU fallback when exiting code 0 without output, which was dismissed).

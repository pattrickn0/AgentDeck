# Holt: overview

Starting point of the documentation, for humans and for AI agents working on the project. It explains
**how Holt works under the hood** (the "environment" where agents run), **what you can do**
with it, and **which rules each document defines**. Technical details are in the other files in this folder.

| Document | Read when |
| :-- | :-- |
| [HOLT.md](HOLT.md) | you want to know where the product is headed, principles, and the order of what comes next |
| [OVERVIEW.md](OVERVIEW.md) (this file) | you want the general map and what can be done in the app |
| [ARCHITECTURE.md](ARCHITECTURE.md) | you are going to work on the bridge, tabs, instances, tasks, or the inter-agent protocol |
| [MAINTENANCE.md](MAINTENANCE.md) | you are going to compile, test, find a file, or follow a recipe (e.g., new agent) |
| [backlog.md](backlog.md) | you want to know the next steps (at most 10, in the order they will be done) |
| [ideas.md](ideas.md) | you want to see undated ideas and known limitations (what is not among the next steps) |
| [history.md](history.md) | you want to know what has already been done (the destination of every completed item from backlog.md) |
| [manual-tests.md](manual-tests.md) | you are testing by hand on Windows with real agents (checklist walkthrough) |
| [LINUX.md](LINUX.md) | you are compiling, running, or testing on Linux (Ubuntu and Fedora) and want to know what has been verified and what is pending |

---

## 1. Environment: how Holt works

### The app

Native desktop terminal in Rust (winit + egui). There are two executables, which must remain **in the same folder**:

- `holt` (crate `app`): the window, the terminal tabs, and the right sidebar (tasks desk, integrated Conversation,
  Git, and GitHub);
- `ask-agent` (crate `cli`): the command agents use to talk to each other and operate tasks.

The "backend" is the `core` crate, with no UI. The UI only talks to it via `Core` methods and receives
`CoreEvent` events back.

### Folders

| Folder | Contents |
| :-- | :-- |
| `workspace/` | the legacy workspace, from the time when there was a global folder. Today each **project** has its own folder (the `cwd` of its tabs); `workspace/` only serves as a migration target from an old config |
| `data/` | runtime data: attachments (`attachments/`), prompts (`prompts/`), hooks (`hooks/`), instructions (`holt-instructions.md`, `agy/<instance>/AGENTS.md`), tasks (`tasks/<project>.json` and `*-brief.md`/`-feedback.md`), sessions, `usage.json`, `crash.log`, `holt.log` (rotates above 2 MB), lock `.holt.lock` and `.holt.bridge` |
| `whisper/` | engine and models for voice dictation (whisper.cpp) |
| `%APPDATA%\Holt\config.json` | configuration. With `HOLT_HOME`, everything stays inside that folder |

**One window per data folder:** opening the app again with the same `data/` brings the live instance back (`POST /activate`)
and exits silently; the window × hides to the system tray (or quits, without a tray) and exit is handled by the icon menu. For another
isolated instance, use `HOLT_HOME`.

### Tabs are instances

Each tab is an **instance** named `model[@role[@name]]`:

- `claude`: the main instance of the model;
- `claude@dev`: an instance with a role;
- `claude@dev@ana`: a second instance of the same model and role, which gets a name.

In addition to the name, each instance has an **LLM model** (`sonnet`, `opus`…), which is not part of the id: it appears next to
the name on the tab and in the sidebar, and is chosen via `ask-agent --model`, the "+ New agent" menu (the `›` arrow for
Claude, Antigravity, and OpenCode lists models; in OpenCode, by provider), or by the `roleModels` default in the
configuration (`{"claude@dev": "sonnet"}`; `model@role` takes precedence over `model`).

The name is the address used by `ask-agent`. Tabs return with the same name and model when the app reopens.

### The environment of each tab

| Variable | What it is |
| :-- | :-- |
| `HOLT_AGENT` | instance name (`claude@dev`) |
| `HOLT_ROLE` | role, when present |
| `HOLT_PORT` | local HTTP bridge port (only on `127.0.0.1`) |
| `HOLT_TOKEN` | **tab-specific** token: how the bridge knows who is asking and from which project |
| `HOLT_DEPTH` | conversation depth (used in headless mode) |
| `HOLT_ASK_WAIT` | how many seconds `ask-agent` waits for a response (default 90; `0` = until completion). User-configurable |
| `PATH` | executable directory comes first, so `ask-agent` is always found |

No other `HOLT_*` variable from the app environment is passed to agents. Holt instructions (with the rule
not to use CLI-internal subagents) arrive through each CLI's dedicated channel, without depending on a user's `AGENTS.md`
(only Goose and Copilot create `AGENTS.md` if the folder does not have one), and folder trust is registered before opening the
tab. Details in [ARCHITECTURE.md](ARCHITECTURE.md).

### The path of an inter-agent question

1. An agent runs `ask-agent claude@dev "task"`.
2. The bridge resolves the destination in the caller's project. If the instance does not exist, its tab is opened in the background.
3. The message is **typed into the destination tab's terminal** when it becomes free (never immediately after a user keypress;
   with hooks in Claude and Codex, after turn end; without hooks, after 1.5 s idle or 30 s in queue).
4. The destination agent replies with `ask-agent --reply <n> "response"`. Only that agent can reply to that question.
5. The response returns to the caller: within 90 s it outputs to stdout of `ask-agent`; past that, `ask-agent` finishes
   with a notice (HTTP 202) and the response is **typed later** into the caller's terminal (and remains in `ask-agent --inbox`).
   In a `shell` tab or in headless mode, `ask-agent` waits until completion.

Everything appears in the **integrated Conversation**, and the user can intervene in any tab.

### The window

| Area | What it has |
| :-- | :-- |
| Title bar | custom (no system title bar): ☰ (settings), collapse sidebar, brand and "Holt", `/ <project>` and branch chip (with the sidebar collapsed, the compact **Agents \| Documents toggle** appears right after); on the right, **usage for each agent with quota** (logo + 5h session and weekly bars; amber starting at 80%, red at 100%; hovering opens the **usage panel** with groups and renewal times) and minimize/maximize/close |
| Left sidebar | the **Agents \| Documents toggle** at the top, **PROJECTS** (each project with its agents in a tree and model on the right; the active one has "+ New agent"; "+" in the header opens a folder as a project; right-click renames, opens folder, or removes), **ORGANIZATION** (`Tabs \| Split \| Highlight`), and **FILES**, the explorer (with Git status letter, refresh, and collapse all); collapses with `Ctrl+B` |
| Center | two views (`Ctrl+Shift+D`): **Agents** (terminals organized in Tabs, Split, or Highlight; in each pane header are tabs, `⋮⋮` handle, microphone, attachment, and "+") and **Documents** (diff, editable files, and markdown occupying the entire center) |
| Right sidebar | **fixed, does not collapse**; width adjustable via border. Four tabs at the top: **Tasks** (the first; badge counts review + validation), **Conversation** (read-only: the feed of inter-agent messages, with unread counter), **Git**, and **GitHub** (with open PR count) |

- **Projects:** each project is a folder with its agents. Switching projects hides tabs of the previous one, which
  **keep running**. The tree, Tasks, Git, GitHub, and Conversation belong to the active project; usage in the title bar is global.
  Removing and re-adding the same folder reuses the project id and recovers its tasks.
- **Organization (`Tabs | Split | Highlight`):** in **Split**, tabs of the same CLI type share the same pane in an
  automatic grid (2 side-by-side, 2+1, 2×2, 3+2, 3×2), with draggable dividers and a `⋮⋮` handle to rearrange panes;
  in **Highlight**, the pane for the active tab's type occupies 2/3 on the left and the others stack on the right; in **Tabs**,
  all stay in a single pane. The active pane gets a border and halo in the agent's color and the others fade; tab strips that
  do not fit show `‹`/`›` arrows and a `▾` menu. Shortcuts: `Ctrl+Tab` / `Ctrl+Shift+Tab` within the pane, `Ctrl+Alt+arrows` between
  panes, `Ctrl+T` new tab.
- **Documents (`Ctrl+Shift+D`):** opening a diff, file (double click in explorer; single click inserts `@file` into
  terminal), or report brings **Documents** to the center; closing the last one or clicking an agent returns to **Agents**. The "Agents"
  badge counts unseen completed turns (or pulses if an agent is working) and hidden terminals are not
  resized. Text files up to 256 KB have **syntax highlighting** (9 languages), search (`Ctrl+F`), and **editing**
  (`Ctrl+S` preserving CRLF/LF and BOM, with confirmation on close without saving and notification if disk changes externally); up to 1 MB
  open read-only. Diffs feature a file list on the left, highlighting, search, and copy of code only; `.md`, Brief, and
  Report open formatted.
- **Tasks tab:** each task runs in an agent with its own branch and worktree (`.holt/worktrees/tN`). Lifecycle:
  *Running* → (`--task done`) *In review* → (merge) *Waiting for validation* (tab, worktree, and branch remain alive
  to test the app) → (validate) *Completed*; *Reject* returns it to the same agent. The **+ New** form takes title,
  brief, **level** (*Simple*, *Medium*, *Complex*), agent/model (recommended by capability + quota headroom), effort,
  base, **scope** (if it overlaps another active task, it enters **the queue** and starts automatically when cleared), and attachments. The
  detail view displays brief, report, diff (**View full diff** / **View merged diff**), **MESSAGES** section, and
  history, and offers buttons to resolve merge rejections right there (stash changes and merge, ask
  agent to commit or resolve conflict).
- **Git tab and GitHub tab:** the **Git** tab operates the local repository (Changes with stage/discard per file or per
  chunk/hunk, commit, paginated History, Stash, branches with merge/rebase, and Conflict resolution), updating automatically
  when `.git/` changes. The **GitHub** tab lists PRs (with checks), Issues, and Actions (with tail of failed job logs) from the
  `origin` remote, using login credentials from Git Credential Manager or `gh`, and allows **Send to agent**.

### Limits, protections, and headless mode

| Rule | Value |
| :-- | :-- |
| Maximum delegation depth (A → B → C; the third chained question is rejected) | 2 |
| Wait cycle (A waits for B, which waits for A) | rejected immediately (HTTP 409) |
| Tabs opened automatically by `ask-agent` | 8 |
| Maximum question time | 10 min |
| Message size / concurrent requests / total bridge body budget | 16 MB / 128 / 64 MB |
| Text from another agent typed into a terminal | no ESC or control characters |
| Prompt for a `.cmd` shim on Windows | passed via file, never on command line |
| File opened in Documents mode | up to 1 MB (editable up to 256 KB / 4,000-byte lines), no null bytes |

When "Open agent tabs" is turned off in Settings, `ask-agent` runs agents in **headless mode**
(`claude -p`, `agy -p`, `codex exec`, `opencode run`…) with a temporary token (tasks require tabs turned on). The full HTTP contract
of the bridge is in [ARCHITECTURE.md](ARCHITECTURE.md).

---

## 2. What you can do

**As a user:**

- Open agent tabs (Claude Code, Antigravity, Codex, OpenCode, Aider, Goose, Copilot, Amazon Q, Open Interpreter) and
  regular terminal tabs, choosing the LLM model per instance.
- Organize work into **projects** (one folder each), view agents from all projects **in a tree** in the sidebar,
  and reorganize the **team** (lead and crew) by dragging one agent onto another.
- View multiple agents at the same time in **split screen** or **highlight** (`Tabs | Split | Highlight`).
- Track **usage** for each agent with quota (session and week) in the title bar and usage panel.
- Talk to an agent via its tab's terminal, reference files (`@file`), attach files/images (button, drag-and-drop,
  or `Ctrl+V`), and use **voice dictation** (`Ctrl+Shift+M` or microphone icon, via local whisper.cpp).
- Follow inter-agent conversations in the integrated Conversation (read-only) and intervene directly in terminals.
- Read and edit project files with syntax highlighting and search, or read diffs and reports full-width in
  **Documents** mode (`Ctrl+Shift+D`).
- Create and manage **tasks** in the Tasks tab (with level, model, effort, scope with automatic queue, diff review,
  assisted merge, and validation step).
- Operate local **Git** (stage per file or hunk, commit, branches, merge/rebase, conflicts, stash) and inspect
  **GitHub** (PRs, issues, Actions, and sending CI failures to an agent).
- Close the window via × to keep agents running in the **system tray**, or exit via the tray menu.

**As an agent, inside a tab:**

| Command | Does |
| :-- | :-- |
| `ask-agent <agent> "message"` | asks another agent or instance (`claude@dev`), opening its tab if it does not exist |
| `ask-agent --new <agent> "message"` | opens another instance of the same role (`claude@dev@ana`) |
| `ask-agent --model <model> <agent> "message"` | chooses the instance's LLM when opening it (`sonnet`, `opencode/glm-5.3-flash-free`…); combines with `--new`; error 409 if tab is already open with a different model |
| `ask-agent --reply <n> "reply"` | replies to question number `n` |
| `ask-agent --file <file> …` | reads message, brief, report, or feedback from a file (preserves accented characters, which PowerShell 5.1 converts to `?` on stdin `-`) |
| `ask-agent --list` | lists available agents and open instances in the project, with model and quota notice |
| `ask-agent --team` | shows your current lead and crew, with model and quota notice |
| `ask-agent --models [<agent>]` | lists, per installed agent (the four official ones in order of greatest quota headroom first), models accepted by `--model` (`-free` first and marked "(free)"; OpenCode by provider), the `quota:` line for each group, and the **capability** table (`simple`, `medium`, `complex`). Bridge rejects (409) questions to exhausted models |
| `ask-agent --boss "message"` | asks or reports to your lead |
| `ask-agent --learn [<file>...]` | without arguments, shows global learnings (`memory/learnings/`), index of remaining ones, and **Current situation** (running tasks and latest commits); with files, shows learnings matching them and warns at the top if another running task touches them |
| `ask-agent --outline <file>...` | shows the outline of each file (signatures and line numbers, without bodies, in Rust and Markdown) to read only the necessary snippet with offset |
| `ask-agent --inbox [--all]` | shows replies to your questions that have not yet been delivered (or the last 20 with `--all`), marking them as delivered |
| `ask-agent --task new "title" [--agent <m>] [--model <m>] [--effort low\|medium\|high] [--level simple\|medium\|complex] [--base <branch>] [--start] -\|"brief"\|--file <file> [--scope <path>…]` | creates a task (dedicated worktree + branch + agent; without `--agent`, chooses the one with the most quota headroom among capable models for the level); `--start` starts it immediately (or queues it if `--scope` overlaps another running/in-review task) |
| `ask-agent --task list` / `show <id>` | lists project tasks / shows full detail of a task |
| `ask-agent --task start <id>` / `merge <id>` / `validate <id>` / `discard <id>` | starts draft / merges task in review into base (`--no-ff`, moving to waiting for validation) / finishes validation and cleans up / discards and cleans up |
| `ask-agent --task done <id> -\|"report"\|--file <file>` | task agent submits report and puts it in review (also valid in validation if new commits exist) |
| `ask-agent --task return <id> "feedback"\|--file <file>` | returns task in review to agent with adjustments (including learnings from files mentioned in feedback) |
| `ask-agent --task reject <id> "reason"\|--file <file>\|-` | rejects task waiting for validation and returns it to the same agent in the same worktree |
| `ask-agent --signal busy\|idle` | internal use by turn start/end hooks (always exits 0) |
| `ask-agent --usage` | internal use by Claude's statusline (sends plan usage to bridge) |
| `ask-agent --help` (`-h`) | prints summary of all commands above |

---

## 3. Documents and their rules

Each document covers a specific topic. Changed the topic? Update the document **in the same commit**.

| File | Purpose | Rules it defines / how to maintain |
| :-- | :-- | :-- |
| `memory/OVERVIEW.md` | overview and this map | update when a new document, folder, command, or limit is introduced |
| `memory/HOLT.md` | product vision and direction | the orchestrator decides the order of what comes next; update when direction changes |
| `memory/learnings/` | long-term memory: one file per pitfall, decision, or convention that is not obvious from the code | format and when to create in `learnings/README.md`; `touches` must exist (verified by test `every_learning_in_the_repository_is_valid`) |
| `memory/ARCHITECTURE.md` | stack, crates, bridge protocol, instances, tab delivery, tasks, and memory | **contract**: routes, HTTP codes, environment variables, and bridge limits. Changed the protocol? Update here |
| `memory/MAINTENANCE.md` | practical guide: Rust, commands, code map, recipes | code map and recipes (e.g., new agent: prompt never raw on command line, hooks if CLI supports them) |
| `memory/backlog.md` | next steps, numbered, at most 10 ("Now" and "Later") | completed item leaves `backlog.md` and goes to `history.md` with one line describing what was done; a new item only enters if it fits within the 10 (otherwise goes to `ideas.md`); development only (manual tests stay in `manual-tests.md`); file does not keep `[x]` |
| `memory/ideas.md` | undated ideas by area, and known limitations | idea that enters `backlog.md` leaves here; new limitation enters here; does not keep completed items |
| `memory/history.md` | what has already been done, newest to oldest (includes the 2026-10-15 audit) | only receives items that left `backlog.md`; one line per item; technical details stay in the commit |
| `memory/manual-tests.md` | manual test script (Windows, real agents), by area (same areas as `backlog.md`) | check `[x]` what passed; what fails becomes an item in `backlog.md`; new feature testable only by hand gets an item in its area; removed feature removes its items |
| `memory/LINUX.md` | dependencies, verified items, and manual script for Linux (Ubuntu and Fedora) | update in the same commit that changes a system dependency, font path, data folder, or GPU support; Linux failures become items in `backlog.md` |
| `README.md` (root) | overview, how to build and run, `ask-agent` examples | stays in the root because GitHub displays it on the repository page |
| `CONTRIBUTING.md` (root) | how to contribute and **project principles** | run `cargo fmt`, `clippy -D warnings`, and `cargo test` before PR; privacy; UI never hangs; texts in pt-BR. Stays in the root because GitHub displays it when opening PRs |
| `AGENTS.md` (root) and `workspace/AGENTS.md` | instructions read by agent CLIs | **generated by the app**: block between `<!-- holt:start -->` and `<!-- holt:end -->` is rewritten on each start; edit only outside it. Located in working folder root because CLIs look for them there |
| `workspace/README.md` | what the `workspace/` folder is (legacy workspace and app's `AGENTS.md`) | isolation: agents work in the project or task folder, not in app files |
| `data/README.md` | what goes into `data/` (attachments, prompts, tasks, hooks, `crash.log`) | privacy: audio deleted after transcription, retention of only recent sessions, nothing versioned |
| `whisper/README.md`, `whisper/models/README.md` | engine and models for dictation | located next to the binaries and models they describe |
| `LICENSE` | MIT license | do not edit |

Rule files that are not `.md`: `rustfmt.toml` (code style, checked in CI), `.github/workflows/ci.yml` (what must pass on every push), and `Cargo.toml` (`rust-version` 1.89 is the minimum Rust version).

### General rules, in summary

1. The UI (`app`) only talks to the backend via `Core` methods; the backend responds with `CoreEvent`.
   (ARCHITECTURE, MAINTENANCE)
2. Before finishing any change: `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace`. (CONTRIBUTING)
3. The bridge only listens on `127.0.0.1`, identifies callers by token, and never trusts `from` in the body.
   Text from another agent never goes raw to a terminal or to the command line. (ARCHITECTURE, CONTRIBUTING)
4. Nothing blocking runs on the UI thread. (CONTRIBUTING)
5. Privacy: audio deleted right after transcription; attachments and prompts kept only for the last 5 sessions;
   never create `AGENTS.md` in the user's home folder. (CONTRIBUTING, data/README)
6. Code comments, docs, and commit messages in English; user-facing text through `t!` with a pt-BR entry; agent-facing
   text in English only. (CONTRIBUTING, learnings/i18n-english-key-and-area-files.md)
7. Documentation alongside code: changed behavior, protocol, or pending work, update the corresponding `.md`
   in `memory/` in the same commit; completed item from `backlog.md` moves to `history.md`. (this document)

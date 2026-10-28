# AgentDeck architecture

Overview and map of documents: [GERAL.md](GERAL.md). Practical guide and file map: [MANUTENCAO.md](MANUTENCAO.md).

Native desktop application in Rust (rewritten from the old Electron version, no Node at runtime).

## Stack and base decisions

| Layer | Choice | Why |
| :-- | :-- | :-- |
| Window and events | **winit 0.30** (`ApplicationHandler`) | full control over the event loop, drag and drop, and undecorated window; forces X11 under WSLg (`app/src/wslg.rs`) |
| Rendering and UI | **glutin + glow** with **egui 0.36** (`egui-winit` + `egui_glow`) | custom-styled immediate mode (`theme.rs`); lightweight OpenGL that runs even on `Xvfb`/llvmpipe in CI |
| Terminal | **alacritty_terminal** (VT parser + grid) + **portable-pty** (ConPTY on Windows, PTY on Unix) | mature VT emulation; custom grid rendering in egui (`app/src/terminal/`) and debounced resize to compensate for ConPTY repaint |
| HTTP bridge | **tiny_http** (thread per request, up to 128 concurrent) | local server on `127.0.0.1:0` for `ask-agent`, authenticated by process token + `Host` validation |
| HTTP client | **ureq** (with rustls) | synchronous and without OpenSSL; runs in threads for GitHub, Antigravity quota, and downloads |
| System and files | **notify**, **rfd** (portal on Linux), **open**, **arboard**, **tray-icon** / **ksni**, **secret-service** (D-Bus on Linux) | native integrations without pulling in GTK3 |
| Audio and dictation | **cpal** (feature `audio`) + **whisper.cpp** (`whisper/`) | optional local transcription; audio deleted immediately after transcription |

## Crates and boundaries

```
crates/
├── core   (agentdeck-core)   backend without graphical interface
├── app    (agentdeck)        winit + egui window and terminal emulator
└── cli    (agentdeck-cli)    ask-agent binary
```

- **`app` ↔ `core` boundary:** the UI only speaks to the backend via `agentdeck_core::Core` methods (`crates/core/src/lib.rs`) and receives events through a channel (`events.rs`), waking the winit loop via `EventLoopProxy`. Blocking operations (git, GitHub, model listing, transcription, document read/write) always run in threads outside the UI.
- **Work with and without frame (`crates/app/src/app.rs`, `crates/app/src/deck/`):** work that does not depend on drawing (core events, thread replies, `ask-agent` deliveries to terminals, delayed answers, scheduled Enter, turn completion, and timers) runs before each frame with the window visible and, when the window is in the tray, minimized, or has 0×0 area (on Windows there is no `WM_PAINT`), in a background step in the event loop every ≥ 50 ms (`ControlFlow::WaitUntil`). Without a frame, the PTY is never resized blindly.
- **Extension for RAG (`crates/core/src/context.rs`):** `ContextProvider` trait, queried by the headless runner before sending the prompt.

---

## Inter-agent protocol and bridge contract (`ask-agent`)

### Environment of each agent process (`crates/core/src/agents.rs`, `env.rs`)

Each interactive tab (and headless execution) receives a sanitized environment:

- **Injected:** `AGENTDECK_PORT` (TCP port on `127.0.0.1`), `AGENTDECK_TOKEN` (128-bit token **exclusive to the tab or execution**), `AGENTDECK_AGENT` (instance id, e.g. `claude@dev`), `AGENTDECK_ROLE` (when there is a role), `AGENTDECK_DEPTH` (delegation depth), `PATH` (with executable folder up front), `TERM=xterm-256color`, `COLORTERM=truecolor`, and on Windows, `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8`, `MSYS_NO_PATHCONV=1`, and `MSYS2_ARG_CONV_EXCL=*` (prevents Git Bash from converting `/command` into a path).
- **Filtered:** any `AGENTDECK_*` inherited from the external environment (except `AGENTDECK_ASK_WAIT`, a user configuration), Claude nested session variables (`CLAUDECODE`, `CLAUDE_CODE_*`), `ELECTRON_RUN_AS_NODE`, and `OPENCODE_CONFIG_CONTENT` (inserted later only for the OpenCode tab it belongs to).

### AgentDeck instructions and folder trust (`agents.rs`, `trust.rs`, `workspace.rs`)

Bridge instructions (use of `ask-agent`, prohibition of CLI-internal subagents without user request, `--models`/`--team`/`--learn`/`--outline`, and `memory/aprendizados/`) arrive through each CLI's dedicated channel, without depending on a prior `AGENTS.md`:

| CLI | Instruction delivery channel |
| :-- | :-- |
| Claude | `--append-system-prompt <text>` (includes name and role), `--allowedTools "Bash(ask-agent:*)"`, and `--settings dados/hooks/claude-settings.json` |
| Codex | `-c developer_instructions=<TOML>` (or raw text in `.cmd` shim) and, outside shims, `-c notify=["ask-agent","--signal","idle"]` |
| OpenCode | `--standalone` + `OPENCODE_CONFIG_CONTENT={"model":…,"instructions":["dados/agentdeck-instructions.md"]}` (`models.rs`) |
| Aider | `--read dados/agentdeck-instructions.md` |
| Open Interpreter | `--custom_instructions <text>` |
| Antigravity | `--add-dir dados/agy/<instance>` (where the app writes an `AGENTS.md` with instance name and role, combined with project's by `agy`) |
| Goose, Copilot | read only the folder's `AGENTS.md`: if missing, the app creates one with only the `<!-- agentdeck:start/end -->` block and notifies in Conversation; if already present, updates only the block |
| Amazon Q, Terminal | no instruction channel |

The text in a shared file (`dados/agentdeck-instructions.md` and `AGENTS.md` block) does not list agents installed on the machine, so as not to dirty `git status` across computers.

**Folder trust (`crates/core/src/trust.rs`):** before opening a tab, the app registers trust for the working folder and project folder so background tabs do not stall on *"Do you trust this folder?"*: in Claude in `~/.claude.json` (if no parent folder is already accepted), in Codex via `-c projects={…}` on the command line, and in Antigravity in `trustedWorkspaces` of `~/.gemini/antigravity-cli/settings.json` (exact path, removed alongside task worktree). User home and drive root are never added.

### Token identity and project isolation (`bridge.rs`, `instances.rs`)

- **Identity:** the bridge identifies callers via the `x-agentdeck-token` header (token per open instance or headless execution). The JSON `"from"` field is ignored for agent tokens; only the app's master token can supply `"from"` and `"project"`.
- **Project scope:** the token carries the project (`p` + 8 hex). Internally, pending questions, per-pair queues, activity, and inbox use the `<project>#<id>` key, allowing two projects to each have their own `claude@dev` without crossing messages or cycles. For agents and users, ids appear without the prefix.

### Bridge HTTP contract (`crates/core/src/bridge.rs`)

Every request requires `x-agentdeck-token` (constant-time comparison) and `Host: 127.0.0.1:<port>` or `localhost:<port>` (other value or missing → **403**). Global limits: up to **128** concurrent requests (**503** above; HTTP 202 questions occupy a slot until delayed reply completes), individual body up to **16 MB** (**413** above), and sum of bodies reading/in flight up to **64 MB** (**503** above).

| Route | Body / parameters | Response and HTTP codes |
| :-- | :-- | :-- |
| `POST /activate` | `{}` (master token only) | **200** `{"ok":true}` and brings window forward; **403** for agent token |
| `GET /agents` | — | **200** `{"agents":[…],"models":{id:model},"quota":{id:quota}}` for caller's project |
| `GET /models[?agent=<id>]` | `agent` optional | **200** `{"text"}` with inventory (`ask-agent --models`): the 4 official CLIs from most quota headroom to most spent, `cota:` line per group, and `capacidade` block; **400** if invalid; **404** if not installed |
| `GET /team` | — (tab token only) | **200** `{"id","boss","crew","models","quota"}` and consumes team change notification; **400** with master token |
| `POST /ask` | `{"to","message","depth","new","wait"[,"reply_to"][,"model"][,"ack"]}` (+ `"from"`, `"project"` with master token) | **200** `{"answer","request"}` within deadline; **202** `{"pending":true,"answer","request"}` after `wait` (reply typed later in tab); **400** unknown target, self-question, `depth >= 2`, invalid `model`, or CLI without model choice; **404** missing `reply_to`; **409** wait cycle, instance already open with different model (suggests `--new`), or exhausted quota (≥ 99.5%); **413** > 16 MB; **500** error or timeout (10 min) |
| `POST /reply` | `{"to","message"[,"reply_to"]}` | **200** `{"answer"}` completing pending question; **400** invalid arguments; **404** no pending question from that sender |
| `GET /inbox[?all=1]` | — (tab token only) | **200** `{"answers":[{"request","from","text","failed","ts","delivered"}]}` (without `all`, undelivered; `all=1`, last 20) and marks as delivered; **400** with master token |
| `POST /inbox/delivered` | `{"request":<n>}` (tab token only) | **200** `{"ok":true}` confirming delivery of reply `<n>` after printing to stdout (when `/ask` sent `"ack":true`); **400** invalid/master token; **404** does not belong to caller |
| `POST /learn` | `{"paths":[…]}` (up to 50 paths) | **200** `{"text"}` with learnings and current situation (without paths) or collision warnings + matching learnings (with paths); **400** without repository |
| `POST /outline` | `{"paths":[…]}` (1 to 50 files) | **200** `{"text"}` with signatures and lines of each file; **400** without files or without repository |
| `POST /signal` | `{"state":"busy"\|"idle"}` | **200** `{"ok":true}` updating tab turn state (tab token only) |
| `POST /usage` | `{"session":{"used","resets_at_ms"}\|null,"weekly":…}` | **200** `{"ok":true}` recording plan usage (`0.0..=1.0`, tab token only; `shell` → **400**) |
| `GET /tasks`, `GET /tasks/<id>` | `?project=<id>` optional with master token | **200** `{"tasks":[…]}` (summary with `scope` and `waiting_for`) or complete `{"task":{…}}`; **404** does not exist |
| `POST /tasks` | `{"title","brief"[,"kind"][,"model"][,"effort"][,"level"][,"base"][,"scope"][,"start"]}` | **200** `{"task"}` (with `start:true`, in `running` or `queued`); **400** invalid data, invalid scope, or without git; **403** active task agent |
| `POST /tasks/<id>/start`, `/merge`, `/validate`, `/discard` | `{}` | **200** `{"task"}`; **403** task agent; **404** does not exist; **409** incompatible state, dirty tree, merge conflict, or held worktree; **500** git failure |
| `POST /tasks/<id>/done` | `{"report"}` | **200** `{"task"}` moving to `review` (task owner or master token only; valid in `running` or `validating` with new commits); **400** empty; **403** another agent; **409** invalid state |
| `POST /tasks/<id>/return`, `/reject` | `{"feedback"}` | **200** `{"task"}` returning from `review` (`return`) or rejecting from `validating` (`reject`) to `running`; **400** empty; **403** task agent; **409** invalid state |

**Exit codes of `ask-agent` (`crates/cli/src/bin/ask-agent.rs`):** `0` success, `1` bridge/agent error, `2` argument error or environment outside AgentDeck (`--signal` always exits 0 to avoid breaking hooks).

### Single instance and system tray (`activation.rs`, `app/src/tray.rs`)

- **Second launch:** the core holds the `dados/.agentdeck.lock` lock and writes port + master token to `dados/.agentdeck.bridge`. A second launch detects the occupied lock, calls `AllowSetForegroundWindow` (on Windows) + `POST /activate`, and exits; the running instance restores and focuses the window.
- **System tray:** closing the window (×, `Alt+F4`, or title bar button) hides the window to the tray (`tray-icon` on Windows, `ksni`/StatusNotifierItem on Linux), keeping agents and bridge active in the background; without a tray on Linux, minimizes. Only **Exit** in the tray menu closes the app, prompting for confirmation in the window if there is an unsaved document or an agent in turn.

---

## Instances, team, models, quota, and capability

- **Identity and team (`instances.rs`, `team.rs`):** instances use `<model>[@<role>[@<name>]]` (`claude`, `claude@dev`, `claude@dev@ana`, `claude@main@bia`). When an agent creates another instance via `ask-agent`, it becomes its lead; the user reorganizes by dragging in the sidebar (cycles are rejected). Closing a tab reassigns subordinates to its lead; renaming preserves the tree. Position changes generate a notice attached to the next message delivered in the tab (or consumed by `ask-agent --team`). The team guides, but does not block direct calls.
- **LLM model and effort (`models.rs`):** resolved when opening the tab in order: (1) explicit request (`--model` or menu), (2) model already saved for the instance in the project, (3) role/CLI default in configuration, (4) CLI's own default. Only `claude`, `antigravity`, `codex`, and `opencode` accept models; requesting a different model for an already open tab returns **409**. Passed as `--model <m>` in `claude`, `agy`, and `codex` (and `opencode run -m` in headless); in OpenCode TUI, passed via `--standalone` + `OPENCODE_CONFIG_CONTENT`. Models for `agy` and `opencode` are listed in a thread (15 s timeout, 10 min cache, `-free` first). Effort (`low`, `medium`, `high`) becomes `--effort` in Claude and suffix `-low|medium|high` in Antigravity (rejected with 400 in others).
- **Plan usage and quota (`usage.rs`, `usage/codex.rs`, `usage/antigravity.rs`):** tracks session (5 h) and week (`0.0..=1.0` + renewal time) and saves in `dados/usage.json`. In Claude, comes from `statusLine` hook (`ask-agent --usage` reads `rate_limits` from stdin, posts `POST /usage`, and passes stdin through to user statusline); in Codex, reads most recent `rollout-*.jsonl` from `~/.codex/sessions` every 60 s; in Antigravity, queries `retrieveUserQuotaSummary` (fallback `fetchAvailableModels`) every 3 min with `agy` OAuth token and version (Windows Credentials, Secret Service D-Bus on Linux, or file), separated by group (`Gemini 4 Argon`, `Gemini Models`, `Claude and GPT models`). On bridge, `/ask` rejects with **409** targets with group ≥ 99.5% (reading < 1 h old) and warns when target exceeds 80% while another official CLI is < 50%.
- **Capability and task selection (`capability.rs`, `capability.json`):** matches task level (`simples`, `media`, `complexa`) with the *AA Coding Agent Index v2.0* (`≥ 53` complex, `40..53` medium, `< 40` simple; effort `low` drops one level; overridable via `dados/capacidade.json`). With ≥ 3 evaluated tasks of a model at a level (`dados/tarefas/*.json`), actual history prevails (more than half discarded or ≥ 2 returns drops one level; last 3 of next level approved on first try raises one level). Without an explicit agent, task creation picks among capable installed CLIs for the level the one with lowest quota used; explicit selection below level generates warning without blocking.

---

## Inter-tab question flow and terminal delivery

### Question, reply, and cycle detection (`exchange.rs`, `inbox.rs`)

1. **Resolution and queue:** `POST /ask` resolves destination in caller's project (or creates instance and opens tab in background, up to 8 automatic tabs). Calls for same `from->to` pair follow a FIFO queue; in headless, sessions for the pair are resumed in Claude (`--session-id`/`--resume`) and Antigravity (`--conversation`).
2. **Cycles and depth:** if target already waits directly or indirectly for caller, `/ask` rejects with **409**. Pending question chain between tabs is rejected starting at depth 2.
3. **Synchronous or delayed reply (HTTP 202):** `ask-agent` waits up to `AGENTDECK_ASK_WAIT` (default 90 s). If `ask-agent --reply <n>` arrives in time, it outputs to stdout with HTTP 200 and the client confirms receipt in `POST /inbox/delivered` (since `tiny_http` does not detect if client died before reading 200). If exceeding 90 s, `/ask` returns **202** freeing `ask-agent` (question stops blocking cycles, but continues counting toward depth) and, when destination replies (within 10 min), the reply is typed into caller's terminal. `shell` tabs and headless executions always wait up to 10 min.
4. **Inbox (`inbox.rs`):** each reply (up to 20 per instance) stays as "undelivered" until `/inbox/delivered` or typing in tab. If caller tab closed or died, app notifies in Conversation and reply remains available in `ask-agent --inbox`.

### When to type in the terminal (`crates/core/src/delivery.rs`, `crates/app/src/deck/`)

The policy in `delivery.rs` avoids overriding the user or TUI:
- **Respect for user:** does not type within 2 s after a user keypress or within 20 s while typed text remains without Enter.
- **Tab readiness:** waits 5 s when opening tabs without hooks; with hooks (Claude and Codex), waits for `idle` signal + 300 ms without output (10 min fallback); without hooks, 1.5 s without output or 30 s waiting in queue.
- **Pasting and Enter:** cleans `ESC` and C0/C1 controls (protecting bracketed paste) and sends via PTY write thread. Enter follows 250 ms later (plus a second Enter in 600 ms when `@mention` is present in Claude/Antigravity; in Windows Codex adds 1 ms per pasted character, up to 10 s). User keys pressed during Enter wait are emitted immediately after it.
- **`.cmd` shims and processes on Windows (`agents.rs`, `headless.rs`, `process.rs`):** interactive tabs open shims via `cmd.exe /d /e:on /v:off /c` with escaped arguments; free prompts never go on a shim's command line (in headless they go via file in `dados/prompts/`). Headless processes and `whisper-cli` join a Job Object with `KILL_ON_JOB_CLOSE`.

---

## Tasks desk (`crates/core/src/tasks/`, `scope.rs`, `crates/app/src/tasks_ui.rs`)

Each task (`t1`, `t2`…, per-project counter never reused) isolates work in an agent (`<model>@tN`), a branch (`tarefa/<id>-<slug>`), and a dedicated worktree.

```
Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged ("completed")
  │  │              ▲  ▲  ▲           │                  │ └─discard──▶ Discarded (from any non-final state)
  │  │              │  │  └──return───┘                  │
  │  │              │  └────────────reject───────────────┘
  │  └─ busy scope ▶ Queued ──(other leaves Running/Review)──▶ Running
  └─ start error ▶ Failed ──start again──▶ Running
```

- **Storage and worktrees (`tasks/store.rs`, `tasks/git_ops.rs`):** saved in `dados/tarefas/<projeto>.json` (outside repo; removing and re-adding folder preserves project id). Worktrees live in `<repo root>/.agentdeck/worktrees/<id>` — inside repository so workspace-confined agents (such as Antigravity) have access — and `/.agentdeck/` is recorded in `<common git>/info/exclude` (never in `.gitignore`).
- **Start and delivery (`tasks/lifecycle.rs`, `tasks/text.rs`):** creates worktree and branch from base, registers `<model>@<id>` subordinate to creator, and delivers brief with memory and rules. When the tab needs to be opened, writes brief (or feedback) to `dados/tarefas/<projeto>-<id>-brief.md` (or `-feedback.md`) and passes a short ASCII line as initial CLI argument (`claude "<line>"`, `agy -i "<line>"`, `opencode --prompt "<line>"`), avoiding loss of text while TUI initializes.
- **Scope and queue (`scope.rs`, `tasks/lifecycle.rs`):** scope lists relative paths (no globs); two scopes collide when one equals or is a path-component prefix of the other (`memory/` and `Cargo.lock` reserve nothing). If colliding with task in `Running`, `Review`, or ahead in queue, enters `Queued` without creating worktree or tab. When blocking task is merged, discarded, or fails to start, queue advances automatically, creating worktree from current base head (`Validating` does not hold the queue). On `done`, files touched outside scope produce a warning in review.
- **Merge (`tasks/merge.rs`, `tasks/stash.rs`):** requires main repo on base branch and clean (ignoring and restoring automatic `AGENTS.md` block) and clean worktree. Performs `git merge --no-ff`, aligns task branch (`git merge --ff-only`), and transitions to `Validating` keeping tab, worktree, and branch alive. In UI, blockages offer one-click resolution — including stashing local changes in a `git stash`, merging, and reapplying by SHA hash after pre-verifying with `git merge-tree --write-tree`.
- **Validation, rejection, discard, and permissions (`tasks/finish.rs`, `file_lock.rs`, `bridge.rs`):** `validate` requires no unmerged commits, closes tab, removes worktree (diagnosing processes locking folder on Windows) and branch, and marks `Merged`. `reject` reopens tab/worktree if needed and returns to `Running` with reason. `discard` cleans tab, worktree, and branch from any non-final state. On bridge, active task agent can only query tasks and call `/done` on its own task (other task routes → **403**).

---

## Two-layer memory (`learnings.rs`, `situation.rs`, `outline.rs`)

1. **Long term — Learnings (`learnings.rs`):** files `memory/aprendizados/*.md` with frontmatter `titulo`, `resumo`, and `touches` (globs relative to root; empty = global), read from task worktree or main repo. At task start, agent receives globals and those matching paths cited in brief in full (up to 6,000 characters) plus index of remaining ones (up to 2,500); on returns/rejections, receives those matching newly cited files in feedback (up to 3,000).
2. **Short term — Current situation and history (`situation.rs`, `tasks/`):** delivers other active tasks with files they touch (branch commits + worktree dirt via `git --no-optional-locks status`) and last 8 commits of base (up to 1,500 characters). Returns and rejections repeat last 5 feedbacks, and task stores up to 300 messages (6,000 characters each) exchanged by agent.
3. **On-demand query (`--learn` and `--outline`):** `ask-agent --learn [<file>…]` (`POST /learn`, up to 12,000 characters) returns globals + index + current situation (without arguments) or `ATTENTION:` conflict warnings + matching learnings (with files). `ask-agent --outline <file>…` (`POST /outline`, `outline.rs`, up to 6,000 characters/file and 16,000/call) extracts signatures and lines (`impl`/`trait`/`mod` items and members in Rust, headings in Markdown) over `highlight.rs` lexer.

---

## Interface: Organization, Documents Mode, Git, and GitHub

- **Center organization (`crates/app/src/split.rs`, `ui/panes.rs`, `ui/tabs.rs`):** groups project tabs by CLI type. In `Split`, builds automatic grid with saved ratios; in `Highlight`, places active tab group in 2/3 on left (switching active tab switches highlight) and stacks others on right; in `Tabs`, uses single window. PTY is resized only when size stabilizes and mouse is released (150 ms debounce on drag, 600 ms on Highlight switch, and 300 ms after prior resize exit, 3 s ceiling).
- **Documents mode and editor (`crates/app/src/docs.rs`, `ui/docs.rs`, `ui/code.rs`, `ui/diff_view.rs`, `highlight.rs`):** toggles center (`Ctrl+Shift+D`) between terminals and document tabs (`TaskDiff`, `CommitDiff`, `FileDiff`, `File`, `Markdown`). With terminals hidden, no resize or keypress reaches them and turn ends turn into a badge on "Agents". Files up to 256 KB (lines ≤ 4,000 bytes, UTF-8 without mixed CRLF/LF) open in editor with layout/highlight cache and atomic save preserving CRLF/LF and BOM; external changes detected by watcher with unsaved edit do not overwrite buffer and open resolution banner. Files up to 1 MB and diffs use line-by-line rendering with custom selection (`ui/select.rs`) and search (`Ctrl+F`); Markdown uses `egui_commonmark`.
- **Local Git and GitHub (`crates/core/src/git.rs`, `github.rs`, `crates/app/src/git_ui.rs`, `github_ui.rs`):** local Git runs `git` binary without a shell, passing paths and commit message via stdin with non-interactive environment (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`), while watcher observes `.git/` (`HEAD`, `index`, `refs/`). GitHub panel reads `origin` remote, retrieves token only in memory (`git credential fill` or `gh auth token`, dropped on 401), queries `api.github.com` in threads (60 s cache), and downloads trailing 32 KiB of failed job logs without resending `Authorization` after redirect.

# Holt architecture

Overview and map of documents: [OVERVIEW.md](OVERVIEW.md). Practical guide and file map: [MAINTENANCE.md](MAINTENANCE.md).

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
├── core   (holt-core)   backend without graphical interface
├── app    (holt)        winit + egui window and terminal emulator
└── cli    (holt-cli)    ask-agent binary
```

- **`app` ↔ `core` boundary:** the UI only speaks to the backend via `holt_core::Core` methods (`crates/core/src/lib.rs`) and receives events through a channel (`events.rs`), waking the winit loop via `EventLoopProxy`. Blocking operations (git, GitHub, model listing, transcription, document read/write) always run in threads outside the UI.
- **Work with and without frame (`crates/app/src/app.rs`, `crates/app/src/deck/`):** work that does not depend on drawing (core events, thread replies, `ask-agent` deliveries to terminals, delayed answers, scheduled Enter, turn completion, and timers) runs before each frame with the window visible and, when the window is in the tray, minimized, or has 0×0 area (on Windows there is no `WM_PAINT`), in a background step in the event loop every ≥ 50 ms (`ControlFlow::WaitUntil`). Without a frame, the PTY is never resized blindly.
- **Extension for RAG (`crates/core/src/context.rs`):** `ContextProvider` trait, queried by the headless runner before sending the prompt.

---

## Inter-agent protocol and bridge contract (`ask-agent`)

### Environment of each agent process (`crates/core/src/agents.rs`, `env.rs`)

Each interactive tab (and headless execution) receives a sanitized environment:

- **Injected:** `HOLT_PORT` (TCP port on `127.0.0.1`), `HOLT_TOKEN` (128-bit token **exclusive to the tab or execution**), `HOLT_AGENT` (instance id, e.g. `claude@dev`), `HOLT_ROLE` (when there is a role), `HOLT_DEPTH` (delegation depth), `PATH` (with executable folder up front), `TERM=xterm-256color`, `COLORTERM=truecolor`, and on Windows, `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8`, `MSYS_NO_PATHCONV=1`, and `MSYS2_ARG_CONV_EXCL=*` (prevents Git Bash from converting `/command` into a path).
- **Filtered:** any `HOLT_*` inherited from the external environment (except `HOLT_ASK_WAIT`, a user configuration), Claude nested session variables (`CLAUDECODE`, `CLAUDE_CODE_*`), `ELECTRON_RUN_AS_NODE`, and `OPENCODE_CONFIG_CONTENT` (inserted later only for the OpenCode tab it belongs to).

### Holt instructions and folder trust (`agents.rs`, `trust.rs`, `workspace.rs`)

Bridge instructions (use of `ask-agent`, prohibition of CLI-internal subagents without user request, `--models`/`--team`/`--learn`/`--outline`, and `memory/learnings/`) arrive through each CLI's dedicated channel, without depending on a prior `AGENTS.md`:

| CLI | Instruction delivery channel |
| :-- | :-- |
| Claude | `--append-system-prompt <text>` (includes name and role), `--allowedTools "Bash(ask-agent:*)"`, and `--settings data/hooks/claude-settings.json` |
| Codex | `-c developer_instructions=<TOML>` (or raw text in `.cmd` shim) and, outside shims, `-c notify=["ask-agent","--signal","idle"]` |
| OpenCode | `--standalone` + `OPENCODE_CONFIG_CONTENT={"model":…,"instructions":["data/holt-instructions.md"]}` (`models.rs`) |
| Aider | `--read data/holt-instructions.md` |
| Open Interpreter | `--custom_instructions <text>` |
| Antigravity | `--add-dir data/agy/<instance>` (where the app writes an `AGENTS.md` with instance name and role, combined with project's by `agy`) |
| Goose, Copilot | read only the folder's `AGENTS.md`: if missing, the app creates one with only the `<!-- holt:start/end -->` block and notifies in Conversation; if already present, updates only the block |
| Amazon Q, Terminal | no instruction channel |

The text in a shared file (`data/holt-instructions.md` and `AGENTS.md` block) does not list agents installed on the machine, so as not to dirty `git status` across computers.

**Folder trust (`crates/core/src/trust.rs`):** before opening a tab, the app registers trust for the working folder and project folder so background tabs do not stall on *"Do you trust this folder?"*: in Claude in `~/.claude.json` (if no parent folder is already accepted), in Codex via `-c projects={…}` on the command line, and in Antigravity in `trustedWorkspaces` of `~/.gemini/antigravity-cli/settings.json` (exact path, removed alongside task worktree). User home and drive root are never added.

### Token identity and project isolation (`bridge.rs`, `instances.rs`)

- **Identity:** the bridge identifies callers via the `x-holt-token` header (token per open instance or headless execution). The JSON `"from"` field is ignored for agent tokens; only the app's master token can supply `"from"` and `"project"`.
- **Project scope:** the token carries the project (`p` + 8 hex). Internally, pending questions, per-pair queues, activity, and inbox use the `<project>#<id>` key, allowing two projects to each have their own `claude@dev` without crossing messages or cycles. For agents and users, ids appear without the prefix.

### Bridge HTTP contract (`crates/core/src/bridge.rs`)

Every request requires `x-holt-token` (constant-time comparison) and `Host: 127.0.0.1:<port>` or `localhost:<port>` (other value or missing → **403**). Global limits: up to **128** concurrent requests (**503** above; HTTP 202 questions occupy a slot until delayed reply completes), individual body up to **16 MB** (**413** above), and sum of bodies reading/in flight up to **64 MB** (**503** above).

| Route | Body / parameters | Response and HTTP codes |
| :-- | :-- | :-- |
| `POST /activate` | `{}` (master token only) | **200** `{"ok":true}` and brings window forward; **403** for agent token |
| `GET /agents` | — | **200** `{"agents":[…],"models":{id:model},"quota":{id:quota}}` for caller's project |
| `GET /models[?agent=<id>]` | `agent` optional | **200** `{"text"}` with inventory (`ask-agent --models`): the 4 official CLIs from most quota headroom to most spent, `quota:` line per group, and `capacidade` block; **400** if invalid; **404** if not installed |
| `GET /team` | — (tab token only) | **200** `{"id","boss","crew","models","quota"}` and consumes team change notification; **400** with master token |
| `POST /ask` | `{"to","message","depth","new","wait"[,"reply_to"][,"model"][,"ack"]}` (+ `"from"`, `"project"` with master token) | **200** `{"answer","request"}` within deadline; **202** `{"pending":true,"answer","request"}` after `wait` (reply typed later in tab); **400** unknown target, self-question, `depth >= 2`, invalid `model`, or CLI without model choice; **404** missing `reply_to`; **409** wait cycle, instance already open with different model (suggests `--new`), or exhausted quota (≥ 99.5%); **413** > 16 MB; **500** error or timeout (10 min) |
| `POST /reply` | `{"to","message"[,"reply_to"]}` | **200** `{"answer"}` completing pending question; **400** invalid arguments; **404** no pending question from that sender |
| `GET /inbox[?all=1]` | — (tab token only) | **200** `{"answers":[{"request","from","text","failed","ts","delivered"}],"mail":[{"mail","from","kind","request","ts","read","preview"}]}` (without `all`, undelivered answers and unread mail; `all=1`, last 20 of each) and marks the answers as delivered; an answer shown in full also counts as read for its mail; **400** with master token |
| `GET /mail/<n>` | — (tab token only) | **200** `{"mail","from","kind":"question"\|"answer","request","ts","text","answerable"}` and marks the mail as read; **400** invalid number; **403** the mail is addressed to another agent, or the master token asked; **404** no such mail in the caller's project |
| `POST /inbox/delivered` | `{"request":<n>}` (tab token only) | **200** `{"ok":true}` confirming delivery of reply `<n>` after printing to stdout (when `/ask` sent `"ack":true`); **400** invalid/master token; **404** does not belong to caller |
| `POST /learn` | `{"paths":[…]}` (up to 50 paths) | **200** `{"text"}` with learnings and current situation (without paths) or collision warnings + matching learnings (with paths); **400** without repository |
| `POST /outline` | `{"paths":[…]}` (1 to 50 files) | **200** `{"text"}` with signatures and lines of each file; **400** without files or without repository |
| `POST /signal` | `{"state":"busy"\|"idle"}` | **200** `{"ok":true}` updating tab turn state (tab token only); `idle` also types the notice again for unread mail (see Durable mail) |
| `POST /usage` | `{"session":{"used","resets_at_ms"}\|null,"weekly":…}` | **200** `{"ok":true}` recording plan usage (`0.0..=1.0`, tab token only; `shell` → **400**) |
| `GET /tasks`, `GET /tasks/<id>` | `?project=<id>` optional with master token | **200** `{"tasks":[…]}` (summary with `scope` and `waiting_for`) or complete `{"task":{…}}`; **404** does not exist |
| `POST /tasks` | `{"title","brief"[,"kind"][,"model"][,"effort"][,"level"][,"base"][,"scope"][,"start"]}` | **200** `{"task"}` (with `start:true`, in `running` or `queued`); **400** invalid data, invalid scope, or without git; **403** active task agent |
| `POST /tasks/<id>/start`, `/merge`, `/validate`, `/discard` | `{}` | **200** `{"task"}`; **403** task agent; **404** does not exist; **409** incompatible state, dirty tree, merge conflict (`/merge`: the agent was asked to resolve it and the merge is approved, see the Tasks desk), or held worktree; **500** git failure |
| `POST /tasks/<id>/done` | `{"report"}` | **200** `{"task"}` moving to `review` (task owner or master token only; valid in `running` or `validating` with new commits; if the merge was already approved, the task comes back already `validating`); **400** empty; **403** another agent; **409** invalid state, or the branch would conflict with the base (the message tells the agent to `git merge <base>`, resolve, check, commit and deliver again) |
| `POST /tasks/<id>/return`, `/reject` | `{"feedback"}` | **200** `{"task"}` returning from `review` (`return`) or rejecting from `validating` (`reject`) to `running`; **400** empty; **403** task agent; **409** invalid state |

**Exit codes of `ask-agent` (`crates/cli/src/bin/ask-agent.rs`):** `0` success, `1` bridge/agent error, `2` argument error or environment outside Holt (`--signal` always exits 0 to avoid breaking hooks).

### Single instance and system tray (`activation.rs`, `app/src/tray.rs`)

- **Second launch:** the core holds the `data/.holt.lock` lock and writes port + master token to `data/.holt.bridge`. A second launch detects the occupied lock, calls `AllowSetForegroundWindow` (on Windows) + `POST /activate`, and exits; the running instance restores and focuses the window.
- **System tray:** closing the window (×, `Alt+F4`, or title bar button) hides the window to the tray (`tray-icon` on Windows, `ksni`/StatusNotifierItem on Linux), keeping agents and bridge active in the background; without a tray (Linux without `StatusNotifierWatcher`), the × quits instead. **Exit** in the tray menu (or the × without a tray) closes the app, prompting for confirmation in the window if there is an unsaved document or an agent in turn.

---

## Instances, team, models, quota, and capability

- **Identity and team (`instances.rs`, `team.rs`):** instances use `<model>[@<role>[@<name>]]` (`claude`, `claude@dev`, `claude@dev@ana`, `claude@main@bia`). When an agent creates another instance via `ask-agent`, it becomes its lead; the user reorganizes by dragging in the sidebar (cycles are rejected). Closing a tab reassigns subordinates to its lead; renaming preserves the tree. Position changes generate a notice attached to the next message delivered in the tab (or consumed by `ask-agent --team`). The team guides, but does not block direct calls.
- **LLM model and effort (`models.rs`):** resolved when opening the tab in order: (1) explicit request (`--model` or menu), (2) model already saved for the instance in the project, (3) role/CLI default in configuration, (4) CLI's own default. Only `claude`, `antigravity`, `codex`, and `opencode` accept models; requesting a different model for an already open tab returns **409**. Passed as `--model <m>` in `claude`, `agy`, and `codex` (and `opencode run -m` in headless); in OpenCode TUI, passed via `--standalone` + `OPENCODE_CONFIG_CONTENT`. Models for `agy` and `opencode` are listed in a thread (15 s timeout, 10 min cache, `-free` first). Effort (`low`, `medium`, `high`) becomes `--effort` in Claude and suffix `-low|medium|high` in Antigravity (rejected with 400 in others).
- **Plan usage and quota (`usage.rs`, `usage/codex.rs`, `usage/antigravity.rs`):** tracks session (5 h) and week (`0.0..=1.0` + renewal time) and saves in `data/usage.json`. In Claude, comes from `statusLine` hook (`ask-agent --usage` reads `rate_limits` from stdin, posts `POST /usage`, and passes stdin through to user statusline); in Codex, reads most recent `rollout-*.jsonl` from `~/.codex/sessions` every 60 s; in Antigravity, queries `retrieveUserQuotaSummary` (fallback `fetchAvailableModels`) every 3 min with `agy` OAuth token and version (Windows Credentials, Secret Service D-Bus on Linux, or file), separated by group (`Gemini 4 Argon`, `Gemini Models`, `Claude and GPT models`). On bridge, `/ask` rejects with **409** targets with group ≥ 99.5% (reading < 1 h old) and warns when target exceeds 80% while another official CLI is < 50%.
- **Resuming conversations (`resume.rs`, `agents::spawn_spec_for`, `app/src/deck/tabs.rs`):** each tab has the CLI's own session id in `ProjectCfg.tab_sessions` (tab id → id), saved as soon as it is known: generated at the tab's first launch for Claude and OpenCode, learned afterwards for Antigravity and Codex, which choose their own. A launch is *first* (Claude `--session-id <uuid>`, OpenCode `--session ses_…`; Antigravity and Codex carry no id), *resume* (Claude `--resume <uuid>`; OpenCode `--session` again; Antigravity `--conversation <id>`; Codex `resume <id>`, always the first arguments; `SpawnSpec::resumed = true`) or *fallback*. Never `--continue` or `resume --last`. Claude's `SessionStart` hook (`ask-agent --signal session`, JSON on stdin, `POST /signal` with `state: "session"`) saves the new id after `/clear`. If a resumed process ends within 10 s with a failure code, `Deck::reopen_without_resume` forgets the id, starts the tab again from scratch (a task tab gets its brief line again) and posts one notice. The id is dropped by `Core::close_instance` (tab ×, finished or discarded task, removed project), not by quitting; renaming a tab moves it. **Antigravity:** the tab is launched with `--log-file data/agy-logs/<tab>.log` (`SpawnSpec::session_log`, removed before each launch) and `Deck::tick` reads what was appended every 2 s (`Core::sync_agy_session` → `resume::agy_session_in_log`: the last `Created`/`Resuming`/`Streaming conversation <uuid>` line wins, so `/new` is followed). **Codex:** `-c notify=["<bin>/ask-agent","--signal","idle"]` makes it call `ask-agent --signal idle <json>` after each turn; `signal_body` copies `thread-id` into `{"state":"idle","session":…}` and `POST /signal` saves it through `Core::set_tab_session`, which for Codex only accepts a thread whose rollout exists under `~/.codex/sessions` (the hidden title thread also calls `notify`). On Windows `Registry::resolve_cli` turns the npm `codex.cmd` into the native `codex.exe` (`codex_native_exe`) so `notify` can be passed. A stored id whose conversation is gone (`agy_has_conversation`, `codex_has_session`) is dropped and the tab opens a new conversation. Details and the flags checked per CLI: `learnings/resume-session-flags-per-cli.md` and `learnings/resume-antigravity-and-codex.md`.
- **Capability and task selection (`capability.rs`, `capability.json`):** matches task level (`simple`, `medium`, `complex`) with the *AA Coding Agent Index v2.0* (`≥ 53` complex, `40..53` medium, `< 40` simple; effort `low` drops one level; overridable via `data/capability.json`). With ≥ 3 evaluated tasks of a model at a level (`data/tasks/*.json`), actual history prevails (more than half discarded or ≥ 2 returns drops one level; last 3 of next level approved on first try raises one level). Without an explicit agent, task creation picks among capable installed CLIs for the level the one with lowest quota used; explicit selection below level generates warning without blocking.

---

## Inter-tab question flow and terminal delivery

### Question, reply, and cycle detection (`exchange.rs`, `inbox.rs`)

1. **Resolution and queue:** `POST /ask` resolves destination in caller's project (or creates instance and opens tab in background, up to 8 automatic tabs). Calls for same `from->to` pair follow a FIFO queue; in headless, sessions for the pair are resumed in Claude (`--session-id`/`--resume`) and Antigravity (`--conversation`).
2. **Cycles and depth:** if target already waits directly or indirectly for caller, `/ask` rejects with **409**. Pending question chain between tabs is rejected starting at depth 2.
3. **Synchronous or delayed reply (HTTP 202):** `ask-agent` waits up to `HOLT_ASK_WAIT` (default 90 s). If `ask-agent --reply <n>` arrives in time, it outputs to stdout with HTTP 200 and the client confirms receipt in `POST /inbox/delivered` (since `tiny_http` does not detect if client died before reading 200). If exceeding 90 s, `/ask` returns **202** freeing `ask-agent` (question stops blocking cycles, but continues counting toward depth) and, when destination replies (within 10 min), the reply is typed into caller's terminal. `shell` tabs and headless executions always wait up to 10 min.
4. **Inbox (`inbox.rs`):** each reply (up to 20 per instance) stays as "undelivered" until `/inbox/delivered` or typing in tab. If caller tab closed or died, app notifies in Conversation and reply remains available in `ask-agent --inbox`.

### Durable mail (`mail.rs`)

A message typed into a terminal is fragile when long (split pastes, Enter timing), clutters the receiver's context and is lost if the app exits before it is typed. So only a **short** message (one line, up to `mail::SHORT_MAX` = 200 characters) is typed as before. A longer or multi-line **question to a tab** (`ask_tab`) and a longer **late answer** (`deliver_late`, the other direction of `--reply`) are saved as *mail* and a one-line notice is typed instead: `[mail #12 from claude@dev: "first 60 chars…" — read with: ask-agent --read 12]` (through `terminal_text`, like any typed text). Failure notices and Holt's own messages (`request: 0`: tasks, GitHub tab) stay typed.

- **Files:** `data/mail/<project>/index.json` (metadata and the 60-character preview, never the text) and one `<n>.txt` per message. Numbers are per project and never reused; the body is written before the index entry; read mail beyond the last 200 is deleted, unread mail never. If it cannot be written, the text is typed as before.
- **Reading:** `GET /mail/<n>` (`ask-agent --read <n>`) answers only the addressee: the token says who asks, nothing in the body is trusted (**403** for another agent or the master token). Reading marks it read; a late answer's mail also marks the inbox entry delivered. For a question it says whether the question still waits for `--reply` (`answerable`; after a restart nobody waits any more and the CLI says so). `ask-agent --inbox` also lists the unread mail.
- **What is only in memory** (per run): was the notice handed to the interface (`notified`), typed (`typed`), how many reminders, when it was read. `Delivery.mail` carries the mail number to the interface, which calls `Core::mail_typed` when it types the notice, `Core::mail_dropped` when the queue is discarded (tab closed or ended) and does not type a notice whose mail was read meanwhile.
- **Reminder:** when the receiver's turn ends (`POST /signal` `idle`, or the app's release of a lost end-of-turn notice: `Core::mail_turn_ended`) and a notice that was **typed** is still unread, it is emitted again (`Delivery { request: 0, answer: true, mail }`: it never opens a tab). At most `mail::MAX_REMINDERS` = 2.
- **Restart:** every unread mail starts a run un-notified; `Deck::start` calls `Core::mail_tab_started` when a tab's process comes up and the notices of that instance are typed again. Pending questions do not survive an exit (the asker's process died with it), so after a restart a question's mail is information; `--read` says nobody waits for the answer.
- **Deadline:** a question that went through the mail counts its 10 minutes from when the mail is **read** (`bridge::mail_deadline`), still extended by the tab's activity and capped at 60 minutes after the reading. Unread, only the 60-minute cap from the sending applies (the timeout text then says the mail was not read).

### When to type in the terminal (`crates/core/src/delivery.rs`, `crates/app/src/deck/`)

The policy in `delivery.rs` avoids overriding the user or TUI:
- **Respect for user:** does not type within 2 s after a user keypress or within 20 s while typed text remains without Enter.
- **Tab readiness:** waits 5 s when opening tabs without hooks; with hooks (Claude and Codex), waits for `idle` signal + 300 ms without output (10 min fallback); without hooks, 1.5 s without output or 30 s waiting in queue.
- **Pasting and Enter:** cleans `ESC` and C0/C1 controls (protecting bracketed paste) and sends via PTY write thread. Enter follows 250 ms later (plus a second Enter in 600 ms when `@mention` is present in Claude/Antigravity; in Windows Codex adds 1 ms per pasted character, up to 10 s). User keys pressed during Enter wait are emitted immediately after it.
- **`.cmd` shims and processes on Windows (`agents.rs`, `headless.rs`, `process.rs`):** interactive tabs open shims via `cmd.exe /d /e:on /v:off /c` with escaped arguments; free prompts never go on a shim's command line (in headless they go via file in `data/prompts/`). Headless processes and `whisper-cli` join a Job Object with `KILL_ON_JOB_CLOSE`.

---

## Chat view of an agent tab (`crates/core/src/chat.rs`, `app/src/deck/chat.rs`, `app/src/ui/chat.rs`)

A Claude tab shows a friendly chat (messages, tool calls with diffs, permission cards, a composer) instead of Claude Code's terminal
UI, with the CLI still running underneath. Holt never reads the terminal screen: the CLI runs in its **structured mode**
(`claude -p --input-format stream-json --output-format stream-json --include-partial-messages --verbose
--permission-prompt-tool stdio`, `chat::chat_args`) and the events are rendered. Chat is the default view of Claude
(`Config.chat_by_default`); the tab menu switches a tab to Terminal and back (`ProjectCfg.tab_views`, `Deck::set_tab_view`): the process is
replaced, the conversation continues (same session id, `--resume`). Other CLIs keep the terminal (`chat::supports`).

- **Core (`chat.rs`, no interface):** `parse_line` turns one stdout line into `ChatEvent`s (init, text deltas, complete assistant message,
  tool results, permission request, interrupt, result; sub-agent traffic is dropped; a `control_request` it does not know is answered with
  an error so the CLI never waits). `Transcript` is the conversation (`Item`: user / assistant text / tool call / permission card / notice):
  `apply(event)` builds it, `push_user` starts a turn (`busy` = turns sent and not yet ended), `from_history` rebuilds it from the CLI's
  own transcript file (`<claude>/projects/<folder>/<session>.jsonl`) so a resumed tab shows what was said (`Core::chat_history`).
  `Core::spawn_chat` starts the child (same `SpawnSpec` env as a PTY tab: `HOLT_*`, `ask-agent` on `PATH`; registered in the process
  registry and, on Windows, in the app's Job Object) with a reader, a stderr and a writer thread, and wakes the interface; `ChatProcess`
  kills it on drop. `Core::spawn_spec_chat` is `spawn_spec_for` for the chat: same conversation plan (`--session-id` first, `--resume` after),
  instructions, model and allowed tools, but no turn hooks (the end of a turn is an event) and the task's first prompt travels as
  `SpawnSpec::first_message`, sent as the first user message.
- **What goes in:** `user_message_json`, `permission_response_json` (allow with `updatedInput`, optionally `updatedPermissions` = the request's
  `permission_suggestions` for "allow for this session"; deny with a message), `interrupt_json` (Esc / Stop).
- **App (`deck/chat.rs`):** `Tab.chat: Option<Box<ChatTab>>` (`Some` = chat view). `Deck::pump_chat` (in `tick`, so also with the window hidden)
  drains each process; `on_chat_event` saves a new session id after `/clear`, auto-denies `AskUserQuestion` with "ask in plain text", marks
  the tab "done" and rings when a permission card needs the user, and at the end of a turn calls `on_agent_signal(false)` and
  `Core::mail_turn_ended` (the reminder of unread mail, which the terminal gets from the `Stop` hook). `chat_submit` sends the composer
  (a terminal-only slash command such as `/config`, `/login`, `/theme`, or one the CLI lists in `terminal_slash_commands`, is not sent: a
  notice points to the Terminal view; every other slash command is sent as text and works in this mode). Dictation, attached files and
  files clicked in the tree are inserted in the composer instead of being typed into a terminal (`chat_insert`).
- **Delivery (`ask-agent`, durable mail, GitHub tab messages):** the same queue and rule as a terminal tab (`delivery::gate`), with a chat
  `TabState`: `hooked`, `busy` = the transcript's, `last_output` = the last event, never `typing`. When ready, `chat_deliver` sends the text
  as a user message (no paste, no Enter timing) and the conversation shows only the message and who sent it (`agent_message_display` strips
  the "how to answer" instructions Holt adds for the model). A question for a busy tab, or one waiting for a permission answer, waits.
- **View (`ui/chat.rs`):** a scroll area of items in a centered column; each item's height is remembered (`ChatTab::heights`), so only the
  items near the viewport are laid out (markdown is the cost; a resumed conversation can have a thousand items). Assistant text is
  `egui_commonmark` like Documents; a tool call is a collapsible row (Edit/Write open by default, with the diff of `old_string`/`new_string`);
  a pending permission is an amber card (Allow / Allow for this session / Deny; Enter and Esc when the composer is empty); the composer
  grows to 8 lines, Enter sends, Shift+Enter newline, ↑/↓ history, `/` suggestions, Esc interrupts. Keys: with a chat tab active
  `term_focus` is off, so the terminal key path and IME are not used.
- **Resume across restarts:** the session id is in `tab_sessions` as for terminal tabs; the chat reloads the transcript file and restarts the CLI
  with `--resume`. `reopen_without_resume` also applies (a resume that fails within 10 s starts from scratch).
- **Checks:** `crates/core/tests/data/claude-stream-*.jsonl` are recordings of Claude Code 2.1.312 (tools + slash command, text streaming,
  interrupt) that `chat.rs` tests parse; `crates/core/tests/chat.rs` (launch, history, a fake CLI speaking the protocol, and an `#[ignore]`d test
  against the real `claude`); `app/src/chat_flow_tests.rs` (delivery into a chat tab, permission round trip, terminal-only command, switching
  views). `HOLT_DEBUG_UI=chat-demo` fills the first chat tab with a made-up conversation for screenshots.
- **Not done / follow-ups for other CLIs** (each needs its own structured transport and a parser like `parse_line` feeding the same
  `Transcript`): **Codex**: `codex exec --json` (one JSON event per line, but one turn per process: resume with `codex exec resume <id>`) or
  the `codex app-server` JSON-RPC (long-lived, approvals and streaming deltas; the better base for a chat). **OpenCode**: `opencode serve`
  (HTTP + SSE events: sessions, messages, permission requests) or `opencode run --format json` per turn. **Antigravity / Gemini and others**: ACP
  (Agent Client Protocol, JSON-RPC over stdio: `session/prompt`, `session/update` notifications, `session/request_permission`) would cover
  every CLI that speaks it with one client. Until then they keep the terminal. Also not done: `AskUserQuestion` (denied with a
  "ask as text" hint), images in the composer, a setting in Settings for the default view (`chat_by_default` is only in `config.json`),
  context-window percentage (needs the `usage` of the assistant messages), Windows with a `.cmd` shim (the arguments are escaped for the
  PTY path; the chat path was not run on Windows).

## Tasks desk (`crates/core/src/tasks/`, `scope.rs`, `crates/app/src/tasks_ui.rs`)

Each task (`t1`, `t2`…, per-project counter never reused) isolates work in an agent (`<model>@tN`), a branch (`task/<id>-<slug>`), and a dedicated worktree.

```
Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged ("completed")
  │  │              ▲  ▲  ▲           │                  │ └─discard──▶ Discarded (from any non-final state)
  │  │              │  │  └──return───┘                  │
  │  │              │  └────────────reject───────────────┘
  │  └─ busy scope ▶ Queued ──(other leaves Running/Review)──▶ Running
  └─ start error ▶ Failed ──start again──▶ Running
```

- **Storage and worktrees (`tasks/store.rs`, `tasks/git_ops.rs`):** saved in `data/tasks/<project>.json` (outside repo; removing and re-adding folder preserves project id). Worktrees live in `<repo root>/.holt/worktrees/<id>` — inside repository so workspace-confined agents (such as Antigravity) have access — and `/.holt/` is recorded in `<common git>/info/exclude` (never in `.gitignore`).
- **Start and delivery (`tasks/lifecycle.rs`, `tasks/text.rs`):** creates worktree and branch from base, registers `<model>@<id>` subordinate to creator, and delivers brief with memory and rules. When the tab needs to be opened, writes brief (or feedback) to `data/tasks/<project>-<id>-brief.md` (or `-feedback.md`) and passes a short ASCII line as initial CLI argument (`claude "<line>"`, `agy -i "<line>"`, `opencode --prompt "<line>"`), avoiding loss of text while TUI initializes.
- **Scope and queue (`scope.rs`, `tasks/lifecycle.rs`):** scope lists relative paths (no globs); two scopes collide when one equals or is a path-component prefix of the other (`memory/` and `Cargo.lock` reserve nothing). If colliding with task in `Running`, `Review`, or ahead in queue, enters `Queued` without creating worktree or tab. When blocking task is merged, discarded, or fails to start, queue advances automatically, creating worktree from current base head (`Validating` does not hold the queue). On `done`, files touched outside scope produce a warning in review.
- **Merge (`tasks/merge.rs`, `tasks/stash.rs`):** requires main repo on base branch and clean (ignoring and restoring automatic `AGENTS.md` block) and clean worktree. Performs `git merge --no-ff`, aligns task branch (`git merge --ff-only`), and transitions to `Validating` keeping tab, worktree, and branch alive. In UI, blockages offer one-click resolution — including stashing local changes in a `git stash`, merging, and reapplying by SHA hash after pre-verifying with `git merge-tree --write-tree`.
- **A delivery merges cleanly, and a conflict goes to the agent (`tasks/lifecycle.rs` `task_done`, `tasks/merge.rs`, `tasks/git_ops.rs` `merge_check`):** `done` first checks, with `git merge-tree --write-tree` (git 2.38+; skipped on an older git), that the branch merges into its current base; on a conflict it answers **409** with `MergeBlock::Conflict`'s wording (`conflict_fix`: `git merge <base>`, resolve keeping both sides, run the checks, commit, `done` again) and the task stays `Running`. The base can still move before the merge: when "Merge" (UI) or `--task merge` hits a conflict, the merge is undone, the agent gets that request at once (`Core::return_locked`, tab reopened if closed), the task goes back to `Running` with `Task::merge_approved` (persisted) and `conflict_asks`, and the call answers "the agent was asked" (`MergeOutcome::resolving`; `--task merge` → 409 with that text). When the agent delivers again, `task_done` merges by itself (`auto_merge_locked`, without stash). A new conflict asks again, at most `MAX_CONFLICT_ASKS` (3) requests in all, then the usual red refusal; any other refusal of the automatic merge (dirty main repository, dirty task folder…) leaves the task in `Review` with `Task::merge_stopped` (reason, and the files for a conflict), which the card shows. `Core::task_cancel_auto_merge` (the card's "Cancel automatic merge") drops the approval: the task waits in review when the agent delivers. A return with feedback or a successful merge also drops it. Review cards read "waiting for your review" and the tray tooltip counts the tasks in review / awaiting validation (`tray.rs` `set_waiting`).
- **Validation, rejection, discard, and permissions (`tasks/finish.rs`, `file_lock.rs`, `bridge.rs`):** `validate` requires no unmerged commits, closes tab, removes worktree (diagnosing processes locking folder on Windows) and branch, and marks `Merged`. `reject` reopens tab/worktree if needed and returns to `Running` with reason. `discard` cleans tab, worktree, and branch from any non-final state. On bridge, active task agent can only query tasks and call `/done` on its own task (other task routes → **403**).

---

## Two-layer memory (`learnings.rs`, `situation.rs`, `outline.rs`)

1. **Long term — Learnings (`learnings.rs`):** files `memory/learnings/*.md` with frontmatter `titulo`, `resumo`, and `touches` (globs relative to root; empty = global), read from task worktree or main repo. At task start, agent receives globals and those matching paths cited in brief in full (up to 6,000 characters) plus index of remaining ones (up to 2,500); on returns/rejections, receives those matching newly cited files in feedback (up to 3,000).
2. **Short term — Current situation and history (`situation.rs`, `tasks/`):** delivers other active tasks with files they touch (branch commits + worktree dirt via `git --no-optional-locks status`) and last 8 commits of base (up to 1,500 characters). Returns and rejections repeat last 5 feedbacks, and task stores up to 300 messages (6,000 characters each) exchanged by agent.
3. **On-demand query (`--learn` and `--outline`):** `ask-agent --learn [<file>…]` (`POST /learn`, up to 12,000 characters) returns globals + index + current situation (without arguments) or `ATTENTION:` conflict warnings + matching learnings (with files). `ask-agent --outline <file>…` (`POST /outline`, `outline.rs`, up to 6,000 characters/file and 16,000/call) extracts signatures and lines (`impl`/`trait`/`mod` items and members in Rust, headings in Markdown) over `highlight.rs` lexer.

---

## Interface: Organization, Documents Mode, Git, and GitHub

- **Center organization (`crates/app/src/split.rs`, `ui/panes.rs`, `ui/tabs.rs`):** groups project tabs by CLI type. In `Split`, builds automatic grid with saved ratios; in `Highlight`, places active tab group in 2/3 on left (switching active tab switches highlight) and stacks others on right; in `Tabs`, uses single window. PTY is resized only when size stabilizes and mouse is released (150 ms debounce on drag, 600 ms on Highlight switch, and 300 ms after prior resize exit, 3 s ceiling).
- **Documents mode and editor (`crates/app/src/docs.rs`, `ui/docs.rs`, `ui/code.rs`, `ui/diff_view.rs`, `highlight.rs`):** toggles center (`Ctrl+Shift+D`) between terminals and document tabs (`TaskDiff`, `CommitDiff`, `FileDiff`, `File`, `Markdown`). With terminals hidden, no resize or keypress reaches them and turn ends turn into a badge on "Agents". Files up to 256 KB (lines ≤ 4,000 bytes, UTF-8 without mixed CRLF/LF) open in editor with layout/highlight cache and atomic save preserving CRLF/LF and BOM; external changes detected by watcher with unsaved edit do not overwrite buffer and open resolution banner. Files up to 1 MB and diffs use line-by-line rendering with custom selection (`ui/select.rs`) and search (`Ctrl+F`); Markdown uses `egui_commonmark`.
- **Local Git and GitHub (`crates/core/src/git.rs`, `github.rs`, `crates/app/src/git_ui.rs`, `github_ui.rs`):** local Git runs `git` binary without a shell, passing paths and commit message via stdin with non-interactive environment (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`), while watcher observes `.git/` (`HEAD`, `index`, `refs/`). GitHub panel reads `origin` remote, retrieves token only in memory (`git credential fill` or `gh auth token`, dropped on 401), queries `api.github.com` in threads (60 s cache), and downloads trailing 32 KiB of failed job logs without resending `Authorization` after redirect.

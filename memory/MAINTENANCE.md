# Maintenance guide

This guide is for whoever maintains Holt, alone or alongside AI agents. It does not assume prior Rust experience.
Overview and document map: [OVERVIEW.md](OVERVIEW.md).

## 1. How Rust works in practice

- **Rust is compiled.** Source code (`.rs`) turns into a native executable (`.exe`). The end user does not need Rust, Node, or any runtime: just the `.exe`.
- **`cargo` is the tool for everything.** It downloads dependencies, compiles, runs tests, and formats code. It is equivalent to `npm` + `node` combined.
- **Crate = package.** This repository is a *workspace*: a root `Cargo.toml` grouping 3 crates in `crates/`.
- **Dependencies** live in `Cargo.toml`, like in `package.json`. Exact versions are locked in `Cargo.lock`, like in `package-lock.json`. Both go into git.
- **The compiler is your greatest ally.** Rust refuses to compile code with invalid memory usage, mismatched types, and various thread race conditions. If it compiled, a large portion of bugs has already been eliminated. Error messages are long, but usually tell you exactly what to do: read the `help:` suggestion.
- **`Result` instead of exceptions.** Functions that can fail return `Result<Value, Error>`. The `?` operator bubbles the error up to the caller. `unwrap()` crashes the program on error, so avoid it in production code.
- **Ownership.** Every value has an owner. To share across threads we use `Arc` (reference count) and `Mutex`/`RwLock` (lock). That is why `Core` is an `Arc` inside and can be cloned freely.

### Day-to-day commands

```bash
cargo run -p holt            # compiles and opens the app (debug)
cargo build --workspace --release   # generates optimized .exe files in target/release/
cargo test --workspace              # runs tests
cargo clippy --workspace --all-targets -- -D warnings   # "linter": flags suspicious code
cargo fmt --all                     # formats code (style in rustfmt.toml; CI checks with --check)
cargo update                        # updates dependencies within allowed versions
```

Minimum Rust version: **1.89** (`rust-version` in `Cargo.toml`). The initial compilation takes a few minutes because it compiles all dependencies. Subsequent ones are incremental and fast. The `target/` folder can be deleted at any time: it is just build cache.

## 2. Code map

```
Cargo.toml                 workspace + shared dependency versions
crates/
├── core/   (holt-core)   "backend": no graphical interface
│   ├── lib.rs        Core: the API used by the UI (like IPC channels)
│   ├── events.rs     CoreEvent: what the backend reports to the UI
│   ├── agents.rs     agent list, where to find each CLI, command for each tab, bridge instructions
│   ├── trust.rs      folder trust per CLI before opening tab (Claude, Codex, Antigravity)
│   ├── bridge.rs     local HTTP server used by ask-agent (/agents, /models, /team, /ask, /reply, /signal, /usage, /learn, /outline, /inbox, /mail, /tasks, /activate)
│   ├── exchange.rs   inter-tab questions awaiting reply (cycles, depth, reply by number)
│   ├── inbox.rs      inbox of replies per instance (`ask-agent --inbox`)
│   ├── mail.rs       long messages saved in `data/mail/` + one-line notice typed (`ask-agent --read`)
│   ├── instances.rs  instance names (model@role@name) and destination resolution
│   ├── team.rs       team: who reports to whom (pure functions)
│   ├── activity.rs   last activity of each tab (queried by tab delivery)
│   ├── models.rs     LLM model of each instance: resolution (requested > saved > role_models), validation, CLI arguments, model list, and `ask-agent --models` inventory
│   ├── capability.rs + capability.json   capability of each model: benchmark table, measured by completed tasks, and choice by capability + quota
│   ├── scope.rs      write scope per task and component-based overlap
│   ├── tasks/        tasks desk: mod.rs (Task, TaskStatus, errors, constants), text.rs (slug, id, messages to agent), store.rs (storage in data/tasks and queries), lifecycle.rs (create, start, queue, report, return), merge.rs, finish.rs (validate, reject, discard), git_ops.rs, and stash.rs
│   ├── file_lock.rs  diagnostics of files and processes holding a worktree
│   ├── highlight.rs  syntax highlighting lexer (custom tokenizer per language: `Tok`, `Span`, `Lang`, `highlight_lines`); lives in core because `outline.rs` uses it, and `app/src/highlight.rs` re-exports it and adds search, `pieces`, and `Cache`
│   ├── outline.rs    `ask-agent --outline`: file outline (signatures with line number, no bodies) per language, with budget
│   ├── learnings.rs  long-term memory: learnings from memory/learnings/ (frontmatter, globs, brief paths, budgeted delivery, `ask-agent --learn`)
│   ├── situation.rs  short-term memory: "Current situation" at task start and in `--learn` (other tasks, their files, clash warnings, and base commits)
│   ├── delivery.rs   when to type a message into a tab's terminal (pure rule, with tests)
│   ├── headless.rs   runs an agent in -p mode (without terminal) and captures reply
│   ├── whisper.rs    voice transcription (whisper.cpp); download.rs downloads engine and model
│   ├── usage.rs      plan usage (session and week); usage/codex.rs and usage/antigravity.rs read each source
│   ├── context.rs    extension point for RAG (ContextProvider; headless queries it)
│   ├── git.rs        workspace git: parse status/diff/branches (pure, with tests) and operations
│   ├── github.rs     GitHub (PRs, issues, Actions): remote → owner/repo, credential helper token, REST API, parse (pure, with tests)
│   ├── workspace.rs  working folder: tree listing, AGENTS.md, watcher, `resolve_workspace_path`, `read_text`, and `write_text` (text file up to 1 MB for Documents mode; with `safe_rel` and symlink check)
│   ├── activation.rs activation of live instance on second launch (`data/.holt.bridge` → `POST /activate`)
│   ├── attachments.rs, sessions.rs, config.rs, env.rs, paths.rs, process.rs, util.rs
│   ├── examples/memeval.rs   measures recall/precision/size of learning delivery on completed tasks
│   ├── tests/bridge.rs   end-to-end bridge tests
│   ├── tests/instructions.rs   instruction channels for each CLI
│   ├── tests/learn_situation.rs   `--learn` with current situation and file-in-use warning
│   ├── tests/git.rs      integration with real git (skipped if git is not installed)
│   ├── tests/tasks.rs    task lifecycle with real git (skipped if git is not installed)
│   ├── tests/tasks_bridge.rs   /tasks routes and task agent permissions
│   ├── tests/tasks_queue.rs    write scope and automatic queue progression
│   ├── tests/support/tasks_env.rs, fake_agent.rs   shared fixture and fake agent binary
│   └── tests/github.rs   origin remote with real git; one `--ignored` test with real network and credentials
├── app/    (holt)        graphical interface (the main .exe)
│   ├── main.rs / app.rs / window.rs / tray.rs / wslg.rs   winit window, OpenGL, event loop, system tray, and WSLg detection
│   ├── debug_ui.rs   fictional screenshot scenarios (`HOLT_DEBUG_UI=…`: `debug_ui::apply`, `debug_tasks`…)
│   ├── theme.rs      colors, fonts, and sizes (the "CSS")
│   ├── icons.rs      SVG icons rasterized at runtime
│   ├── util.rs       pure functions shared across the UI
│   ├── agents.rs     colors, names, and agent menus on the UI side
│   ├── highlight.rs  re-exports core lexer and adds search, `pieces`, and `Cache` for Documents mode
│   ├── terminal/     terminal emulator: mod.rs (pty + alacritty_terminal), view.rs (drawing and mouse), input.rs (keyboard), colors.rs (palette), record.rs (PTY recording for debugging: tab menu or `HOLT_DEBUG_PTY=1`), resize.rs (resizing as ConPTY expects)
│   ├── git_ui.rs     state and actions for Git tab (sections, topbar chip, reloads)
│   ├── github_ui.rs  state and actions for GitHub tab (core events, selection, "Send to agent" menu, demo data)
│   ├── tasks_ui.rs   state and actions for Tasks tab (list, detail, form, thread calls to core) and pure functions (groups, diff summary, texts, buttons per state), with tests; demo data
│   ├── tasks_flow_tests.rs   desk Deck end-to-end with real core and git (create, start, review, return, merge, validate, discard, reopen tab)
│   ├── docs.rs       center Documents mode: `CenterView`, `DocKind`/`DocTab`/`DocState` (including `Source`, editable file, and `Find`, search), `Docs` (open, focus, close, duplicate, per project), file list of a diff (`diff_files`), toggle badge (`Badge`), and in `impl Deck`, open each document type and read off UI thread (`UiMsg::Doc`); pure tests
│   ├── docs_demo.rs  fictional documents for `HOLT_DEBUG_UI=docs-demo*`
│   ├── dictation.rs  voice dictation: microphone states and fixed target in tab where recording started (pure functions, with tests)
│   ├── split.rs      split screen: groups, grid, dividers, neighbors (pure functions, with tests)
│   ├── deck/         tab state and UI logic: mod.rs (types, `Deck`, constants), frame.rs (frame, `work`/`background`, deliveries, events), tabs.rs, keyboard.rs, sidebar.rs, projects.rs, feed.rs, attachments.rs (attachments and dropped files), and dictation.rs
│   ├── audio.rs      notification sound and microphone (cpal)
│   ├── crash.rs      crash logging in data/crash.log; logfile.rs, app log in data/holt.log; smoke.rs, screenshot capture (HOLT_SMOKE)
│   └── ui/           mod.rs (rendering, modals) + background.rs (background with radial glows)
│       ├── titlebar.rs (title bar) + usage.rs (agent usage in the bar)
│       ├── panes.rs (split panes: header with handle, mic, attachment, and "+") + tabs.rs (tab strip: gradient, ‹ › arrows, ▾ menu; renders list of `Item`, serves agent tabs and document tabs)
│       ├── docs.rs (Documents mode: Agents | Documents toggle (`switch`, drawn by sidebar or, when collapsed, by title bar), document strip, diff with file list, text file, edit notice, search and save prompt, and markdown)
│       ├── code.rs (file as code: editor on `TextEdit` and large file rendered line by line), diff_view.rs (diff rendering with line wrapping, highlighting, and search), select.rs (text selection for custom-rendered screens)
│       ├── sidebar.rs (toggle, ORGANIZATION, and FILES) + projects.rs (projects and agents tree; `model_tag` shortens model name)
│       ├── right_panel.rs (right sidebar: Tasks/Conversation/Git/GitHub; Conversation is read-only)
│       ├── tasks.rs (Tasks tab), git.rs + git_views.rs (Git tab), github.rs (GitHub tab)
│       └── menus.rs (new tab, settings), shortcuts.rs (shortcuts), overlay.rs (drag-and-drop overlay), empty_state.rs (no open agent)
└── cli/    (holt-cli)    ask-agent
    ├── src/bin/ask-agent.rs     the agent command (`parse_task`, `learn_command`, `outline_command`, `models_command`, `inbox_command`, `read_command`, with tests)
    └── tests/ask_agent.rs, tests/tasks.rs   real `ask-agent` against a real bridge
```

**Golden rule:** the UI (`app`) only speaks to the backend via `Core` methods (in `crates/core/src/lib.rs`). Events return via `CoreEvent` (`events.rs`). Changed something in the backend? No UI screen needs to know how it was implemented.

### How the UI works ("immediate" mode)

The UI uses **egui**, which operates in *immediate mode*. There is no DOM tree like in HTML. On each frame, code "draws" the whole screen from the current state, and a click is detected in the exact same place that draws the button:

```rust
if ui.button("Send").clicked() {
    deck.send_message();
}
```

To change visuals, edit `theme.rs` (colors and sizes) or the corresponding area file in `ui/`. **winit** only creates the window and delivers operating system events: keyboard, mouse, file drops, and resizing.

## 3. Common recipes

**Adding a new CLI agent.** Edit `crates/core/src/agents.rs`:
1. Create an entry in `AGENTS` table with id, label, color, resolution function, and "not installed" message.
2. If it has a non-interactive mode, add arguments in `headless.rs` (in per-agent `match`). The prompt
   **never** goes raw on the command line: use stdin or `self.prompt_arg(prompt, via_cmd)`, which passes
   via file when the agent is a `.cmd` shim on Windows (`cmd.exe` would execute `&`, `|`… in the text).
3. If the CLI has turn start/end hooks, connect them to `ask-agent --signal busy|idle` in `spawn_spec_for`
   (like Claude and Codex): delivery into tabs becomes more accurate (`delivery.rs`).
4. If the CLI chooses the model, implement `models::accepts_model`, `cli_args` (tab), and `headless_args` (headless); if there is no
   option in interactive mode (like OpenCode TUI), find another path (env var config) and verify the result
   observing the real CLI. To list models in the menu, add the agent to `Core::known_models`.
5. Optionally, create an alias in `ALIASES`, and update the list of ids in the `workspace_agents_md_and_spawn_specs` test.

**Changing a color or size.** Edit `crates/app/src/theme.rs`: colors are fields of `Palette` (read via `theme::pal()`); accent color is that of the active agent (`theme::accent()`). Right sidebar widths: `RIGHT_*_W`. The icon (`assets/holt.ico`) is regenerated with `scripts/generate-icon.ps1`.

**Working on split screen.** Calculation is pure and tested in `crates/app/src/split.rs` (group of each tab,
grid layout, rectangles, dividers, neighbor for shortcuts, pane order, focus on close);
`Deck` holds the order (`group_order`), ratios (`split_ratios`), and which tab appears in each pane (most
recently activated, `Tab::shown`); rendering lives in `crates/app/src/ui/panes.rs` (panes, dividers,
dragging) and `ui/tabs.rs` (tab strip). To see dragging without a mouse: `HOLT_DEBUG_UI=split-drag`.

**Working on dictation and attachments.** Microphone and attachment buttons live in each pane header (`ui/panes.rs`) and act on its
visible tab; microphone state and fixed transcription target are pure and tested functions in `crates/app/src/dictation.rs`
(`Deck` methods in `deck/dictation.rs`); file handling (copying to `data/attachments`, typing `@path` or the path,
`input::paste_bytes` without Enter) and drop zone (`resolve_drop_zone`, `attach_dropped_files`) live in `deck/attachments.rs`, and
drag highlighting lives in `ui/overlay.rs`. The "New task" form attaches files through the same mechanism (`util::append_brief_attachments` appends paths to the brief).

**Working on the Git tab.** Operations live in `crates/core/src/git.rs` (`impl Core`, with safe environment in
`base_command`); state and reloads in `crates/app/src/git_ui.rs`; rendering in `crates/app/src/ui/git.rs`
(drawing only collects actions and applies them at the end). New git command: always `args([...])` with `--` before
paths, never a shell; test output parsing with sample bytes and the flow in `tests/git.rs`. To view
without a repository: `HOLT_DEBUG_UI=git-demo` (or `git-diff`, `git-branches`, `git-norepo`, `git-nogit`).

**Working on the GitHub tab.** Fetching lives in `crates/core/src/github.rs` (`impl Core`: `github_refresh`,
`github_load_pr`, `github_load_run_jobs`, `github_load_job_log`, all on dedicated threads returning
`CoreEvent::Github`); API parsing is pure and tested with sample JSON (`parse_pulls`, `parse_issues`,
`parse_workflow_runs`, `parse_workflow_jobs`, `tail_log`, `parse_github_remote`, `parse_credential_fill`).
The token never leaves `SecretToken`: do not put it in logs, error `format!`, or config. State and event handling
live in `crates/app/src/github_ui.rs`; rendering in `crates/app/src/ui/github.rs`. Prompt messages
sent to agents are built in core (`format_pr_prompt`, `format_issue_prompt`, `format_failed_job_prompt`) and
delivered via `Deck::send_to_instance` (opens tab if missing and queues like any message from another
agent, with `request: 0`). To view without network: `HOLT_DEBUG_UI=github-demo` (or `github-issues`,
`github-actions`, `github-menu`, `github-norepo`, `github-error`). To test real network and login:
`cargo test -p holt-core --test github -- --ignored --nocapture`.

**New learning.** Create `memory/learnings/<name>.md` (format and criteria in `README.md` of the folder). Invalid frontmatter is
ignored with a log warning, and the test `every_learning_in_the_repository_is_valid` (`learnings.rs`) fails if any file in the folder fails
validation or if a literal `touches` points to a non-existent file.

**Working on tasks.** Model and lifecycle live in `crates/core/src/tasks/` (`mod.rs`, `lifecycle.rs`, `merge.rs`, `finish.rs`,
`store.rs`, `text.rs`, `git_ops.rs`, and `stash.rs`, plus `scope.rs` and `capability.rs`; git uses the safe runner in `git.rs`); routes
and permissions live in `bridge.rs` (`tasks_route`, `task_permission`); the command lives in `crates/cli/src/bin/ask-agent.rs` (`parse_task`, with
tests). Pure text (slug, branch, agent messages, `exclude`) has tests in `tasks/mod.rs` and `tasks/text.rs`; flow with real git
in `tests/tasks.rs`, `tests/tasks_bridge.rs`, and `tests/tasks_queue.rs` (fixture in `tests/support/tasks_env.rs`; git identity
comes from test's `GIT_CONFIG_GLOBAL`). Changed what the agent receives at startup? That is `tasks::start_message` (in `tasks/text.rs`).

**Working on the Tasks tab (UI).** State and actions live in `crates/app/src/tasks_ui.rs`; rendering in
`crates/app/src/ui/tasks.rs` (rendering only *collects* clicks in `Act`; `apply` executes at the end of the frame).
Every core task call runs git, so goes to `Deck::spawn_bg` and returns as `UiMsg::Tasks(generation, TaskReply)`
(generation increments on each project switch and drops replies from previous one); list reloads when opening tab, on every
`CoreEvent::TasksChanged`, and after each action. `tasks_flow_tests.rs` exercises this flow with real core and git. The badge number on the tab icon is `TasksUi::review_count`. Which buttons each
state has is `actions_for` (pure function, with tests); list grouping, diff summary (`parse_diff_stat`), and
timestamp strings as well. Full diff comes from `Core::task_diff` and opens in the center (`Deck::docs_open_task_diff`, see "Working on
Documents mode"), rendered by `ui::git::diff_lines_at` (`ui/diff_view.rs`), the same renderer as Git tab (without per-hunk buttons). Reopening a task tab is `Deck::open_task_tab` (registers instance
again; terminal opens in worktree automatically). To view without git or agents: `HOLT_DEBUG_UI=tasks-demo` (or
`tasks-empty`, `tasks-detail`, `tasks-running`, `tasks-failed`, `tasks-confirm`, `tasks-discard`,
`tasks-return`, `tasks-error`, `tasks-busy`, `tasks-new`, `tasks-new-agent`, `tasks-new-model`; `debug_tasks` in `debug_ui.rs`).
For minimum sidebar width, set `{"rightPanelWidth": 280}` in `cfg/config.json` inside `HOLT_HOME` folder.

**Working on Documents mode.** State and pure logic live in `crates/app/src/docs.rs` (`Docs`: open/focus/close per project, deduplicated;
`diff_files`: file list of a `GitDiff`; `DocTab::apply`: what each type displays upon receiving data), with
tests; rendering in `ui/docs.rs` (toggle is called by sidebar and, when collapsed, by title bar; `ui/mod.rs` picks between `panes::draw` and `docs::draw`) and
tab strip in `ui/tabs.rs` (`strip_items`, same styling as agents). New document = a `DocKind`, the `DocState` it displays,
reading in `Deck::docs_load` (in a thread, returns as `UiMsg::Doc`), and body in `ui/docs.rs::document`. The file comes from
`Core::read_text` (1 MB, `safe_rel`); diff comes from `Core::task_diff`, `git_commit_detail`, or `git_diff`; markdown is rendered by
**`egui_commonmark`** (version follows egui's: 0.25 matches egui 0.36; see `memory/learnings/egui-commonmark-follows-egui.md`).
Non-negotiable rules: with center in Documents **no `fit_tab`** runs (PTY does not change size), terminal receives no keyboard
or IME (`handle_key`, `handle_ime`), and turn end marks `Tab.done` even on active tab. The watcher (`CoreEvent::WorkspaceChanged`) reloads
open files (`docs_reload_files`). `tasks_flow_tests.rs` covers flow with real core and git. To view without dependencies:
`HOLT_DEBUG_UI=docs-demo` (or `docs-demo-file`, `-md`, `-report`, `-unreadable`, `-many`, `-badge`, `-edit`, `-find`, `-diff-find`,
`-external`, `-confirm`, `-large`, `-md-code`; `docs_demo.rs`).
The editable file (`Source`), saving (`docs_save`, `write_text`), and close prompt are in `ARCHITECTURE.md` ("Documents mode and editor") and `memory/learnings/documents-editor.md`.

**Plugging in RAG (future).** Implement `ContextProvider` trait (`context.rs`) and register via `core.register_context_provider(...)`. Headless runner already queries registered providers.

**Adding a test.** Place a `#[test]` in a `#[cfg(test)] mod tests` block at the end of the file. For end-to-end tests, use `crates/core/tests/`.
To invoke the bridge as a tab, obtain its token with `spawn_spec_for(&ident).env["HOLT_TOKEN"]`
(see `tab_token` in `tests/bridge.rs`); with master token, body `from` is accepted. Tests initialize
`Core` with `system_dirs: false`, so as not to depend on CLIs installed on the machine.

## 4. Working with AI agents on the project

- Always ask agents to run `cargo fmt`, `cargo clippy ... -D warnings`, and `cargo test --workspace` before concluding. That is the same as what CI runs.
- Changes to `Core` (lib.rs) affect the UI. Ask for small changes and one area at a time (core **or** app).
- **CI** (Continuous Integration) is GitHub's workflow (`.github/workflows/ci.yml`). On each push it compiles and tests on Windows, Ubuntu, and Fedora (Fedora in container, build and tests only), marking the commit/PR with ✅ or ❌. If ❌, open the run log to view the error. What has been verified on Linux is in [LINUX.md](LINUX.md).
- To debug visually without opening the window: `HOLT_SMOKE=<folder>` saves a `screen.png` capture and exits the app (without downloading Whisper, to avoid initiating 1.6 GB in every temp folder). There are also `HOLT_DEBUG_FEED=1`, `HOLT_DEBUG_FAKE_SHELL=1`, `HOLT_DEBUG_PTY=1` (records raw PTY output of each tab in `data/pty/`, see `terminal/record.rs`; tab right-click menu toggles same recording without env var), and `HOLT_DEBUG_UI=…` (`tasks-demo`, `tasks-detail`, `tasks-new`, `newtab`, `newtab-models:<agent>`, `usage-demo`, `usage-panel`, `git`, `git-demo`, `git-diff`, `github-demo`, `split-drag`, `docs-demo`, `docs-demo-file`, `docs-demo-md`, `docs-demo-report`, `docs-demo-unreadable`, `docs-demo-many`, `docs-demo-badge`, `drop`, `drop-pane:<n>`, `drop-task`, `mic-recording`, `target`, `attach`, `text`, `download`…; list in `debug_ui::apply`, `crates/app/src/debug_ui.rs`). On Linux, run under `xvfb-run`.
- Logs: run with `RUST_LOG=debug` to view detailed messages in console (in debug builds).

## 5. Known limitations

The list is in [ideas.md](ideas.md), section "Known limitations"; what has not yet run on Windows is in [manual-tests.md](manual-tests.md).

## 6. Name history

The app was CommandDeck, then AgentDeck, and is now **Holt** (task `t1`, 2026-10-29). There is no compatibility left with
the old names since 2026-10-29 (task `t3`): no config copy, no `data/` entry renames, no old env prefix, header, lock,
`AGENTS.md` marker or worktree root. An install from before the rename must be migrated by hand.
`memory/history.md` keeps describing the past under the old names. With `HOLT_HOME` the app is isolated and reads no user config.

**After updating (user steps):** rename the GitHub repository to `pattrickn0/Holt` (GitHub redirects the old URL) and update
`origin`; optionally rename the local folder to `~/Holt` and rebuild so the menu entry points at the new path.

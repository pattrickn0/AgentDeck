# Ideas and known limitations

What is **not** among the next steps. Not a commitment: an idea only becomes work when it enters
[pendencias.md](pendencias.md), which holds at most 10 items (user review on 2026-10-24). Upon entering there, it leaves here.
A limitation that starts getting in the way during actual use also moves up there.

## Ideas

### Tasks desk and models

- **Acid test on a project other than AgentDeck** (user, 2026-10-24): a Godot game, after system tray and conversation
  resumption. So far the app was only used to build itself (Rust, full `memory/`, agents familiar with the repository).
  Points to observe: memory starting empty (do agents create learnings?), `--outline` without GDScript, reimporting the
  `.godot/` folder in each new worktree, reviewing `.tscn`/`.tres` and binary assets, running tests with `godot --headless`,
  and scope queue behavior with files everyone touches (`project.godot`, the main scene).

- **Messages to the agent do not mention validation:** `start_message` does not disclose that, after merging, the task can still
  come back rejected; it only finds out when `reject_message` arrives. Check in actual use if this is needed.
- **Run project tests upon receiving the report**; **edit a draft task**.
- **"How to validate" in the brief** (from OpenSpec, 2026-10-24): an optional section with WHEN/THEN scenarios; the report
  specifies which test covers each, and "awaiting validation" shows them as checkboxes (the format of `testes.md`).
  Optional: making it mandatory would create unnecessary bureaucracy for small tasks.
- **Project skills for the 4 official CLIs** (2026-10-24): each CLI searches for skills in a folder (`.claude/skills`,
  Antigravity to be confirmed, `.agents/skills` in Codex, `.opencode/skills`); AgentDeck would provide a single folder for all.
- **Model selector showing only what the plan allows** (`t14`): the list comes from `agy models`/`opencode models` (what the
  CLI knows, not what the plan allows). On 2026-10-24 the user stated that in practice it already works this way; reopen if a
  model without access appears. If implemented, the same filter applies to `Core::models_inventory`.
- **Model per instance, later:** switch the model of an open tab; Codex models list under "+ New agent"; free-form field for
  models outside the list; edit `roleModels` via the UI.

### Documents Mode, phase 2

- Side-by-side diff; persist document tabs (currently lost on app exit); document side-by-side with an agent (see
  [GERAL.md](GERAL.md), "Documents").
- Editor (`t39`, intentionally deferred): auto-indent, replace (`Ctrl+H`), go to line (`Ctrl+G`), right-click context menu on
  diff and large files, horizontal scrolling, mixed CRLF/LF and non-UTF-8 files (currently read-only), search in formatted
  markdown and in brief/report, search by occurrence in diff, syntax highlighting inside markdown code blocks, more
  languages (see `memory/aprendizados/editor-de-documentos.md` before replacing with `syntect`).
- Explorer: mark modified files (git), filter/search, open files of a task from its detail view.
- Diff: layout hash (`content_hash`, `ui/diff_view.rs`) scans the entire diff every frame; compute once on load if a multi-megabyte
  diff impacts performance.

### Interface

- **English in the interface** (was in `pendencias.md`; removed on 2026-10-24 to make room for Windows and Linux items).
- **Themes** (user request, 10/18; set aside on 2026-10-24): replace `theme.rs` constants with a runtime palette selected
  in settings, including background and the 16 terminal colors (`terminal/colors.rs`).
- Color emojis (egui only supports monochrome); hover animation on empty state.
- Split screen: drag a tab to another window, maximize a window, keyboard shortcut with collapsed sidebar, per-tab cache if
  repainting is heavy with 5–6 windows.
- Projects: file tree and Git persist state per project; `Deck` logic in pure testable functions (like `split.rs`); UI tests
  (egui) for right panel, Git tab, and split screen.
- Antigravity usage: refresh in-memory token when CLI is closed (currently shows "no data" after ~1 h); unobtrusive warning when
  internal API repeatedly fails.

### Memory

- `--outline` in JS/TS, Python, and PowerShell/shell; measure in practice whether agents run `--learn`/`--outline`.
- General `ContextProvider` (postponed on 2026-10-23; reopen only if telemetry shows agents aren't prompting for it), and RAG
  powered by it.
- Agent profiles in files (`prompts/_comum.md` and `prompts/<agente>.md`), editable without recompilation.
- Current situation with recent decisions; warning for `touches` pointing to renamed/deleted files and a mechanism to retire
  learnings; new learning from a branch visible before merge.

### Bridge and GitHub

- `ask-agent --cancel <n>`; automatically redeliver responses left in `--inbox`; pre-authorize `ask-agent` in CLIs requiring in-tab
  approval (currently only Claude launches with `--allowedTools`).
- In headless mode, aider (`--yes-always`) and interpreter (`-y`) run without approval: evaluate more restricted modes.
- GitHub via the app: comment on and approve PRs, create issues, rerun jobs; exercise `gh auth token` pathway.
- `commit.gpgsign` with graphical pinentry: allow password dialog to appear instead of rejecting.

### Systems

- Linux: run genuinely on Wayland (GNOME, KDE) and Fedora Workstation; WSLg via X11 (maximized window, scaling, keyring
  quota; [LINUX.md](LINUX.md), 2.8); packages (`.deb`, `.rpm`, Flatpak, AppImage, `.desktop`); dictation with GPU;
  warning when missing `xdg-desktop-portal`/`xdg-utils`; native Linux Git/GitHub messages.
- **Mobile** (much further ahead, once the app is stable): monitor and communicate with agents away from the computer.

## Known limitations

For anyone modifying the code; these are not tasks.

- **Claude Code duplicates conversation in history on every width change** (Windows): it clears only the screen and reprints,
  without `ESC[3J`. The PTY only resizes after window dimensions remain stationary for 150 ms (one drag = one copy); on tab
  switch in Focus mode, for 600 ms, and the next tab resize waits for reprinting to finish (300 ms quiescent output, 3 s ceiling).
  History also accumulates 1 to 3 repeated lines across width changes. See
  `memory/aprendizados/claude-code-sem-esc3j-no-windows.md`.
- **Bridge does not detect client (`ask-agent`) disconnecting** during wait (`tiny_http`); the question remains pending until
  answered or timing out after 10 min. Undelivered answers remain in `ask-agent --inbox`.
- **Tab agent not responding via `ask-agent`** (weak model, or user took over the terminal): question expires after 10 minutes,
  notifying the caller in writing.
- **Turn hooks only in Claude and Codex**; others rely on output heuristics (1.5 s idle / 30 s in queue).
- **Bridge instructions are frozen at startup**: an agent launched before a new version is unaware of new commands until the tab
  is reopened; team notifications only arrive with the next message.
- **Bridge without UI runs in headless mode** (does not appear in tabs).
- **`AGENTS.md` in non-active projects** is only rewritten when opening their next tab.
- **Antigravity quota comes from an internal API** (`v1internal:retrieveUserQuotaSummary`) that may change without warning;
  **Codex usage** was only tested with sample files.
- **Select dropdown, spinner, and tooltips are custom-drawn**; blur behind drag overlay is approximated.
- **Out of scope** (2026-10-23): dedicated Ollama agent and "local AI as an internal service"; local models enter through a
  supported CLI (see [AGENTDECK.md](AGENTDECK.md), "What AgentDeck is and is not").

# ⚡ Holt

**A desktop terminal where your AI agents work as a team.** Claude Code, Codex, Antigravity, and OpenCode
run side by side, talk to each other, and receive isolated tasks that you review and merge, without losing track of anything.

![Holt with Claude, Codex, and OpenCode in Highlight mode and the tasks desk on the right](docs/screenshots/main.png)

100% Rust, native for **Windows** and **Linux**. MIT License.

---

## Why it exists

Each agent CLI is good on its own, but working with several at the same time turns into a pile of loose terminals: you
copy text from one to another, lose track of what each one is doing, and hope two agents don't touch the same
file.

Holt brings everything together in one window and gives agents what they need to collaborate:

- **A way to communicate.** Inside any tab, the agent runs `ask-agent codex "review this"`. The message is
  typed into the other agent's terminal, and the reply returns to whoever asked. You see everything happening and can
  intervene in any tab.
- **Isolated, reviewable work.** A task gets its own branch and folder (git worktree). The agent works there,
  delivers a report, and you inspect the diff and decide whether to merge, return with adjustments, or discard.
- **Project memory.** What the project has learned (pitfalls, decisions, conventions) lives in versioned files,
  and the agent receives the relevant slice for the files it will touch.

No invisible work: every agent has a real terminal tab, and none launches hidden subagents.

## What you can do

### Agents

- **Real terminal tabs** (ConPTY on Windows) for each agent, restored when the app reopens.
- **Multiple instances per agent**, with role and name: `claude`, `claude@dev`, `claude@dev@ana`. Each has its own
  **model** (`opus`, `sonnet`, `gpt-6-luna`, `opencode/deepseek-v4.1-flash-free`…), displayed next to the name.
- **Team**: drag one agent onto another in the sidebar to define boss and subordinates.
- **Plan usage** in the title bar: each agent's 5-hour session and weekly limits, with warnings starting at 80%. An exhausted
  model receives no requests.
- **Holt instructions in every tab**, delivered via each CLI's native channel, without depending on your project's `AGENTS.md`.
- **No blocking prompts**: folders opened as projects are automatically registered as trusted in Claude, Codex, and
  Antigravity, so tabs opened in the background won't stall on "Do you trust this folder?".

### Tasks desk

The **Tasks** tab in the right sidebar is where major work happens:

```
draft ─→ in progress ─→ in review ─→ awaiting validation ─→ completed
  │           ↑              │               │
  └→ queued ──┘              └ return ↺      └ reject ↺ (returns to the same agent)
```

At any stage before completion, the task can be discarded, and the branch and folder are removed along with it.

- **Agent selection by capability and quota.** State the size of the work (`simple`, `medium`, or `complex`). The
  desk filters models capable of handling it, based on public benchmarks and historical task outcomes, and among them
  chooses the one with the most remaining quota headroom.
- **Scoped queuing.** A task can declare the paths it will touch. Another task needing the same paths waits
  **in the queue** and starts automatically when the first one finishes.
- **Real review**: report, syntax-highlighted diff, and timeline of messages the agent exchanged.
- **Assisted merge**: if the merge is rejected (uncommitted changes, conflicts), the window itself explains the
  cause and provides the way forward.

### Interface

- **Projects**: each project is a folder with its own agents. Switching projects hides the tabs of the previous one, which
  continue running.
- **Three layouts** for the center: **Tabs** (a single window), **Split** (one window per agent type, in a grid), and
  **Highlight** (the active tab's window large, the others stacked alongside).
- **Integrated conversation**: agent-to-agent messages in real time, read-only.
- **Documents**: the center toggles to read and **edit** files with syntax highlighting and search, and to view full-width
  diffs and reports (`Ctrl+Shift+D`).
- **Git** (changes, hunk staging, commit, branches, merge and rebase, conflicts, stash) and **GitHub** (PRs with
  checks, issues, Actions, and the log of the failed job; one click sends the failure to an agent).
- **Attachments** via button, drag and drop, or `Ctrl+V` with images, and **optional voice dictation** with local whisper.cpp
  (CUDA, Vulkan, or CPU). Audio is deleted immediately after transcription.
- **System tray**: × hides the window while agents keep working; opening the app again brings back the same window.

## Supported CLIs

Install whichever you want to use. Holt uses whatever it finds installed.

| CLI | Installation |
| :-- | :-- |
| Claude Code | `npm install -g @anthropic-ai/claude-code` |
| Codex | `npm install -g @openai/codex` |
| Antigravity | Windows: `irm https://antigravity.google/cli/install.ps1 \| iex` · Linux: installer at https://antigravity.google |
| OpenCode | `npm install -g opencode-ai` (includes free models; also runs local models) |

Also open as tabs, with less integration: Aider, Goose, GitHub Copilot CLI, Amazon Q, Open Interpreter, and the system's
standard terminal.

## Installation and running

There is no installer yet: the app runs from the folder where it was built.

### Windows

Prerequisite: [Rust](https://rustup.rs) 1.89 or newer.

```powershell
git clone https://github.com/pattrickn0/Holt.git
cd Holt
cargo build --workspace --release
.\target\release\holt.exe
```

### Linux

Prerequisites: [Rust](https://rustup.rs) 1.89 or newer and sound and keyboard libraries.

```bash
# Ubuntu / Debian
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev
# Fedora
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel
# openSUSE
sudo zypper install gcc gcc-c++ make pkgconf-pkg-config alsa-devel libxkbcommon-devel

git clone https://github.com/pattrickn0/Holt.git
cd Holt
cargo build --workspace --release
```

The release build also adds Holt, with its icon, to the app menu (`~/.local/share/applications`); open it from
there or run `./target/release/holt`. To skip that, build with `HOLT_NO_DESKTOP_ENTRY=1`; to remove it,
`scripts/install-desktop.sh --uninstall`.

- To open folders and links and choose files: `xdg-utils` and `xdg-desktop-portal` with a backend
  (`xdg-desktop-portal-gtk` in GNOME/XFCE, `xdg-desktop-portal-kde` in KDE).
- Without sound or microphone (drops the ALSA requirement): `cargo build -p holt --no-default-features --release`.
- Tested on X11 (Ubuntu) and on GNOME/Wayland (openSUSE Tumbleweed); Fedora is checked in CI, and in WSLg the app opens
  via X11 automatically. If the window misbehaves on Wayland, try `WINIT_UNIX_BACKEND=x11`. Details and limitations in
  [memory/LINUX.md](memory/LINUX.md).

### The two executables

The build produces `holt` (the app) and `ask-agent` (the command that agents use). They need to stay **in the same
folder**. Inside tabs, the app automatically adds `ask-agent` to `PATH`.

## Getting started

1. Open the app and click **Open folder…** to create the first project (your code folder).
2. Under **+ New agent** in the sidebar, open Claude, Codex, or another installed CLI. In Claude, Antigravity, and
   OpenCode, the `›` arrow selects the model.
3. Chat with the agent in the tab's terminal. To have it call another agent, just ask: it knows how to use `ask-agent`.
4. For larger work, use **+ New task** in the Tasks tab: describe the request, choose the level, and let the desk
   select the agent. Once it delivers, review the diff and merge.

## `ask-agent`: the agents' command line

Agents use these commands on their own (instructions are delivered to each tab). You can also run them in a standard
terminal tab.

```bash
# Conversation
ask-agent codex "Review the parse_config function in src/config.rs"
ask-agent --model sonnet claude@dev "Implement item 3"      # opens claude@dev with the sonnet model
ask-agent --reply 7 "Done: the src/ folder was split"       # replies to question #7
ask-agent --boss "Finished the review"                      # reports to boss
ask-agent --inbox                                           # replies that arrived later

# Who is there
ask-agent --list                                            # open agents and instances
ask-agent --team                                            # your boss and team
ask-agent --models                                          # models for each agent, quota, and capability

# Tasks
ask-agent --task new "Login screen" --level complex --scope src/ui --start --file brief.md
ask-agent --task list
ask-agent --task merge t3                                   # merges and leaves awaiting validation
ask-agent --task validate t3                                # completes and cleans up the worktree

# Project memory
ask-agent --learn src/config.rs                             # what the project has learned about this file
ask-agent --outline src/config.rs                           # signatures with line numbers, to read just the right slice
```

Accented text works better via file (`--file`): on Windows, PowerShell standard input replaces accents with `?`.
The complete reference is in [memory/OVERVIEW.md](memory/OVERVIEW.md#2-what-you-can-do).

## Shortcuts

| Shortcut | Action |
| :-- | :-- |
| `Ctrl+T` | new tab of focused type |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | next / previous tab in the window |
| `Ctrl+Alt+arrows` | focus neighboring window |
| `Ctrl+B` | show or hide sidebar |
| `Ctrl+Shift+D` | toggle center between Agents and Documents |
| `Ctrl+Shift+M` | dictation in visible tab |
| `Ctrl+S` / `Ctrl+F` | save / find in open document |
| `Ctrl+/` | list shortcuts |

With the terminal focused, `Ctrl+B` goes to the tab's program (useful in tmux). The "App shortcuts above the terminal" option
in the ☰ menu reverses this.

## Data and privacy

- **Nothing passes through a Holt server**, because it does not have one. The CLIs themselves communicate with
  the cloud; quota reading uses credentials they already store on your machine.
- Inter-agent communication is a local HTTP bridge, strictly on `127.0.0.1`, with a token per tab: an agent cannot
  impersonate another.
- Dictation audio is deleted immediately after transcription. Attachments and long prompts are retained only from the
  last 5 sessions.
- Opening a folder as a project counts as trusting it: the app registers it as trusted in Claude Code
  (`~/.claude.json`) and in Antigravity (`~/.gemini/antigravity-cli/settings.json`). Your home folder and drive roots
  are never registered.
- Runtime data lives in `data/`, next to the executable (or in `HOLT_HOME`). Configuration is stored in
  `%APPDATA%\Holt\config.json` on Windows and in `~/.config/Holt/config.json` on Linux.

## Documentation

Project documentation lives in [`memory/`](memory/OVERVIEW.md), versioned alongside the code:

- [OVERVIEW.md](memory/OVERVIEW.md): the starting point, covering how the app works, what you can do, and the documentation map.
- [ARCHITECTURE.md](memory/ARCHITECTURE.md): crates, workflows, and the inter-agent bridge contract.
- [MAINTENANCE.md](memory/MAINTENANCE.md): how to build, test, and maintain, with a hands-on introduction to Rust.
- [HOLT.md](memory/HOLT.md): product vision and direction.
- [backlog.md](memory/backlog.md) and [history.md](memory/history.md): what comes next and what has already been done.
- [learnings/](memory/learnings/README.md): pitfalls and decisions that the code does not show.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Before submitting, run `cargo fmt --all`,
`cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace`; CI verifies all three on Windows,
Ubuntu, and Fedora.

## License

MIT. See [LICENSE](LICENSE).

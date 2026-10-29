# 🤝 Contributing Guide

## Environment

- Windows 10/11 64-bit (primary target). Linux (Ubuntu and Fedora) works for development.
- [Rust stable](https://rustup.rs), 1.89 or newer: `rustup update` occasionally.
- Linux: `sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev` (Ubuntu/Debian) or
  `sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel` (Fedora). To run with
  a window and dialogs, see also `xdg-utils` and `xdg-desktop-portal` in the [README](README.md#linux).
  GTK3 is not needed.

```bash
git clone https://github.com/pattrickn0/AgentDeck.git
cd AgentDeck
cargo run -p agentdeck          # runs the app (debug mode)
```

## Before opening a PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

CI (`.github/workflows/ci.yml`) runs this on Windows and Ubuntu; Fedora runs build and tests in a container
(formatting with `cargo fmt --all -- --check` only in the main job; code style is in `rustfmt.toml`). What has already
been verified on Linux, what is pending, and how to test manually are in [`memory/LINUX.md`](memory/LINUX.md).

## Project principles

1. **Privacy:** `.wav` audio is deleted immediately after transcription; attachments and long prompts are retained for the last 5 sessions only.
2. **Isolated folder:** each agent works in the **project** folder in which the tab was opened (or, in a task, in its own worktree, `<repo>/.agentdeck/worktrees/<id>`), never in the app's files. The app only creates `AGENTS.md` inside the app's own folder (`workspace/`); in project folders it only updates the block of an already existing `AGENTS.md`, and never writes to the user's home directory.
3. **Secure local bridge:** `127.0.0.1` only, a random token **per agent process** (the bridge knows who is asking via the token) and `Host` header validation. Text coming from another agent never goes raw to a terminal (`util::terminal_text`) or to the command line of a `.cmd` shim.
4. **UI never blocks:** everything that can block (processes, network, transcription) runs in a thread.
5. **Language:** code comments, docs and commit messages in English. User-facing text goes through `t!("English text")`
   with a pt-BR entry in `crates/core/src/i18n/pt_br/<area>.rs` (the app is English by default, Portuguese is an
   option). Agent-facing text (`ask-agent` output, instructions, briefs, bridge responses) is English only and never goes
   through `t!`. See [`memory/learnings/i18n-english-key-and-area-files.md`](memory/learnings/i18n-english-key-and-area-files.md).

6. **Documentation alongside code:** changed behavior, protocol, or a pending item? Update the corresponding
   document in [memory/](memory/OVERVIEW.md) in the same commit. A completed pending item moves from `backlog.md` to
   `history.md`.
7. **Project memory:** anything that is not obvious in the code and cost time (a pitfall, a decision, a convention)
   becomes a file in [memory/learnings/](memory/learnings/README.md), in the same commit.

See [memory/MAINTENANCE.md](memory/MAINTENANCE.md) for the code map and common recipes, and
[memory/OVERVIEW.md](memory/OVERVIEW.md) for the overview and documentation map.

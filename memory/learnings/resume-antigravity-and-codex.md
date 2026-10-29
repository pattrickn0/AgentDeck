---
title: Resuming Antigravity and Codex: the id is read from agy's log and from Codex's notify (not from us)
summary: agy --conversation <id> with the id from its --log-file; codex resume <id> with the thread-id of the notify JSON, filtered by its rollout (a hidden title thread calls notify too); bare ask-agent in notify was not found on Windows.
touches: [crates/core/src/resume.rs, crates/core/src/agents.rs, crates/core/src/bridge.rs, crates/cli/src/bin/ask-agent.rs, crates/app/src/deck/frame.rs]
---
Part B of the resume work (`resume-session-flags-per-cli.md` has the base and Claude/OpenCode). These two CLIs choose the
id of a conversation themselves, so the app learns it after the launch (`resume::learns_id_later`). Checked on this machine
on 2026-10-29 (`agy` 1.4.10, Codex 0.164.0):

- **Antigravity:** `--conversation <id>` resumes (print mode and TUI; the TUI redraws the old messages and goes on).
  `--continue`/`-c` is "the most recent conversation": never used. An unknown id only prints `warning: conversation "..." not
  found` and opens a new one (exit 0), so `resume::agy_has_conversation` (`~/.gemini/antigravity-cli/conversations/<id>.db`)
  decides whether a launch counts as a resume. `--log-file <path>` is truncated at each start and creates the folders. The
  TUI creates the conversation with the **first message**, not at startup (an idle tab's log names none), and the log then has
  `Created conversation <uuid>` (also after `/new`), `Streaming conversation <uuid>` and, for `--conversation`,
  `Resuming conversation <uuid>`. The last of those lines is the tab's conversation (`agy_session_in_log`). The log is
  `data/agy-logs/<tab>.log`, read by `Deck::tick` every 2 s from where it stopped (`Core::sync_agy_session`); it is removed
  before each launch, and the tab remembers its path (`SpawnSpec::session_log`), so a rename while it runs does not lose it.
- **Codex:** `codex resume <id>` (a subcommand: it goes before every other option; the others, `-c`, `--model`, `--add-dir`,
  are accepted after it). `resume --last` is never used. The id is the `thread-id` of the JSON `notify` appends as the **last
  argument**: `{"type":"agent-turn-complete","thread-id":"…","turn-id":"…","cwd":"…","client":"codex-tui","input-messages":[…],
  "last-assistant-message":"…"}`. A resumed thread keeps its id. An unknown id fails at once with `No saved session found
  with ID …` (exit 1), which the existing fallback (`Deck::reopen_without_resume`) turns into a fresh tab. The rollout is
  `~/.codex/sessions/<y>/<m>/<d>/rollout-<time>-<id>.jsonl` (`CODEX_HOME` overrides the folder).
- **Trap, Codex's title thread:** after the first message Codex runs a hidden thread to name the chat, and it calls `notify`
  too, with a **different** `thread-id` (and `input-messages` starting "Generate a concise, single-line task title…"). That
  thread has no rollout and cannot be resumed, and its notification comes second, so saving every `thread-id` would store
  the useless one. `Core::set_tab_session` therefore accepts a Codex id only if `resume::codex_has_session` finds the rollout.
- **Trap, `notify` on Windows:** the program in `notify=[…]` was **not found** when given as the bare name `ask-agent`, even
  with it first in `PATH` (also `ask-agent.exe`); the same command with the full path ran, with the tab's environment
  (`AGENTDECK_*`) and the JSON intact. `codex_notify_arg` passes `<bin dir>/ask-agent[.exe]` (bare name only if the file is
  not there). It is a TOML array written as JSON, which Codex parses.
- **Trap, the npm shim:** `notify` needs the direct executable (a `.cmd` loses the quotes: `cmd-shims-on-windows.md`).
  `agents::codex_native_exe` finds the real `codex.exe` the shim's `bin/codex.js` would run
  (`node_modules/@openai/codex/node_modules/@openai/codex-win32-x64/vendor/x86_64-pc-windows-msvc/bin/codex.exe`, then the
  hoisted `node_modules/@openai/codex-win32-x64/…`, then `@openai/codex/vendor/…`; `-win32-arm64` on ARM) and `resolve_cli`
  launches it directly; the shim is the fallback for any other layout. Same flow on Unix, where Codex is already a direct
  executable.
- A Codex tab closed before its first reply has no id (it is only known when the first turn ends): it reopens as a plain
  `codex`. `/new` and `/resume` inside either TUI are followed (the next log line or notify names the new conversation).

**Why:** backlog item 3 (T4), part B: Antigravity and Codex tabs also opened an empty conversation after a restart.
**How to apply:** when testing Codex in a PTY, answer ConPTY's cursor query (`ESC[6n` → `ESC[1;1R`) or the TUI never draws,
wait ~3 s between the pasted text and Enter (`codex-windows-enter-after-paste.md`), and pass `-c projects={'<dir>'={trust_level='trusted'}}`
for the trust prompt; for `agy` use `--settings <file>` with `trustedWorkspaces` instead of editing the user's `settings.json`
(PowerShell 5.1 strips the inner quotes of native arguments: write them as `\"`). Do not add a "latest session" fallback
for either CLI.

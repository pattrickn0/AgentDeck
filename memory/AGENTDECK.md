# AgentDeck: vision and direction

**AgentDeck** is the development environment for AI agents. Since 2026-10-17 the
product has been driven by Claude (the orchestrator); the user follows along as client and collaborator and is the one who approves
what goes into `main` (`main` is the stable version; day-to-day work happens on `dev`).

## Who it is for

1. **For agents**, first. An agent here must be able to receive a task, work in isolation without stepping on
   others, know what the project has already decided, ask another agent for help, and deliver something reviewable. The explorer,
   Git, GitHub, and memory exist for this.
2. **For the human**, who needs to see what is happening without reading entire terminals: what each agent is
   doing, what is ready, what awaits review, and what went wrong.

## Principles

- **The unit of work is the task, not the message.** A task has a brief, an agent with a model, an
  isolated branch, a report, a diff, and tests. Conversation between agents is a means, not the end.
- **Everything visible.** An agent working appears in a tab; no invisible work that the human cannot follow.
- **Isolation by default.** Each task in its own worktree: parallel agents do not interfere with each other, and discarding
  an attempt costs nothing.
- **The right model for each job.** Opus to orchestrate and decide, Sonnet to develop, free models
  (OpenCode) for review and questions, always with findings verified. Subscription quota is the scarce resource.
- **Memory lives in files.** What the project decided and learned stays in versioned markdown (this folder), and the
  app delivers the right piece to each agent at the right time.
- **No freezing magic.** No hidden prompts, no frozen dialogs, no orphan processes; errors with explanation.

## What AgentDeck is and is not (user, 2026-10-23)

- **100% Rust**, no other technology (no Electron, Tauri, or web). Systems: **Windows and Linux**; macOS is not on the roadmap.
- **Revolves around agent CLIs** (Claude Code, Antigravity, Codex, OpenCode, and whatever exists), cloud or not:
  whatever is installed is used. **It is not a local model app**: the dedicated Ollama agent (`ollama-agent`) is removed; a local
  model remains possible through a CLI that supports it (e.g. OpenCode).
- **Visible agents**: when an agent needs another, it opens an instance in AgentDeck (tab) via `ask-agent` or the
  tasks desk, **not a CLI-internal subagent**, unless the user explicitly asks.
- **Three memories**: **general** (how to use AgentDeck, identical for everyone, delivered by the app to every CLI in any
  project); **project** (`memory/aprendizados/` in the user's repository, versioned in it); and **local** (the
  user's and CLIs', which never goes into git).
- **MIT** license. Currently only the user develops; contributions from others, perhaps after release.
- **After `main`** (revised on 2026-10-24): stability and per-task timeline first, then the
  **installer**; English UI and themes were set aside (English left the list on 2026-10-24, in favor of the Windows and Linux items decided below) ([pendencias.md](pendencias.md)). **Much further ahead**, with the app very stable: monitor and talk to agents **via mobile**.
- Do not do for now: line-by-line comments on diffs to return to the agent ("return with text" is sufficient).
- **Officially supported CLIs** (user, 2026-10-24): **Claude Code, Codex, Antigravity, and OpenCode**. These are tested and
  fully functional (hooks, usage, resume). Others continue opening and working, but without warranty (usage bar
  or hooks may fail) until they enter official support, like Orca, which accepts any CLI.
- **Decided on 2026-10-24** (comparison with MonoCode and Orca; plan revised with `claude@arquiteto`): the X sends
  to the tray and opening again brings the instance back (like Discord); each tab resumes its conversation after
  closing or a crash; task phase comes solely from hooks (never self-declared by agent); durable mail (short
  message typed, long via notice); write scope per task with queue; memory gains a graph as an index (markdown
  files remain the source), measured against what already exists. Items are in [pendencias.md](pendencias.md).
- Third-party skills: only those that do not conflict with the rules above. From Superpowers 4 were brought in (`.claude/skills/`, with
  `ORIGEM.md`); those that instruct opening subagents, creating worktrees outside the desk, or storing separate plans were
  excluded. OpenSpec was not adopted (would be a second place for the plan).

## Direction (order decided by the orchestrator)

1. ~~LLM model per instance~~ (done on 2026-10-17).
2. ~~**Tasks desk**~~ (phases A and B done on 2026-10-17; "waiting for validation" stage arrived in `t10`; remaining items in
   [ideias.md](ideias.md); per-task timeline is in [pendencias.md](pendencias.md)) in place of integrated Conversation: create, start (worktree + branch + agent tab with model),
   monitor, receive report, review diff, merge, validate (or reject), or discard with a button. It is the workflow the
   orchestrator used to do by hand.
3. **Two-layer memory, our own.** **Phase 1 done** (tasks `t13`, `t15`, and `t16`): `memory/aprendizados/*.md` with `touches`, delivered
   at the start of each task and via `ask-agent --learn`; "Current situation" section (other tasks, their files, including uncommitted ones, and base commits); history of
   feedbacks and learnings for files cited in returns and rejections; `ask-agent --learn` with and without files. How it works: [ARCHITECTURE.md](ARCHITECTURE.md) ("Two-layer memory"). **Phase 2 done**
   (2026-10-23, `t40` and `t41`), **on demand**: `ask-agent --outline <file>` (signatures with line number) and, in `--learn` from
   any tab, the current situation and warning for files that another running task also touches. The general `ContextProvider`, which
   would push context across all channels, was postponed: [ideias.md](ideias.md), "Memory".
   The original plan, for reference: Long term: versioned markdown "learnings" in the repository, with
   frontmatter specifying touched files (`touches`), delivered to an agent when its task touches those files;
   and a file API summary (`skeleton` style) so the agent reads less. Short term: task brief,
   decisions, and recent activity delivered by the app (the "single context injection point", postponed; see "Memory" in
   [ideias.md](ideias.md)).
   - **Trail-Code evaluated on 2026-10-17 and not adopted as a dependency.** `skeleton` is good (−87% tokens), but
     `ask` is lexical and did not beat `grep`, `callers` does not resolve cross-crate method calls in Rust, `init`
     writes hooks into global `~/.claude` (affects all repositories) even with `--no-global`, and all its output
     instructs the agent to announce saved tokens to the user (third-party text steering the agent). We kept the
     ideas: versioned learnings with `touches` and API summary.
4. **Readable diff for the user to validate** (agreed on 2026-10-21): today the diff lives squeezed in the right sidebar. The center
   gains "document tabs" alongside terminal tabs, and a task diff (and later file editor and a more
   useful explorer) opens in them, in full-screen width. It is what was missing for the human to validate without leaving the app.
   Phase 2 (side-by-side diff, editor extras, explorer) is in [ideias.md](ideias.md).
5. **Rename to AgentDeck and visual identity** (agreed on 2026-10-22). **Done**: the name (task `t20`, with migration
   of legacy names: see [MANUTENCAO.md](MANUTENCAO.md), section 6) and visual design (`t21`–`t23`). **Themes** were set
   aside on 2026-10-24 ([ideias.md](ideias.md)).
6. **Replace `main` with `dev`**: **done on 2026-10-24**. `main` is the Rust rewrite; the TypeScript pilot
   was deleted. The next step is the "After `main`" block above.

---
title: --effort is only valid for agents that support effort in their tab
summary: `--task new --effort` with codex or opencode is rejected ("effort not applicable") and the task is not even created.
touches: [crates/core/src/models.rs, crates/core/src/tasks/**]
---
Codex does not support effort levels, and OpenCode only supports effort in headless mode (the tab does not receive it). With `--effort`,
`ask-agent --task new` fails before creating the task.

**Why:** on 2026-10-24, creating t49 (codex) and t50 (opencode) failed for this reason and had to be redone.
**How to apply:** pass `--effort` only to Claude tasks (and agents whose error messages do not reject it); for codex and
opencode, create without `--effort`.

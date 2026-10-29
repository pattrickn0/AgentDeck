---
title: A task agent does not request a merge
summary: The agent's work ends at --task done; merging belongs to whoever reviews.
touches: []
---
A task agent only queries tasks and delivers the report for its own task (`ask-agent --task done`). It does not ask
anyone, agent or user, to merge nor does it run `--task merge`; the bridge rejects (403) any other task action
coming from it (`task_permission`, `bridge.rs`).

**Why:** on the first real use of the board, `opencode@t2` asked Claude to merge its own task; the decision to merge
belongs to whoever reviews the diff, not whoever wrote it.
**How to apply:** finish with the report and wait. If something is missing, ask the boss with `ask-agent --boss`.

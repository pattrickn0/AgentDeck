---
title: AGENTS.md is rewritten by the app and must not block merging
summary: The app maintains a block in AGENTS.md; merge ignores it in the "dirty" check and preserves and rewrites it if the branch also modifies the file.
touches: [AGENTS.md, crates/core/src/workspace.rs, crates/core/src/tasks/**]
---
The app maintains a marked block in the workspace `AGENTS.md` with instructions for the bridge (Antigravity has no flag
for this). It only updates a file that already exists (`workspace::ensure_agent_context`), writing alongside and replacing, and
preserves CRLF. The only exception is creating one with only the block for a CLI that has no other channel (`create = true`; see
`instructions-per-cli.md`); the user's home folder never receives the file.

**Why:** the file changes on its own and appears as a modification in the main repository, and `git merge` refuses to merge
on top of a modified file that the branch also touches ("commit your changes or stash them").
**How to apply:** `task_merge` excludes `AGENTS.md` from the "dirty" check. If the task branch also touches the file and the
difference from HEAD is only the app's block (`workspace::only_marker_blocks_differ`), it saves the content, runs
`git checkout -- AGENTS.md`, merges, and rewrites the block afterwards, in any outcome (`workspace::restore_marker_block`).
If the user edited text outside the block, it refuses without discarding anything. Do not edit the AgentDeck block by hand or
include it in a task commit.

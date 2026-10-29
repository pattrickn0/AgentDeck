---
title: Inspecting another task's worktree without taking git locks
summary: To read status from another agent's worktree, use worktree_dirty_unlocked (--no-optional-locks), not worktree_dirty.
touches: [crates/core/src/tasks/**, crates/core/src/situation.rs]
---
A standard `git status` updates the index and creates `index.lock` in the worktree. If that directory belongs to another agent
running `git add`/`commit` at the same instant, their command fails with an `index.lock` error. "Current situation" inspects
other agents' worktrees (`task_situation`), so it uses `worktree_dirty_unlocked` (`git --no-optional-locks status`).
`worktree_dirty` remains in places that modify the current task (`task_done`, `task_merge`, `task_validate`).

**Why:** status queries are purely informational and must never interfere with concurrent commits.
**How to apply:** when reading state from a worktree that does not belong to the running task, use the unlock variant.
`git::base_command` already sets `GIT_OPTIONAL_LOCKS=0`; the explicit flag keeps the intent obvious and independent of environment setup.

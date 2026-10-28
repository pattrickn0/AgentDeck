---
title: Merge stash is shared across worktrees and must not delete anything
summary: "Stash and merge" restores by commit hash (stash apply <hash>, never pop stash@{n}), pre-validates using git merge-tree, and leaves it in stash if uncertain.
touches: [crates/core/src/tasks/**]
---
`task_merge_assisted(.., stash_main = true)` stashes uncommitted changes from the main repository using `git stash`,
performs the merge, and restores them. The stash stack is **shared across the main repository and all worktrees**, and an
improper `pop` leaves the directory with merge conflict markers.

**Why:** (1) another task or session might push to stash midway through, meaning `stash@{0}` is not
necessarily ours, and position can shift even between pre-validation and restoration: the code records the hash (`stash_push`),
restores via `git stash apply <hash>`, and only then drops the entry, finding its position again by hash (`stash_ref`) immediately
prior to `drop`. With `pop stash@{n}`, a concurrent `push` by someone else would cause the app to restore and drop another user's stash.
(2) restoring with conflicts leaves conflict markers in the folder and unmerged entries in the index; therefore,
`stash_pop` pre-checks without touching the filesystem via `git merge-tree --write-tree --merge-base=<stash>^1 HEAD <stash>`
(git 2.38+) before restoring. Upon conflict, or on git lacking `merge-tree --write-tree`, it leaves the stash entry intact and the
notice explains how to recover. (3) `base_command` sets `GIT_LITERAL_PATHSPECS`: magic pathspecs (`:(exclude)AGENTS.md`)
do not work, so files are passed via `--pathspec-from-file=- --pathspec-file-nul` (`diff --name-only -z` output).
(4) Stashing does not include untracked files; any untracked file that the branch also creates would be rejected by git
midway through merging, which is why `untracked_clash` rejects before stashing anything.

**How to apply:** when modifying merge logic, every code path after `stash_push` must call `give_back` (success,
aborted conflict, failure, `AGENTS.md` rejection). Only user action enables stashing; `task_merge` (CLI and
agents) never stashes anything. In tests, Windows git might restore files with CRLF (`core.autocrlf`): compare
after normalizing line endings.

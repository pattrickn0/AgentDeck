---
title: An approved merge that conflicted waits for the agent and merges by itself
summary: A conflict at merge time sends the task back to the agent with merge_approved set; task_done merges on redelivery (max 3 requests), and done refuses a branch that does not merge cleanly.
touches: [crates/core/src/tasks/**, crates/app/src/tasks_ui.rs, crates/app/src/ui/tasks.rs]
---
A task has three persisted fields for this: `merge_approved` (the reviewer already decided to merge), `conflict_asks`
(automatic requests so far) and `merge_stopped` (why an automatic merge did not happen, shown on the card).

- `task_done` refuses (409) a branch that `git merge-tree --write-tree` says conflicts with the base, so what arrives in
  review is mergeable at that moment (an old git without `--write-tree` skips the check).
- The base can still move before the merge. A conflict in `merge_locked` (UI "Merge" or `--task merge`) undoes the merge
  and, if tabs are on and `conflict_asks < MAX_CONFLICT_ASKS` (3), calls `return_locked` with `auto = Some(asks + 1)`:
  the task is `Running` with `merge_approved`, and the call answers `MergeOutcome { resolving: true }` (the CLI sees 409
  with the same text).
- `task_done` with `merge_approved` runs `auto_merge_locked` right after accepting the report. No stash is ever done
  automatically; a refusal leaves the task in `Review` with `merge_stopped`.
- `conflict_asks` only keeps counting while `merge_approved` is still set; a person's merge (flag clear) starts at 0, and
  giving up (third conflict) clears the flag so the red box and its manual button work as before.
- Returning with feedback (`task_return`), merging, or `task_cancel_auto_merge` drop the approval.

**Why:** the user complained that "most of the time" a delivered task showed the conflict box and they had to click "Ask
the agent to resolve the conflict" every time; the app already knew the fix.
**How to apply:** do not add a path that sets `merge_approved` without also sending the agent the request, and do not
let it persist in `Review` (the automatic merge either happens or the flag is cleared). The retry limit cannot be
reached with a real race in tests: they edit the task JSON and restart (`restart_editing` in `tests/tasks.rs`).

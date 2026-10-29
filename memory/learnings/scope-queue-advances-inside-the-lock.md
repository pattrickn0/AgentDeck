---
title: The scope-based queue advances inside the operations lock and Validating does not hold it
summary: A task leaving Running/Review frees the queue via advance_queue_locked (without re-acquiring the lock); awaiting validation holds no one.
touches: [crates/core/src/tasks/**, crates/core/src/scope.rs]
---
A task with a scope (`Task::scope`, list of paths; `scope.rs`) that overlaps with another `Running`/`Review` task transitions to
`Queued` during `task_start`. What moves it out of the queue is `Core::advance_queue_locked`, called at the end of `merge_task` (on success),
`task_discard`, and a failed `task_start`.

**Why:** `TaskStore::op` is a `parking_lot::Mutex` (non-reentrant) held across every lifecycle operation. Calling
`task_start` from within `merge_task` would deadlock permanently; hence `start_locked` and `advance_queue_locked` exist,
assuming the lock is already held. `Validating` intentionally does not block the queue: the code is already on the base branch, the next task
builds on it, and waiting for manual user validation would stall parallelism (rejection merely logs a notice in history if it returns with overlapping files).
`memory/**` and `Cargo.lock` (`scope::UNRESERVED`) are exempt from overlap detection and from "outside of scope" warnings.

**How to apply:** any new operation moving a task out of `Running`/`Review` (or causing it to fail) must call
`advance_queue_locked` at the end while still holding the lock; never call `task_start`/`tasks_advance_queue` from within another operation. The base branch
for a task leaving the queue is resolved when it starts (its worktree is branched from the base branch tip at that moment), not at task creation time. The UI
does not need to poll in `Deck::tick`: task startup emits the exact same `CoreEvent::Deliver` event as the Start button.

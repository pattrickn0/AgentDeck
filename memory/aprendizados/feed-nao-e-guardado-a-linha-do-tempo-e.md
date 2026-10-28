---
title: The feed (Integrated Chat) is not persisted; the task timeline is
summary: Deck::feed lives only in memory (500 entries); what persists is Task::messages, recorded via Core::feed.
touches: [crates/core/src/lib.rs, crates/core/src/tasks/**, crates/core/src/bridge.rs, crates/app/src/deck/**, crates/app/src/ui/tasks.rs]
---
Integrated Chat (`Deck::feed`) is an in-memory UI list capped at `FEED_MAX`; closing the app clears it. The
timeline of each task cannot depend on it: `Core::feed` (the sole entry point for `ask-agent`: `bridge::Caller::feed`,
`answer_lost`...) calls `record_task_message`, which writes to `Task::messages` in the task JSON.

**Why:** persisting the full feed would mix all projects and tabs together and grow indefinitely; per-task data has a clear cap
(`MAX_MESSAGES` × `MAX_MESSAGE_CHARS`) and data is cleaned up when the task is deleted. Only `Ask`, `Answer`, and `Error` entries
where the task instance is sender or receiver are recorded; `Thinking` is transient, `System` has no sender, and `User` originates in the UI
itself (`Deck::add_feed`), never passing through core.
**How to apply:** any new bridge entry must go through `Core::feed` (with `.in_project(...)`: without a project it is not
recorded) to appear on the timeline. Comparison is against the project-less id (`claude@t3`, never `claude@t30`). Do
not call `Core::feed` from within `with_tasks`: recording re-locks the task store.

---
title: Notifications are not persisted; the task timeline is
summary: Core::feed feeds two things: Task::messages (persisted) and the app's notifications (memory only, 100); a new notice needs a Tone.
touches: [crates/core/src/lib.rs, crates/core/src/events.rs, crates/core/src/tasks/**, crates/core/src/bridge.rs, crates/app/src/deck/**, crates/app/src/notices.rs]
---
`Core::feed` is the single entry point for both. It calls `record_task_message`, which writes `Ask`, `Answer` and `Error`
entries where the task instance speaks or listens to `Task::messages` (the detail's Timeline pane, persisted in the task
JSON). Then the UI turns `System` entries and agent `Error` entries into notifications (`notices::notice_text`), kept only
in memory (`MAX_NOTICES` = 100) and cleared when the app closes; questions and answers are not shown outside the timeline.
Since t8 there is no Integrated Chat: a message between agents outside any task is only in the pair's history
(`ask-agent --inbox`).

**Why:** persisting a global feed would mix projects and grow without limit; per-task data has a clear cap and is deleted
with the task. Notifications are news, not a record.
**How to apply:** a bridge entry must go through `Core::feed` with `.in_project(...)` (without a project it is not recorded)
to appear on a timeline. A system notice gets a `Tone` (`FeedEntry::tone`: `Warning` for what was refused or worked around,
`Error` for what failed, so the toast stays until closed) and, when it is about a task, `.about_task(id)` so the
notification opens it. In the app, use `Deck::notify(tone, text)`; `Deck::system` is plain information. Do not call
`Core::feed` from within `with_tasks`: recording re-locks the task store.

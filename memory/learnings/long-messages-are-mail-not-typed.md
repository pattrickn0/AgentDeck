---
title: Long messages are mail, not typed; every delivery of a notice must end typed or dropped
summary: A long or multi-line question/late answer is saved in data/mail/ and only a one-line notice is typed; the interface must report mail_typed or mail_dropped, or reminders and the restart notice break.
touches: [crates/core/src/mail.rs, crates/core/src/bridge.rs, crates/app/src/deck/frame.rs, crates/app/src/deck/tabs.rs]
---
Typing long or multi-line text into a CLI is fragile (split pastes, Enter timing) and loses the text if the app exits first.
Above `mail::SHORT_MAX` (200 characters) or with a newline, `ask_tab` and `deliver_late` save the text and type
`[mail #n from X: "…" — read with: ask-agent --read n]`; the addressee reads it with `GET /mail/<n>` (403 for anyone else).

**Why:** the per-run state (`notified`, `typed`, reminders) lives only in memory, so it only stays right if the interface
reports what happened to each notice.
**How to apply:** any new code path that queues a `Delivery` with `mail: Some(n)` must end in `Core::mail_typed`
(typed) or `Core::mail_dropped` (queue discarded, tab closed/ended, question gone). Without `mail_dropped` the notice is
never typed again when the tab comes back (`mail_tab_started`); without `mail_typed` the end-of-turn reminder never fires.
Reminders and restart notices are `request: 0, answer: true` deliveries: they never open a tab and never call
`fail_delivery`/`answer_lost`. A question's deadline counts from the reading (`bridge::mail_deadline`), so a new wait
on a mailed question must use `reply_deadline(core, project, request, …)`, not `deadline_after` directly.
Pending questions do not survive a restart (the asker's process dies with the app): mail does, as information.

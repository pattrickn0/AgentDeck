---
title: Tasks core: which text goes through t! and which stays English
summary: Desk text (labels, history events, refusals, notices) uses t!; briefs/feedback/boot lines are plain English; saved values are English and older Portuguese ones are read as English.
touches: [crates/core/src/tasks/**, crates/core/tests/tasks*.rs, crates/core/tests/support/tasks_env.rs]
---
In `crates/core/src/tasks` the text has three destinations:

- **What the desk shows** (`TaskStatus::label`, `Task::log` events, `TaskError` messages, `MergeBlock::explain`, stash notices, feed lines) goes through `t!`
  with the pair in `i18n/pt_br/tasks_core.rs`. The bridge answers agents under `i18n::english`, so an error that reaches an agent comes out English.
- **What only agents read** (`start_message`, `return_message`, `reject_message`, `memory_block`, `boot_line`, `MergeBlock::return_feedback`,
  the `[text cut at N characters]` marker of `cap_text`, the git stash name) is plain English, never `t!`.
- **Saved data** is English: `Feedback::origin` (`review` / `validation`; a `validation` counts as a rejection in `capability.rs`),
  `Task::creator` (`user`, `instances::USER`), the `task/` branch prefix, `data/tasks/`, the `.unreadable-` backup name and the merge
  commit subject `Merge task tN: ...` (parsed by `examples/memeval.rs`). Files and branches made before the English rename carry
  `revisão` / `validação` / `usuário`, `tarefa/` and `Merge tarefa`: the readers accept them (serde `deserialize_with`, `valid_task_branch`,
  memeval), so old tasks keep working.

**Why:** `start_locked` runs on the UI thread, so a `t!` reached from it (for example `TaskStatus::label()` inside the situation block that
`situation.rs` builds for the brief) comes out in the user's language inside agent-facing text.
**How to apply:** when a function builds text for an agent and calls anything that may use `t!`, wrap it in `i18n::english(|| ...)`
(see `task_memory`). Tests: run in English; check the Portuguese side with `i18n::with_lang(Lang::PtBr, || ...)`.

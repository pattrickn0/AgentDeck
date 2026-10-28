---
title: Tasks core: which text goes through t!, which stays English, which Portuguese values are data
summary: Desk text (labels, history events, refusals, notices) uses t!; briefs/feedback/boot lines are plain English; revisão/validação/usuário and "Merge tarefa" are saved data and stay.
touches: [crates/core/src/tasks/**, crates/core/tests/tasks*.rs, crates/core/tests/support/tasks_env.rs]
---
In `crates/core/src/tasks` the text has three destinations:

- **What the desk shows** (`TaskStatus::label`, `Task::log` events, `TaskError` messages, `MergeBlock::explain`, stash notices, feed lines) goes through `t!`
  with the pair in `i18n/pt_br/tasks_core.rs`. The bridge answers agents under `i18n::english`, so an error that reaches an agent comes out English.
- **What only agents read** (`start_message`, `return_message`, `reject_message`, `memory_block`, `boot_line`, `MergeBlock::return_feedback`,
  the `[text cut at N characters]` marker of `cap_text`, the git stash name) is plain English, never `t!`.
- **Saved data** stays as it is: `Feedback::origin` (`revisão` / `validação`, read by `capability.rs`; `text.rs::origin_label` shows it as
  review/validation to the agent), `Task::creator` (`usuário`), the `tarefa/` branch prefix, `dados/tarefas/`, the `.ilegivel-` backup name and the
  merge commit subject `Merge tarefa tN: ...` (parsed by `examples/memeval.rs`).

**Why:** `start_locked` runs on the UI thread, so a `t!` reached from it (for example `TaskStatus::label()` inside the situation block that
`situation.rs` builds for the brief) comes out in the user's language inside agent-facing text.
**How to apply:** when a function builds text for an agent and calls anything that may use `t!`, wrap it in `i18n::english(|| ...)`
(see `task_memory`). Tests: run in English; check the Portuguese side with `i18n::with_lang(Lang::PtBr, || ...)`.

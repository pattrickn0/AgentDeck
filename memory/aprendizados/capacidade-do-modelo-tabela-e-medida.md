---
title: Model capability comes from benchmark tables and measurements; the agent's model is assumed
summary: capability.json (AA Coding Agent Index) provides the level; 3+ tasks at a level make measurement take precedence; without --model, the assumed model in defaults applies.
touches: [crates/core/src/capability.rs, crates/core/src/capability.json, crates/core/src/tasks/**]
---
The level reached by a model (`simples`, `media`, `complexa`) comes from `capability.json` (score from the AA Coding Agent Index v2.0,
thresholds 53 and 40, outside the table = `simples`) and, with 3 or more tasks at a level, from measurements of completed tasks
(`capability::measure`/`capacity`). Effort `low` drops one level. `dados/capacidade.json` overrides the table by `name`.

**Why:** selecting solely by quota sent gpt-6-luna to build an engine. The CLI does not report its default model,
so the table includes `defaults` (claude→sonnet, codex→gpt-6-luna...): what applies to an agent opened without `--model` is a
guess, and automatic selection does not record that model in the task (only that of `role_models` or the requested one).
**How to apply:** if an agent runs a different model from the assumed one, configure `role_models` or edit `defaults` in
`dados/capacidade.json`; new models enter the table with `aliases` (CLI id without provider, identical or followed by `-`; the
`.` does not separate, so `gpt-6` won't match `gpt-6.1-sol`). Measurement groups by the table row's name, so OpenCode's free DeepSeek
and paid DeepSeek count together.

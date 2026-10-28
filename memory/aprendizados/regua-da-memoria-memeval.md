---
title: Memory benchmark (memeval): character budget matters more than co-change
summary: cargo run -p agentdeck-core --example memeval measures recall/precision/size of learning delivery; on 24/10 co-change did not beat the baseline of most-changed files, and the 6000-character ceiling was the bottleneck.
touches: [crates/core/examples/memeval.rs, crates/core/src/learnings.rs, crates/core/src/situation.rs]
---
`crates/core/examples/memeval.rs` evaluates across merged tasks (and direct commits to `dev`) what each learning delivery
strategy would have predicted accurately. Run with the **Windows path** of the tasks file (the binary does not parse
`/c/Users/...` from Git Bash):

`cargo run --release -p agentdeck-core --example memeval -- . "C:\...\dados\tarefas\<project>.json" [--detalhe]`

Ground truth = learnings with `touches` matching files modified by the task, evaluated **at the base commit** of the task (the point where the
branch was created, not the merge's `merge-base`: merging `dev` into the branch would shift the base). Global learnings are excluded from scoring.

**Findings from 24/10 evaluation** (39 tasks, 28 with base learnings, 12-day history):
- The current delivery bottleneck is the **6000-character budget**, not ranking quality: strategy (a) achieves 46% recall
  under the budget vs. 60% without it (9000 characters already reaches 56%), at ~+500 tokens per task.
- **Co-change did not beat the baseline control** "suggest files most frequently changed prior to the base" (core files: `deck.rs`,
  `tasks.rs`, `app.rs`), neither for memory delivery nor file prediction (neighbor precision ~30% vs. ~28%).
  Sorting by lift (frequency weighting) degraded precision. Age decay (7 and 3 days) showed no effect.
- Task descriptions frequently reference paths (36 of 39 tasks); commit messages rarely do (12 of 58): the commit sample
  is weak for strategies (a) and (c).

**Why:** in a small, young repository, the same handful of files are modified in almost every task, so "changed together"
degenerates into "is a central hub file".
**How to apply:** before building dependency graphs, re-run evaluation once more history is recorded (currently results shift
with just 1 or 2 tasks) and always compare against the baseline grid control `ctrl`; co-change is only worthwhile if it outperforms it.

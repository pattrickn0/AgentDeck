---
title: historico.md merges using union strategy
summary: .gitattributes marks memory/historico.md with merge=union; do not remove it, and do not edit old entries.
touches: [memory/historico.md, .gitattributes]
---
`.gitattributes` marks `memory/historico.md` with `merge=union`: when two branches append lines at the same
location, git retains lines from both sides instead of stopping with a merge conflict.

**Why:** every task places its entry at the top of the history file, so two concurrent tasks previously conflicted there on
every merge (t33, t34, t35, and t39 on 23/10; t37 and t38 were heading the same way). The user saw "conflict while
merging" rejections on every task.
**How to apply:** the file only receives new entries; do not rewrite or delete historical entries, because union merge
does not perform true conflict detection and would keep both versions. A task's entry may end up placed below entries that were merged
after it started: this is expected. Do not use `union` on files where lines are also deleted (`pendencias.md`, code).

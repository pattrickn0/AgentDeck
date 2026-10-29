---
title: A tab that needs to launch receives its request via file, not typed input
summary: Typing into a CLI that is still booting drops text; initial prompt is an ASCII line instructing it to read a file.
touches: [crates/core/src/tasks/**, crates/core/src/agents.rs, crates/core/src/models.rs, crates/core/src/delivery.rs]
---
Typing text into a tab that is still initializing (brand new, or restarted process) drops input: the CLI is still booting up (observed
with Antigravity). Therefore, task briefs, feedback, and rejection notices are written to `data/tasks/<project>-<id>-brief.md` (or
`-feedback.md`) and `Delivery::boot` sends only a single ASCII command line (`tasks::boot_line`) directing it to read that file.
`agents::spawn_spec_for` puts it on the command line once (`models::boot_prompt_flag`): `claude "<line>"`,
`agy -i "<line>"`, `opencode --prompt "<line>"`.

**Why:** quotation marks, line breaks, and accented characters on the command line are corrupted in Windows `.cmd` shims.
**How to apply:** do not pass task body text directly on the command line, and do not type into a booting tab. New messages
dispatched to a task agent pass through `task_boot_prompt`; CLIs without a `boot_prompt_flag` and already-running tabs continue using typed input.

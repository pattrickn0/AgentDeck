---
title: Codex sandbox does not write to .git, even with --add-dir
summary: In a task worktree, Codex's `git add`/`git commit` yields "Access is denied"; committing requires approval outside the sandbox.
touches: [crates/core/src/agents.rs, crates/core/src/tasks/**]
---
A task worktree stores its index and HEAD in `<repo>/.git/worktrees/<id>`, outside the task folder. In
`workspace-write` mode (the default for Codex 0.164 on Windows, with `sandbox = "elevated"`), Codex protects any path in
`.git`: neither `--add-dir <repo>\.git`, nor `--add-dir <repo>\.git\worktrees\<id>`, nor
`sandbox_workspace_write.writable_roots` permit writes (tested with `codex exec` and `codex sandbox`).

**Why:** on 2026-10-24, t49 (Codex, gpt-6-luna) delivered its report without committing; `git add` exited with status 1 and
merging was rejected due to uncommitted work. The commit was only made when the command was approved outside the sandbox.
**How to apply:** when assigning a task to Codex, note in the instructions that committing requires approval outside the sandbox. Better yet: have the app
commit remaining changes in the worktree upon receiving `--task done`, or open the task Codex with a different sandbox
policy (user decision).

**Test that only fails in sandbox:** `file_lock::tests::a_file_held_open_names_this_process` fails inside Codex (Restart
Manager returns `[]`) and passes outside it (observed in t55, 2026-10-25). Reviewers should run tests outside the sandbox
before treating this failure as a defect.

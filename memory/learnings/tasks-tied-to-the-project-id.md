---
title: Tasks are bound to project id, not directory path
summary: Removing and re-adding a project used to generate a new id and orphaned tasks in data/tasks/<old id>.json; re-adding the same directory now reuses the id.
touches: [crates/core/src/tasks/**, crates/core/src/config.rs, crates/core/src/lib.rs]
---
The task board stores tasks in `data/tasks/<project id>.json` (and task descriptions in `<id>-tNN-brief.md`). Removing a
project from the list does not delete this file, but re-adding the same directory used to generate a new id (`p` + hex) and the
board appeared empty. Worktrees (`.holt/worktrees/`) move along with the folder, but each task's `worktree` field
and git's internal records (`.git/worktrees/*/gitdir`) still referenced the old path.

**Why:** on 2026-10-24, a project directory was renamed, the project was removed and re-added, and 47
tasks vanished from the board (id changed from `pfdaeded2` to `p76d8a279`).

**How to apply:** since `t50` (2026-10-24), removing a project preserves the id + path pair in `Config.removed_projects`
(`removedProjects` in `config.json`, camelCase, last 50; `Config::remove_project`), and `Config::add_project`
(`Core::create_project`) reuses the id when the path returns, provided no active project currently uses that id.
Path comparison uses custom logic (`same_folder`): `\` and `/` are treated equally, trailing slashes are trimmed, and on
Windows only, comparison is case-insensitive. Therefore, do **not** compare project paths with `util::same_path` when the
intent is "the path entered by the user".

Manual recovery (the paragraph below) is only needed for projects removed prior to this change that lack a saved pair.
With the app closed (it keeps the project list in memory and writes it when modifying tasks): copy `<old id>.json`
to `<new id>.json` (the id is found in `projectList` in `%APPDATA%\Holt\config.json`), update the old path in the
`worktree` field, rename `<old id>-tNN-*.md` files, and run `git worktree repair <worktrees>`; or add
`{"id": "<old id>", "path": "<the folder>"}` to `removedProjects` and re-add the folder via the app, which is the path
implemented in `t50`.

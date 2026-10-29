# Project learnings

AgentDeck's long-term memory: one `.md` file per item the project has learned that a future agent should not
need to rediscover (a pitfall, a decision, a convention). Versioned in git, learnings go through the same review
and merge workflow as code. This `README.md` does not count as a learning.

## Format

```
---
title: Accents are lost on the Windows standard input
summary: PowerShell 5.1 sends stdin as ASCII; text with accents goes through --file.
touches: [crates/core/src/bridge.rs, crates/cli/**]
---
The fact, then **Why:** and **How to apply:** in a few lines.
```

- `title` and `summary` (preferred, in English) or the old `titulo` and `resumo`: one line each, required. The summary is
  what shows in the index. New learnings are written in English.
- `touches`: globs relative to repository root, as a list (`[a, b]`) or as list items `- a`. `*` does not cross folders,
  `**` crosses; a pattern without `/` (`*.rs`) matches the filename across any directory. Empty or omitted = applies to
  the entire project.
- The body is required. Keep it short: the app delivers full content up to a character budget; anything beyond that is truncated
  to an index summary line.
- Files with invalid frontmatter are ignored (with a warning logged in the app log); they never fail a task.

## How they reach the agent

- At the start of each task: full text of global learnings and those matching files cited in the task description, plus an index
  of remaining learnings.
- Upon return or rejection of a task: specific learnings for files newly cited in feedback that the agent had not yet seen.
- On demand: `ask-agent --learn [<file>...]` prints learnings relevant to those files; without arguments, prints global
  learnings in full along with an index of all others.

## When to create one

Only for things that are **not obvious** from the code and cost time or caused bugs. Do not summarize what code already clearly shows.
Create the file on the task branch and reference it in the report. File naming: short, lowercase, hyphenated.

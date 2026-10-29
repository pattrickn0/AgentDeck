---
title: App statusline requires user statusline padding
summary: Claude Code applies statusLine padding; our --usage only forwards output, so write_claude_hooks copies the user's padding.
touches: [crates/core/src/agents.rs, crates/cli/src/bin/ask-agent.rs]
---
The app writes `data/hooks/claude-settings.json` (passed via `--settings`) with its own statusline
(`ask-agent --usage`), replacing the user's statusline. `--usage` runs the user's command and prints its
output, but `padding` is applied by Claude Code, not by the command line: with a hardcoded `"padding": 0`, a user
statusline that had margins ends up touching the border. `write_claude_hooks` reads `padding` from the user's statusline
following the same precedence as Claude Code (project `.claude/settings.local.json`, project `.claude/settings.json`,
`$CLAUDE_CONFIG_DIR/settings.json`, or `~/.claude/settings.json`), ignoring our own file, and writes that value
into our `statusLine`.

**Why:** `padding` does not appear in the command's stdout, so simply proxying the user's command is insufficient.
**How to apply:** when touching `write_claude_hooks`, preserve padding retrieval. The hooks file is shared per
data directory, so projects with different padding configurations alternate which value is stored (matching the user's
global padding for all projects is the common case).

---
title: Accents are lost on the Windows standard input
summary: PowerShell 5.1 sends stdin as ASCII; text with accents goes through --file.
touches: [crates/cli/**, crates/core/src/bridge.rs, crates/core/src/tasks/**, crates/core/src/util.rs]
---
In Windows PowerShell 5.1, `$OutputEncoding` is US-ASCII: piped text (`... | ask-agent x -`) or text passed via the
shell loses every non-ASCII character (accents and dashes turn into `?`) before reaching `ask-agent`.
`ask-agent --file <file>` reads the file directly, in UTF-8 (or UTF-16 with BOM, as PowerShell 5.1 writes it), via
`util::decode_text`.

**Why:** it was the first real bug on the task board (reports and briefs arrived with `?`).
**How to apply:** reports, feedback, and task requests, or any text with accents, go into a file
(`ask-agent --task done <id> --file <file>`). When touching `ask-agent` or the bridge, do not replace `--file` with stdin
and keep `decode_text` when reading.

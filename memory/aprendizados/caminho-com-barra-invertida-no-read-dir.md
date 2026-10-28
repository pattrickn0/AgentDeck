---
title: Backslash in a path is only a separator on Windows
summary: On Unix `\` is part of the filename (`a\b` is a directory); only on Windows does `read_dir` replace `\` with `/` before validating and joining.
touches: [crates/core/src/workspace.rs]
---
`Path::components()` (and `Path::join`) only treats `\` as a separator on Windows. On Unix, the backslash is an
ordinary character in the name: `ws.join("zeta\\")` looks for a directory named `zeta\`, and a directory `a\b` (originating from a Windows tool
or an archive) really exists.

**Why:** the test `workspace::tests::read_dir_orders_and_filters` passed on Windows and failed on Ubuntu starting
17/10/2026 (`index out of bounds`): it requested `read_dir(ws, "zeta\\")`. Task `t29` normalized `\` → `/` across all
platforms, but the ultrareview of 23/10 showed that this breaks directory `a\b` on Unix: the `rel_path` returned by the tree
preserves the exact name, and replacing the slash opened a different directory (or none).

**How to apply:** normalize `\` → `/` only on Windows (`cfg!(windows)`), as `read_dir` does now. On Unix, preserve the name
byte for byte. A path test with `\` is only valid on Windows; on Unix, the test is for a directory with `\` in its name. When touching
path functions, run tests on Linux (CI) as well: passing only on Windows proves nothing.

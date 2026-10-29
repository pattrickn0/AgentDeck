---
title: The whole codebase uses rustfmt and CI rejects drift
summary: Run cargo fmt --all before each commit; CI "Formatting" step (cargo fmt --all -- --check) fails on any unformatted file.
touches: [crates/**]
---
Since 2026-10-25, the entire workspace passes `cargo fmt --all -- --check`, using the project's `rustfmt.toml` (`max_width = 140`,
`use_small_heuristics = "Max"`). CI runs this step before building on Windows and Ubuntu, so any unformatted file turns CI
red.

**Why:** the old learning advised formatting only one's own files because the codebase had accumulated drift; as a result, tasks
stopped running `fmt` and three files in t52 were committed unformatted, turning CI red without anyone noticing.
**How to apply:** `cargo fmt --all` before committing (it now touches only what you changed). If differences appear in a file you didn't
touch, it came from another unformatted commit: include it in your commit, which is what keeps CI green.

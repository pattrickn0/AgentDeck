---
title: --outline finds items using braces and syntax highlighting lexer, not a full parser
summary: outline.rs uses spans from highlight.rs (the lexer lives in core) to ignore strings and comments; lexer bugs cause missing items.
touches: [crates/core/src/outline.rs, crates/core/src/highlight.rs, crates/app/src/highlight.rs]
---
`ask-agent --outline` (`outline.rs`) does not understand programming language syntax deeply: it strips quoted strings
and comments from source code using spans provided by `highlight::highlight_lines`, detecting items via brace depth
and line-opening keywords (`fn`, `struct`, `impl`...). Because of this, the lexer (`Tok`, `Span`, `Lang`, `Carry`, `highlight_line(s)`)
lives in `crates/core/src/highlight.rs`; search, `pieces`, and `Cache` reside in `crates/app/src/highlight.rs`, which
re-exports the lexer (app call sites `crate::highlight::...` remained unchanged).

**Why:** this was the lightest way to generate code outlines without importing `syn` or `tree-sitter`. The trade-off is that
a lexer bug breaks the outline for the entire file: a multiline `"` string that `highlight` failed to carry over across lines
caused an inner `/**` to open a block comment, and `learnings.rs` lost all definitions after a constant (fixed: Rust `"` strings
now carry over to subsequent lines).
**How to apply:** if an outline is missing items, check first for strings or comments misparsed by `highlight`
(`highlight_lines` on the line preceding the first missing item) before touching `outline.rs`. Outline items only include top-level
declarations and members of `impl`/`trait`/`mod`/`class`; function bodies are never included, and a single-line `impl X { fn a() {} }`
will not list members. Responses exceeding `FILE_BUDGET`/`OUTLINE_BUDGET` truncate with "... and N more items". The prompt
instructions for every tab (`bridge_instructions`) have a character budget checked by `the_instructions_carry_the_rules_and_stay_short`: the
`--outline` sentence increased it from 2580 to 2740.

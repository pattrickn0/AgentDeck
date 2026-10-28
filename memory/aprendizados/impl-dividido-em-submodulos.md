---
title: `impl` split across child modules: private methods and shadowed names
summary: When splitting an `impl` into child modules, private `fn` in one section is invisible to another (`pub(super)`), and a submodule named after `use crate::x::{self, ...}` shadows the import.
touches: [crates/app/src/deck/**, crates/core/src/tasks/**]
---
The `impl Deck` in `crates/app/src/deck.rs` was split into one `impl` per file in `crates/app/src/deck/` (t54). Anyone repeating this
split on another large `impl` (t53 does the same with `crates/core/src/tasks.rs`) will encounter two pitfalls that
`cargo check` only surfaces after moving everything.

**Why:** visibility in Rust is scoped per module, not per `impl`. A `fn load_project(&mut self)` declared in
`deck/tabs.rs` is private to `deck::tabs` and hidden from `deck::sidebar`, even though both are methods on `Deck`; only
`Deck`'s fields remain visible (as they reside in the parent module). Also, `mod dictation;` in the parent module collides with
`use crate::dictation::{self, ...}`: the `use` fails to compile (E0255) and, worse, `dictation::toggle(...)` starts resolving
to the newly declared submodule (E0425), because shadowing affects every child module that includes `use super::*;`.

**How to apply:** after moving code, run `cargo check` and replace `    fn name(` with `    pub(super) fn name(` only on
methods called across sections (`pub(super)` = visible to parent and siblings, matching the visibility the method had
when inside the single module; do not use `pub(crate)`, which widens scope unnecessarily). For shadowed names, remove `self` from the `use`
and qualify call sites (`crate::dictation::toggle`). `use super::*;` at the top of each file imports types, constants,
and imports from the parent without unused import warnings, and unit tests (`#[cfg(test)]`) can stay inside the files they test.

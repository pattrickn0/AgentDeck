---
title: The Documents mode editor: decisions and pitfalls
summary: Highlighting via custom tokenizer (no syntect); TextEdit requests the galley every frame (fingerprint cache); dirty edits are never replaced by disk reads.
touches: [crates/app/src/docs.rs, crates/core/src/highlight.rs, crates/app/src/ui/code.rs, crates/app/src/ui/select.rs, crates/app/src/ui/diff_view.rs, crates/app/src/ui/docs.rs]
---
A small text file in Documents mode is a `DocState::Source` rendered via an `egui::TextEdit` (`ui/code.rs`); diffs and
large files are rendered manually (`ui/diff_view.rs`, `ui/code.rs::text_body`) using selection logic from `ui/select.rs`.

- **Highlighting without `syntect`.** `egui_extras::syntax_highlighting` (syntect) would bring in regex engines, syntaxes, and themes:
  several MBs in executable size and minutes added to CI compilation across three platforms, all for nine languages. `highlight.rs` is a table
  per language (keywords, comments, quotes) and a per-line function, with state passed between lines in `Carry`.
  If a language is missing, add an entry to `spec()` (or `Lang::from_path`); only consider `syntect` if true grammar parsing is required.
- **`TextEdit` requests the galley every frame.** The `layouter` runs every frame (and again on each keypress), so computing syntax highlighting there without a cache
  would rebuild the whole file 60 times a second. `docs::Shaped` caches the galley keyed by text fingerprint, wrap width, display scale,
  and search state (`Find::stamp`), while `highlight::Cache` caches styled spans by text and language: typing invalidates, redrawing does not. When touching keys,
  verify the test `the_shaped_text_is_rebuilt_only_when_...`. The editor limit (`EDIT_MAX` 256 KB, 4,000-byte line) exists for this reason.
- **The multiline `TextEdit` in egui 0.36 does not scroll horizontally** (text overflows and is clipped): the editor wraps lines at the
  available width, and the line numbers column only renders a number on the first line of each paragraph (`ends_with_newline` on the galley). Large files,
  which bypass `TextEdit`, scroll in both directions.
- **Dirty edits are never replaced by a disk read.** `DocTab::apply` routes disk reads for a `Source` to `Source::reload`, which only preserves disk text
  in `external` (a banner prompts the user). Any new code path writing to `DocState` must respect this; it is also why
  `docs_reload` for a `Source` does not show "Loading...". The file watcher re-reads after our own saves: `Source::is_disk` prevents false
  warnings.
- **Line endings and BOM are preserved as originally found.** The UI operates on `\n` without BOM; `Eol`/`bom` from the original file are applied in
  `workspace::write_text`. Mixed CRLF+LF or non-UTF-8 text locks the editor (`Source::locked`): saving back would alter lines the user
  never touched.
- **Custom selection only where egui cannot handle it.** `select::drive` registers the interaction area before any overlapping buttons are painted (such as chunk buttons
  in Git tab diffs), otherwise the area intercepts their clicks. The position is (line, visual text byte); copied text unexpands
  tabs (`diff_view::unexpanded`).
- **UI testing with egui**: a fresh `Context` has no fonts until the first `run_ui`, and `TexturesDelta` requires `.clear()` before
  being dropped (see `draw_frame` in `tasks_flow_tests.rs`, which also injects key events, text, and mouse actions).

---
title: egui_commonmark tracks the egui version
summary: The minor version of egui_commonmark follows egui (0.25 for egui 0.36); update both together and restore markdown spacing.
touches: [Cargo.toml, crates/app/Cargo.toml, crates/app/src/ui/docs.rs]
---
Markdown in Documents mode is rendered by `egui_commonmark` (workspace dependency, no extra features). It depends on
an exact version of `egui`: 0.25 corresponds to egui 0.36 (0.26 requires 0.37, 0.24 requires 0.35). When upgrading egui,
check crates.io for which `egui_commonmark` targets the new egui and upgrade both in the root `Cargo.toml`; if none is available,
custom rendering with `pulldown-cmark` (headings, checklists with `- [ ]`, tables, blockquotes, code blocks) is the fallback plan.

**Why:** the app zeros `style.spacing.item_spacing` and `button_padding` (`theme::install_style`) because almost everything is painted by hand;
`egui_commonmark` relies on this spacing to separate paragraphs and list items, and without it text renders glued together. Therefore,
`ui/docs.rs::markdown_style` restores spacing, fonts, and colors specifically within the document column.
**How to apply:** when updating egui, update both simultaneously and check results with `HOLT_DEBUG_UI=docs-demo-report`; adjust
markdown styling only in `markdown_style`, never in `install_style`.

---
title: In egui, place color and TextEdit margin can be ignored
summary: painter.galley only uses the given color for uncolored text; TextEdit with .frame(...) ignores .margin(...).
touches: [crates/app/src/ui/**]
---
Two pitfalls that slipped through the visual overhaul unnoticed:

- `place(painter, r, align, g, color)` (and `painter.galley`) only applies `color` to uncolored text (`PLACEHOLDER`). A galley
  created with `galley(..., p.text_strong)` always renders white, regardless of the color passed to `place`. This caused
  the right sidebar tabs to render completely white and active badge numbers to appear white on highlight background colors.
- In egui 0.36, `TextEdit::frame(Frame)` replaces the entire frame: `.margin(...)` only applies to the default
  frame. Using `.frame(egui::Frame::NONE).margin(m)` causes text to touch the edge.

**Why:** neither produces an error or warning; it only becomes apparent upon close inspection of screenshots.

**How to apply:** construct galleys with their final color upfront (or use `text_in`, which handles this); when color depends on hover,
measure width with `text_w` beforehand and paint afterward. For text inputs with custom framing, place the margin inside the frame:
`.frame(egui::Frame::NONE.inner_margin(m))`.

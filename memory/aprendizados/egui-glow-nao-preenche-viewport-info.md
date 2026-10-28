---
title: EguiGlow does not populate ViewportInfo (maximized is always None)
summary: Without egui_winit::update_viewport_info each frame, viewport().maximized is None; the app populates it in App::redraw.
touches: [crates/app/src/app.rs, crates/app/src/ui/titlebar.rs]
---
We use winit + `egui_glow::EguiGlow` without eframe. eframe is what normally calls `egui_winit::update_viewport_info`; `EguiGlow::run`
only holds a private `viewport_info` that never reaches `RawInput`. Result: `ctx.input(|i| i.viewport().maximized)` (and
`minimized`, `focused`, `inner_rect`...) is always `None`. This caused maximized windows not to return to windowed mode: the
button repeatedly sent `Maximized(true)`, the glyph remained unchanged, and `resize_edges` stayed enabled.

**Why:** nothing logs a warning; `unwrap_or(false)` masks the `None`.
**How to apply:** `App::redraw` calls `update_viewport_info(&mut viewport_info, ..)` and writes the result into
`egui.egui_winit.egui_input_mut().viewports[ROOT]` before calling `egui.run`. Any UI depending on `ViewportInfo` requires
this step. When persisting window size, ignore maximized state (`size_to_persist`): otherwise a "normal" window reopens
matching screen dimensions.

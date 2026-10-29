---
title: UI modifications require the agentdeck-ui skill and tokens from theme.rs
summary: Load the agentdeck-ui skill, use exclusively colors and measurements from theme.rs, and verify via screenshots.
touches: [crates/app/src/ui/**, crates/app/src/theme.rs, crates/app/src/*_ui.rs, crates/app/src/icons.rs]
---
The UI is hand-painted egui (`Painter`, `ui.interact`), governed by dedicated rules in the `agentdeck-ui` skill
(`.claude/skills/agentdeck-ui/SKILL.md`): product design principles, decisions already made by the user, and the tokens
table. Colors, fonts, and dimensions must come from `crates/app/src/theme.rs`, never as hardcoded inline values in the code.

**Why:** the visual presentation became inconsistent when each area picked its own colors (the left sidebar felt like a different
theme compared to the right sidebar); theme architecture is documented in `ideas.md` ("Interface"), and future theming relies on everything
originating from `theme.rs`.
**How to apply:** before creating or changing anything in the UI, invoke the `agentdeck-ui` skill. Then
compile, run with `AGENTDECK_SMOKE` and `AGENTDECK_DEBUG_UI=<state>` in a separate data folder
(`AGENTDECK_HOME`), and review screenshots to check alignment and contrast. Clicks and drags do not show up in
screenshots: inform the user about what remains to be verified manually.

**Since t21 (visual overhaul C, part 1):** colors come from `theme::pal()` (semantic palette `Palette`), no longer from loose
constants; the accent color is that of the active agent (`deck.accent()`/`theme::accent()`, with text over it in `on_accent`); the
test `colors_come_from_the_palette` (`theme.rs`) prevents hardcoded colors outside `theme.rs`, `terminal/colors.rs`, and
`icons.rs`. `Color32` stores premultiplied alpha: for a translucent color use `theme::with_alpha`, never `.r()` on a color
with alpha. `painter.galley(pos, galley, color)` does not change the color already recorded in the galley: create it with the right color upfront.
Glass surfaces are translucent, so island shadows are clipped to gaps (`ui::island_shadows`) and corners are
repainted via `island_edge`; menu/modal backgrounds must be `popup` (opaque), never `hover` or `chip`.

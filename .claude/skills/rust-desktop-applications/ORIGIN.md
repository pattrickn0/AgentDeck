# Origin

Copied from [bobmatnyc/claude-mpm-skills](https://github.com/bobmatnyc/claude-mpm-skills),
`toolchains/rust/desktop-applications` (MIT license, see `LICENSE`), on 2026-10-15.

Changes in `SKILL.md`: `name` went from `desktop-applications` to `rust-desktop-applications`, and
`user-invocable: false` and `disable-model-invocation: true` were removed (with them the skill never loaded). The
references are as in the original.

Use in AgentDeck: a general Rust desktop reference. Most of it is about Tauri; for egui see
`references/native-gui-frameworks.md`. The app's visual rules are in the `agentdeck-ui` skill.

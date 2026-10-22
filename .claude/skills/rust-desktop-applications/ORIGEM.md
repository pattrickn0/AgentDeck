# Origem

Copiada de [bobmatnyc/claude-mpm-skills](https://github.com/bobmatnyc/claude-mpm-skills),
`toolchains/rust/desktop-applications` (licença MIT, ver `LICENSE`), em 2026-10-15.

Alterações no `SKILL.md`: `name` passou de `desktop-applications` para `rust-desktop-applications`, e saíram
`user-invocable: false` e `disable-model-invocation: true` (com eles a skill nunca carregava). As referências
estão como no original.

Uso no AgentDeck: referência geral de Rust desktop. A maior parte é sobre Tauri; para egui vale
`references/native-gui-frameworks.md`. As regras visuais do app ficam na skill `agentdeck-ui`.

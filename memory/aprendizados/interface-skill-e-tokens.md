---
titulo: Mexer na interface exige a skill agentdeck-ui e os tokens de theme.rs
resumo: Carregue a skill agentdeck-ui, use só cores e medidas de theme.rs e confira com captura de tela.
touches: [crates/app/src/ui/**, crates/app/src/theme.rs, crates/app/src/*_ui.rs, crates/app/src/icons.rs]
---
A interface é egui pintado à mão (`Painter`, `ui.interact`), com regras próprias na skill `agentdeck-ui`
(`.claude/skills/agentdeck-ui/SKILL.md`): princípios do produto, decisões já tomadas pelo usuário e a tabela de
tokens. Cores, fontes e tamanhos vêm de `crates/app/src/theme.rs`, nunca como cor solta no código.

**Por quê:** o visual ficou inconsistente quando cada área escolhia suas cores (a barra esquerda parecia de outro
tema que a direita); está em `pendencias.md` ("Identidade visual e temas"), e os temas futuros dependem de tudo vir
do `theme.rs`.
**Como aplicar:** antes de criar ou mudar qualquer coisa na interface, invoque a skill `agentdeck-ui`. Depois
compile, rode com `AGENTDECK_SMOKE` e `AGENTDECK_DEBUG_UI=<estado>` numa pasta de dados separada
(`AGENTDECK_HOME`) e abra a captura para conferir alinhamento e contraste. Clique e arrasto não aparecem na
captura: diga ao usuário o que falta testar à mão.

**Desde a t21 (visual C, parte 1):** cores vêm de `theme::pal()` (paleta semântica `Palette`), não mais de constantes
soltas; a cor de destaque é a do agente ativo (`deck.accent()`/`theme::accent()`, texto sobre ela em `on_accent`); o
teste `colors_come_from_the_palette` (`theme.rs`) barra cor solta fora de `theme.rs`, `terminal/colors.rs` e
`icons.rs`. `Color32` guarda o alfa pré-multiplicado: para uma cor translúcida use `theme::with_alpha`, nunca `.r()`
em cor com alfa. `painter.galley(pos, galley, cor)` não troca a cor já gravada na galley: crie-a com a cor certa.
O vidro é translúcido, então a sombra das ilhas é recortada para as frestas (`ui::island_shadows`) e os cantos são
repintados por `island_edge`; fundo de menu/modal tem de ser `popup` (opaco), não `hover` ou `chip`.

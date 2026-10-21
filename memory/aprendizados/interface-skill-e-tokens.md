---
titulo: Mexer na interface exige a skill commanddeck-ui e os tokens de theme.rs
resumo: Carregue a skill commanddeck-ui, use só cores e medidas de theme.rs e confira com captura de tela.
touches: [crates/app/src/ui/**, crates/app/src/theme.rs, crates/app/src/*_ui.rs, crates/app/src/icons.rs]
---
A interface é egui pintado à mão (`Painter`, `ui.interact`), com regras próprias na skill `commanddeck-ui`
(`.claude/skills/commanddeck-ui/SKILL.md`): princípios do produto, decisões já tomadas pelo usuário e a tabela de
tokens. Cores, fontes e tamanhos vêm de `crates/app/src/theme.rs`, nunca como cor solta no código.

**Por quê:** o visual ficou inconsistente quando cada área escolhia suas cores (a barra esquerda parecia de outro
tema que a direita); está em `pendencias.md` ("Identidade visual e temas"), e os temas futuros dependem de tudo vir
do `theme.rs`.
**Como aplicar:** antes de criar ou mudar qualquer coisa na interface, invoque a skill `commanddeck-ui`. Depois
compile, rode com `COMMANDDECK_SMOKE` e `COMMANDDECK_DEBUG_UI=<estado>` numa pasta de dados separada
(`COMMANDDECK_HOME`) e abra a captura para conferir alinhamento e contraste. Clique e arrasto não aparecem na
captura: diga ao usuário o que falta testar à mão.

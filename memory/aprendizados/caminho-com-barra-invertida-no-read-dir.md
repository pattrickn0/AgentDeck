---
titulo: Barra invertida em caminho só é separador no Windows
resumo: No Unix `\` é parte do nome (`a\b` é uma pasta); só no Windows `read_dir` troca `\` por `/` antes de validar e juntar.
touches: [crates/core/src/workspace.rs]
---
`Path::components()` (e `Path::join`) só trata `\` como separador no Windows. No Unix, a barra invertida é um caractere
normal do nome: `ws.join("zeta\\")` procura uma pasta chamada `zeta\`, e uma pasta `a\b` (vinda de ferramenta do Windows
ou de um arquivo compactado) existe de verdade.

**Por quê:** o teste `workspace::tests::read_dir_orders_and_filters` passava no Windows e falhou no Ubuntu a partir de
17/10/2026 (`index out of bounds`): ele pedia `read_dir(ws, "zeta\\")`. A `t29` passou a normalizar `\` → `/` em todas as
plataformas, mas o ultrareview de 23/10 mostrou que isso quebra no Unix a pasta `a\b`: o `rel_path` que a árvore devolve
traz o nome intacto, e trocar a barra abria outra pasta (ou nenhuma).

**Como aplicar:** normalize `\` → `/` só no Windows (`cfg!(windows)`), como `read_dir` faz agora. No Unix, mantenha o nome
byte a byte. Teste de caminho com `\` vale só no Windows; no Unix, o teste é o de uma pasta com `\` no nome. Ao mexer em
função de caminho, rode o teste também no Linux (CI): passar só no Windows não prova nada.

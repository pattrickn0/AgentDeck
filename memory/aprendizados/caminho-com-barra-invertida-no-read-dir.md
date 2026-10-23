---
titulo: Barra invertida em caminho só é separador no Windows
resumo: No Unix, `ws.join("zeta\\")` procura um arquivo com `\` no nome; normalize o separador antes de validar e juntar.
touches: [crates/core/src/workspace.rs]
---
`Path::components()` (e `Path::join`) só trata `\` como separador no Windows. No Unix, a barra invertida é um caractere
normal do nome do arquivo: `read_dir(ws, "zeta\\")` validava o caminho com `git::safe_rel` (que aceita `zeta\` no Unix) e
depois fazia `ws.join("zeta\\")`, procurando um arquivo chamado `zeta\` — devolvia vazio e estourava o índice no teste.

**Por quê:** o teste `workspace::tests::read_dir_orders_and_filters` passava no Windows desde sempre e falhou no Ubuntu
a partir de 17/10/2026 (`index out of bounds` em `workspace.rs`). Como a CI depois passou a parar na formatação, a falha
ficou escondida.

**Como aplicar:** em função que recebe caminho relativo digitado/transportado, normalize `\` → `/` **antes** de validar e
de juntar (é o que `read_dir` faz agora). Caminhos que o app monta já usam `/`; o caso aparece em caminho vindo de fora
ou de código escrito no Windows. Ao mexer em função de caminho, rode o teste também no Linux (CI) — passar só no
Windows não prova nada.

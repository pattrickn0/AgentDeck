---
titulo: A capacidade do modelo vem da tabela de benchmarks e da medida; o modelo do agente é presumido
resumo: capability.json (AA Coding Agent Index) dá o nível; 3+ tarefas num nível fazem a medida mandar; sem --model, vale o modelo presumido em defaults.
touches: [crates/core/src/capability.rs, crates/core/src/capability.json, crates/core/src/tasks.rs]
---
O nível que um modelo alcança (`simples`, `media`, `complexa`) sai de `capability.json` (nota do AA Coding Agent Index v2.0,
limiares 53 e 40, fora da tabela = `simples`) e, com 3 ou mais tarefas num nível, da medida das tarefas já feitas
(`capability::measure`/`capacity`). Esforço `low` desce um nível. `dados/capacidade.json` sobrepõe a tabela por `name`.

**Por quê:** escolher só pela cota mandava o gpt-6-luna fazer uma engine. O CLI não diz qual é o seu modelo padrão,
então a tabela traz `defaults` (claude→sonnet, codex→gpt-6-luna…): o que vale para um agente aberto sem `--model` é um
palpite, e a escolha automática não grava esse modelo na tarefa (só o de `role_models` ou o pedido).
**Como aplicar:** se um agente roda outro modelo que o presumido, configure `role_models` ou edite `defaults` em
`dados/capacidade.json`; modelo novo entra na tabela com `aliases` (id do CLI sem provedor, igual ou seguido de `-`; o
`.` não separa, para `gpt-6` não casar com `gpt-6.1-sol`). A medida agrupa pelo nome da linha da tabela, então o
DeepSeek free do OpenCode e o pago contam juntos.

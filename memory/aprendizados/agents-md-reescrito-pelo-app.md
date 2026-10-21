---
titulo: O AGENTS.md é reescrito pelo app e não pode travar a mesclagem
resumo: O app mantém um bloco no AGENTS.md; a mesclagem ignora esse arquivo na checagem de "sujo".
touches: [AGENTS.md, crates/core/src/workspace.rs, crates/core/src/tasks.rs]
---
O app mantém um bloco marcado no `AGENTS.md` do workspace com as instruções da ponte (o Antigravity não tem flag
para isso). Nunca cria o arquivo na pasta do usuário: só atualiza um que já exista (`workspace::ensure_agent_context`),
gravando ao lado e trocando, e mantém CRLF.

**Por quê:** o arquivo muda sozinho e aparece como alteração no repositório principal, e `task_merge` recusa
mesclar com alterações sem commit em arquivos rastreados.
**Como aplicar:** `task_merge` tira `AGENTS.md` da checagem de "sujo" (o git ainda protege o arquivo se a mesclagem o
tocar). Não edite o bloco do CommandDeck à mão nem o inclua no commit de uma tarefa.

---
titulo: O AGENTS.md é reescrito pelo app e não pode travar a mesclagem
resumo: O app mantém um bloco no AGENTS.md; a mesclagem o ignora na checagem de "sujo" e o guarda e regrava se a branch também mexe no arquivo.
touches: [AGENTS.md, crates/core/src/workspace.rs, crates/core/src/tasks/**]
---
O app mantém um bloco marcado no `AGENTS.md` do workspace com as instruções da ponte (o Antigravity não tem flag
para isso). Só atualiza um arquivo que já exista (`workspace::ensure_agent_context`), gravando ao lado e trocando, e
mantém CRLF. A única exceção é criar um só com o bloco para o CLI que não tem outro canal (`create = true`; ver
`instrucoes-por-cli.md`); a pasta pessoal do usuário nunca recebe o arquivo.

**Por quê:** o arquivo muda sozinho e aparece como alteração no repositório principal, e `git merge` recusa mesclar
sobre um arquivo alterado que a branch também altera ("commit your changes or stash them").
**Como aplicar:** `task_merge` tira `AGENTS.md` da checagem de "sujo". Se a branch da tarefa também mexe no arquivo e a
diferença para o HEAD é só o bloco do app (`workspace::only_marker_blocks_differ`), ele guarda o conteúdo, faz
`git checkout -- AGENTS.md`, mescla e regrava o bloco depois, em qualquer desfecho (`workspace::restore_marker_block`).
Se o usuário editou o texto fora do bloco, recusa sem descartar nada. Não edite o bloco do AgentDeck à mão nem o
inclua no commit de uma tarefa.

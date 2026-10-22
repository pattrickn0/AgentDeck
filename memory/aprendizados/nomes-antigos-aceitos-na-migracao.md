---
titulo: Os nomes antigos (CommandDeck) são aceitos de propósito durante a migração
resumo: Variáveis COMMANDDECK_*, cabeçalho x-commanddeck-token, marcadores commanddeck:start/end, trava .commanddeck.lock e config CommandDeck/ ainda são lidos; não "limpe" antes da hora.
touches: [crates/core/src/env.rs, crates/core/src/workspace.rs, crates/core/src/bridge.rs, crates/core/src/config.rs, crates/core/src/lib.rs, crates/core/src/agents.rs, crates/cli/src/bin/**]
---
O app foi renomeado de CommandDeck para AgentDeck (tarefa `t20`). Quem atualiza não pode perder projetos, abas, tarefas
nem a ponte, então o código ainda aceita o nome antigo em cinco lugares, e todos têm teste:

- **Variáveis de ambiente:** `agentdeck_core::env_var("X")` (`env.rs`) lê `AGENTDECK_X` e, se faltar ou estiver vazia,
  `COMMANDDECK_X`. Toda leitura (app, core, `ask-agent`, `ollama-agent`) passa por ele, para uma aba aberta pela versão
  antiga continuar funcionando. O app só **escreve** `AGENTDECK_*`. O filtro de `build_agent_env` (`agents.rs`) tira os dois
  prefixos das variáveis herdadas, com `ASK_WAIT` como exceção nos dois nomes.
- **Ponte:** o servidor aceita `x-agentdeck-token` e `x-commanddeck-token` (`bridge::token_header`); os clientes mandam o novo.
- **`AGENTS.md`:** `workspace.rs::marker_re` reconhece `<!-- commanddeck:start/end -->` (além de `orquestra` e `jwic`) e
  troca o bloco pelo `agentdeck:`, sem duplicar nem tocar no resto do arquivo.
- **Trava:** `lock_data_dir` (`lib.rs`) trava `dados/.agentdeck.lock` **e** `dados/.commanddeck.lock`; com o app antigo
  aberto na mesma pasta, o novo recusa abrir (e o antigo recusa se o novo abriu primeiro).
- **Configuração:** `config::legacy_config_files` começa por `%APPDATA%/CommandDeck/config.json`. Ao carregar dele, o app
  **copia** para `AgentDeck/config.json` (o antigo fica como está) e mantém projetos **e abas** (a config de "CommandDeck
  Terminal"/Electron e a "orquestra" seguem zerando as abas). Com `AGENTDECK_HOME` o app é isolado e não lê config nenhuma
  do usuário.

**Por quê:** o rename vem antes da promoção `dev` → `main`, para quem usa a `main` migrar uma vez só; um usuário que
atualiza não pode perder nada. Quem "limpa" um desses fallbacks quebra a atualização, ou uma aba que ficou aberta.
**Como aplicar:** não remova nenhum deles antes da próxima versão depois da promoção para a `main` (decisão do
orquestrador; ver a seção "Migração do nome" em `MANUTENCAO.md`). Ao remover, tire os cinco juntos, com os testes. Ao
escrever um teste que mexe nessas variáveis, remova **os dois** prefixos do ambiente do processo filho (os testes podem
rodar dentro de uma aba da versão antiga, que tem `COMMANDDECK_*` no ambiente).

---
titulo: Só a config do CommandDeck ainda é migrada; os outros nomes antigos saíram
resumo: Variáveis COMMANDDECK_*, x-commanddeck-token, marcador commanddeck: e .commanddeck.lock não valem mais (t32); só a cópia de %APPDATA%/CommandDeck/config.json segue em config.rs.
touches: [crates/core/src/env.rs, crates/core/src/workspace.rs, crates/core/src/bridge.rs, crates/core/src/config.rs, crates/core/src/lib.rs, crates/core/src/agents.rs, crates/cli/src/bin/**]
---
O app foi renomeado de CommandDeck para AgentDeck (tarefa `t20`), que aceitou os nomes antigos por compatibilidade. A `t32`
(2026-10-23, a pedido do usuário, antes da promoção `dev` → `main`) tirou todos, com os testes:

- **Variáveis:** só `AGENTDECK_*` (`agentdeck_core::env_var`, `env.rs`); `build_agent_env` filtra só esse prefixo, com `ASK_WAIT`
  como exceção.
- **Ponte:** só `x-agentdeck-token` (`bridge::token_header`); o `ask-agent` manda só ele.
- **`AGENTS.md`:** `workspace.rs::marker_re` reconhece `agentdeck` e os mais antigos `orquestra` e `jwic`, não mais `commanddeck`.
  Um bloco `commanddeck:` já gravado fica no arquivo, ao lado do bloco novo (apague à mão).
- **Trava:** só `dados/.agentdeck.lock`.

**Fica uma exceção**, num lugar só: `config::legacy_config_files` começa por `%APPDATA%/CommandDeck/config.json`. Sem
`AgentDeck/config.json`, o app **copia** a do CommandDeck (o antigo fica como está) e mantém projetos **e abas** (as de
"CommandDeck Terminal"/Electron e de "orquestra" seguem zerando as abas). Com `AGENTDECK_HOME` o app é isolado e não lê config
nenhuma do usuário.

**Por quê:** só aqui a falta da migração tem custo real: quem atualiza sem nunca ter aberto o AgentDeck perde a lista de
projetos. Os outros nomes, quando faltam, só pedem fechar e reabrir uma aba (variáveis, cabeçalho, trava) ou apagar um bloco
velho do `AGENTS.md`.
**Como aplicar:** não tire a cópia da config antes de uma versão depois da promoção para a `main` (item em `pendencias.md`);
ao tirar, leve `legacy_config_files`, `is_commanddeck_config`, a cópia em `Config::load_from` e os testes `*_commanddeck_*`.
Nunca reintroduza um prefixo antigo de variável ou cabeçalho "só por compatibilidade".

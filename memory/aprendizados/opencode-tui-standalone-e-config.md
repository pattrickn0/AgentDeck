---
titulo: O TUI do OpenCode precisa de --standalone para ler modelo e instruções do ambiente
resumo: OPENCODE_CONFIG_CONTENT só é lida por quem sobe o servidor; o TUI abre sempre com --standalone e build_agent_env limpa a variável herdada.
touches: [crates/core/src/models.rs, crates/core/src/agents.rs]
---
O TUI do OpenCode não tem `--model` nem opção de linha de comando para instruções de sistema (só `opencode run` e `mini`
têm `-m`, e a variante `modelo#alta` só existe no `run -m`). Na aba interativa, o modelo e o arquivo
`dados/agentdeck-instructions.md` vão em `OPENCODE_CONFIG_CONTENT` (`models::opencode_tui`), que vale por cima da
configuração do usuário e soma as instruções ao `AGENTS.md` do projeto.

**Por quê:** a variável `OPENCODE_CONFIG_CONTENT` só é lida pelo processo que sobe o servidor do OpenCode. Sem
`--standalone`, o TUI reutiliza o servidor compartilhado em segundo plano, que ignora o ambiente da aba nova. Do outro
lado, um processo filho aberto de dentro de uma aba do OpenCode herdaria `OPENCODE_CONFIG_CONTENT` daquela aba.

**Como aplicar:** toda aba do OpenCode abre com `--standalone` (`models::opencode_tui`), com ou sem modelo escolhido, e
tira o sufixo `#variante` do nome do modelo. Em `agents::build_agent_env`, mantenha `OPENCODE_CONFIG_CONTENT` na lista
`REMOVE` para não vazar para processos filhos nem para os testes da ponte; `spawn_spec_for` insere o valor da aba nova
depois desse filtro.

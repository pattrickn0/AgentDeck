# Pendências

Os **próximos passos**, no máximo 10, na ordem em que serão feitos (revisados com o usuário em 2026-10-24). Ao concluir
um item, **tire-o daqui** e acrescente uma linha do que foi feito em [historico.md](historico.md) (regras dos documentos em
[GERAL.md](GERAL.md)). Este arquivo não guarda item concluído.

O que não está entre os próximos fica em [ideias.md](ideias.md): ideias sem data e as limitações conhecidas. Uma ideia
só entra aqui quando sai outro item (a lista não passa de 10). Os **testes manuais** ficam em [testes.md](testes.md); um
teste que falhar vira item aqui. O rumo do produto e quem decide estão em [AGENTDECK.md](AGENTDECK.md).

## Agora

1. [ ] **Validação no uso (usuário).** O terminal sem as linhas em branco depois de mudar o tamanho (`t37`; se voltar,
   "Gravar o terminal (diagnóstico)" no menu da aba e mandar `dados/pty/`), o editor de documentos (`t39`) e a recusa por
   cota (`ask-agent --models antigravity`, `--team`). Também do usuário: renomear a pasta local `commanddeck-dev` (o
   repositório no GitHub já se chama `AgentDeck`).
2. [ ] **Testes da ponte também no Windows.** Os testes do fluxo entre abas (`a_question_is_typed_into_the_tab_and_answered_by_callback`
   e vizinhos, `an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told`) só rodam em Unix (`#[cfg(unix)]`
   em `crates/core/tests/bridge.rs`, agentes falsos em shell): criar agentes falsos para Windows para a CI do Windows
   cobrir a ponte. A falha da CI de 2026-10-24 (teste que só valia num sistema) é desta família.
3. [ ] **Pasta da worktree que não sai ao validar: achar a causa.** O log já existe (2026-10-24, `file_lock.rs`): a cada
   falha de `remove_task_worktree`, o `agentdeck.log` e o aviso da tarefa dizem que arquivos ficaram e que processo os tem
   aberto (Restart Manager), ou que só pastas vazias ficaram (um processo com o diretório atual dentro dela). Falta a
   próxima ocorrência real: ler o aviso (procure "não saiu" no log) e corrigir a causa.
4. [ ] **Linha do tempo das mensagens de cada tarefa**, para aposentar a Conversa integrada: no detalhe da tarefa, as
   mensagens entre agentes daquela tarefa, em ordem.

## Depois (na lista, ainda não)

5. [ ] **Instalador para Windows**, e na mesma versão tirar a migração da config `CommandDeck/` (`config::legacy_config_files`;
   lista e razão em [MANUTENCAO.md](MANUTENCAO.md), seção 6).
6. [ ] **Inglês na interface.**

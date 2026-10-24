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
2. [ ] **Instruções do Antigravity sem criar `AGENTS.md` no repositório** (`t38`). Hoje o app cria o arquivo na pasta do
   usuário (ele aparece no `git status`) porque não havia outro canal verificado. Testar se o `agy` lê `AGENTS.md`/`GEMINI.md`
   de uma pasta passada em `--add-dir` (o app já passa `dados/`: bastaria gravar lá); se não, avaliar um plugin do `agy`
   (`~/.gemini/config/plugins/<nome>/rules/AGENTS.md`, global e valendo fora do AgentDeck). Goose e Copilot seguem pelo
   `AGENTS.md`.
3. [ ] **`config.json` com BOM UTF-8 é ignorado em silêncio** (visto na `t28`): o app cai nos padrões sem aviso. Editar
   pelo PowerShell 5.1 (`Out-File`/`Set-Content -Encoding utf8`) grava o BOM. Tirar o BOM ao ler (`Config::load`) e
   avisar no log quando o arquivo não se lê.
4. [ ] **Testes da ponte também no Windows.** Os testes do fluxo entre abas (`a_question_is_typed_into_the_tab_and_answered_by_callback`
   e vizinhos, `an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told`) só rodam em Unix (`#[cfg(unix)]`
   em `crates/core/tests/bridge.rs`, agentes falsos em shell): criar agentes falsos para Windows para a CI do Windows
   cobrir a ponte. A falha da CI de 2026-10-24 (teste que só valia num sistema) é desta família.
5. [ ] **Pasta da worktree que não sai ao validar: log para achar a causa.** A tarefa fica em `Validating` e dá para tentar
   de novo, mas não se sabe o que segura a pasta (visto na `t14`: cinco tentativas). Registrar no `agentdeck.log`, a cada
   falha de `remove_task_worktree`, o erro do git, o arquivo que não saiu e, no Windows, que processo o tem aberto
   (Restart Manager), e mostrar isso na mensagem.
6. [ ] **Linha do tempo das mensagens de cada tarefa**, para aposentar a Conversa integrada: no detalhe da tarefa, as
   mensagens entre agentes daquela tarefa, em ordem.

## Depois (na lista, ainda não)

7. [ ] **Instalador para Windows**, e na mesma versão tirar a migração da config `CommandDeck/` (`config::legacy_config_files`;
   lista e razão em [MANUTENCAO.md](MANUTENCAO.md), seção 6).
8. [ ] **Inglês na interface.**

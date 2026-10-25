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
2. [ ] **Segundo plano e bandeja** (decidido em 2026-10-24). Primeiro (T1a, feito na `t44`), o `Deck::tick` saiu de dentro da
   pintura: `Deck::background` roda a partir do laço do winit com a janela escondida ou minimizada (ver ARCHITECTURE.md,
   "Trabalho com e sem quadro"; `app::is_hidden` decide). A T1b precisa, ao esconder a janela com `set_visible(false)`, pedir
   uma rodada (`Background::wake`), porque o winit não avisa. Depois (T1b), o X
   esconde a janela na bandeja, sem perguntar (Windows: `tray-icon`/Win32; Linux: `ksni`, sem GTK; sem
   `StatusNotifierWatcher` no D-Bus, o X minimiza); a bandeja tem Abrir e Encerrar (Encerrar pergunta só com aba no meio de
   um turno); abrir o programa de novo traz a instância de volta, como o Discord (a segunda abertura lê a porta da ponte
   na trava, faz `POST /activate` e sai; no Windows, `AllowSetForegroundWindow`).
3. [ ] **Retomar a conversa de cada aba** depois de fechar o app ou de um crash (T4). Id por aba em `ProjectCfg`, gravado
   assim que é conhecido, nunca `--continue`. Claude: `--session-id`/`--resume` com id nosso (o hook atualiza depois de
   `/clear`); OpenCode: `--session <id>` (cria se não existe); Antigravity: `--conversation <id>`, com o id lido do
   `--log-file` da aba; Codex: `codex resume <id>`, com o id do `notify`, e no Windows resolver o shim do npm até o
   executável para o Codex ter hooks. Na mesma tarefa: o Antigravity pede a confiança da pasta a cada abertura (hipótese:
   o `--add-dir dados`; um teste com 4 casos decide). Retomada que falha logo reabre do zero e avisa uma vez.
4. [ ] **A mesa escolhe o agente pela cota** (decidido em 2026-10-24). Sem `--agent`, o `--task new` usa o CLI oficial com
   mais folga, e o motivo fica registrado; no formulário "+ Nova", a cota ao lado de cada agente, com o mais folgado já
   marcado; aviso (não recusa) ao delegar a um modelo acima de 80% com outro oficial folgado. Antes, na `t47`: a memória
   geral manda distribuir pela cota e o `--models` mostra a cota de todos os oficiais.
5. [ ] **Memória medida, depois o grafo** (T3, depois T7). Medição automática sobre as tarefas já feitas: o certo são os
   aprendizados dos arquivos que a tarefa de fato mudou; o palpite é o que cada jeito entregaria lendo só o pedido (hoje;
   hoje + o índice de uma linha de todos; grafo); nota = acertos e tamanho entregue, com os aprendizados como estavam na
   base da tarefa. O grafo começa pela co-mudança do `git log` (sem merges, sem commits grandes, sem `memory/**`), que
   também sugere o escopo das tarefas; só vale para a memória se ganhar do índice.
6. [ ] **Fase da tarefa e "parado há N min"** (T5), só pelos hooks: `PostToolUse` do Claude (a última ferramenta usada
   vira "editando", "testando"…); os outros CLIs mostram trabalhando/parado e há quanto tempo.
7. [ ] **Correio durável** (T6). Mensagem de até ~200 caracteres, numa linha, continua digitada; a longa fica guardada em
   disco e no terminal vai só o aviso (`ask-agent --read <n>`); o aviso é repetido no fim de turno se ninguém leu; o prazo
   de resposta conta a partir da leitura; as mensagens pendentes sobrevivem ao fechamento do app.
8. [ ] **Pasta da worktree que não sai ao validar: achar a causa.** O log já existe (2026-10-24, `file_lock.rs`): a cada
   falha de `remove_task_worktree`, o `agentdeck.log` e o aviso da tarefa dizem que arquivos ficaram e que processo os tem
   aberto (Restart Manager), ou que só pastas vazias ficaram (um processo com o diretório atual dentro dela). Falta a
   próxima ocorrência real: ler o aviso (procure "não saiu" no log) e corrigir a causa.
9. [ ] **Aposentar a Conversa integrada.** A linha do tempo por tarefa já existe (t43, seção MENSAGENS do detalhe, guardada em
   `Task::messages`); falta validá-la no uso e então tirar a Conversa. O que a linha do tempo não tem: as mensagens do usuário
   à aba (`FeedKind::User`) e os avisos do sistema.

## Depois (na lista, ainda não)

10. [ ] **Instalador para Windows**, e na mesma versão tirar a migração da config `CommandDeck/` (`config::legacy_config_files`;
   lista e razão em [MANUTENCAO.md](MANUTENCAO.md), seção 6).

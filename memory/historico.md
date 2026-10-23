# Histórico

O que já foi feito, do mais recente para o mais antigo. É o destino de todo item concluído de
[pendencias.md](pendencias.md): sai de lá e entra aqui com uma linha do que foi feito (regras dos documentos em
[GERAL.md](GERAL.md)). Os textos abaixo ficam como foram escritos na época; para o estado atual do código, vale
[ARCHITECTURE.md](ARCHITECTURE.md).

## Concluído

- [x] **Limpezas: dica do Ctrl+L, padding da statusline e modelos da demonstração** (tarefa `t26`). A dica de "Atalhos do app acima do terminal" cita só o Ctrl+B (o Ctrl+L era da caixa de mensagem removida na `t11`) e os comentários que ainda falavam do compositor saíram de `main.rs`, `theme.rs`, `config.rs`, `terminal/input.rs` e `agents.rs`; `write_claude_hooks` lê o `padding` da statusline própria do usuário (projeto local, projeto e usuário, na precedência do Claude Code) em vez de fixar 0, com teste; os dados de demonstração da mesa (`tasks_ui.rs`) usam os modelos atuais (`claude-opus-5-5`, `claude-sonnet-5-5`, `gemini-4-argon-high`, `opencode/deepseek-v4.1-flash-free`).

- [x] **Visual C, parte 3: barra direita, menus e modais** (tarefa `t23`). Cabeçalho da barra direita de 32 px (`TOPBAR_H`) com 4 abas-pílula com nome (`Tarefas`, `Conversa`, `Git`, `GitHub`) e selo em pílula; Mesa de tarefas com cartões ativos de 3 linhas, barra de progresso de 5 passos e botões de ação embutidos (`Mesclar`/`Devolver` em revisão, `Validar`/`Reprovar`/`Ver diff` em validação), seção `HISTÓRICO` com filtro (`Concluídas e descartadas`, `Só concluídas`, `Só descartadas`) e linhas compactas de 24 px, rodapé fixo `+ Nova tarefa`, detalhe e formulário com `section_label`, `button`/`button_danger` e campos `recess` com foco `accent`; Conversa com cabeçalho `CONVERSA` + `Limpar`, `pal().user` e cartões translúcidos; Git e GitHub alinhados aos botões, campos, rótulos espaçados e chave segmentada do Visual C; menus, dicas (11,5 px) e modais (`popup`, cantos 10, `glass_border`, título 13 px 600); `#[allow(dead_code)]` transitórios da `t21` removidos.

- [x] **Visual C, parte 2: barra lateral e centro** (tarefa `t22`). A chave Agentes | Documentos sai da barra de título e vira o topo da barra lateral (24 px, segmento ativo na cor do agente, selo como ponto; recolhida, volta compacta para a barra de título, logo depois do logo); agentes numa linha de 24 px com o logo do modelo, o nome com reticências e o modelo encurtado à direita em IBM Plex Mono (`projects::model_tag`: `sonnet`, `argon`, `flash`), selecionado com fundo de destaque a 18% e filete de 2 px; projeto com o anel do agente que terminou sem ser visto; ORGANIZAÇÃO vira o controle Abas | Dividida; o explorador vira ARQUIVOS, em cinza, com a letra do Git (M, A, ?) e linhas de 20 px; as abas viram pílulas (a ativa na cor do agente, o ✕ desenhado, sem o modelo), com a faixa de 30 px, nos agentes e nos documentos (ativa em `selected`); lista de arquivos do diff, botões e estado vazio com os ajudantes da parte 1. Saíram os ícones `Split`, `FolderOpen` e `FolderClosed` e as cores `BADGE_*` das extensões.

- [x] **Visual C, parte 1: a base** (tarefa `t21`). Paleta semântica `theme::pal()` (`Palette`, `DARK`) no lugar das constantes soltas; cor de destaque = cor do agente ativo (`Deck::accent()`, `theme::accent()`); fontes Inter e IBM Plex Mono embutidas (`assets/fonts`, OFL); fundo com dois brilhos radiais (`ui/background.rs`); barra lateral, centro, barra direita e cada janela da tela dividida viram ilhas de vidro (`ui::island*`, 6 px de margem e fresta); barra de título nova (marca, projeto, branch, uso das assinaturas só de quem tem cota); logo `Icon::AgentDeck`, `assets/agentdeck.ico` (`scripts/gerar-icone.ps1`), ícone da janela e do executável (`build.rs`); ajudantes `segmented`, `button`, `section_label`, `chip`, `separator`; teste que barra cor solta fora de `theme.rs`, `terminal/colors.rs` e `icons.rs`. Faltam as partes 2 (barra lateral e centro) e 3 (barra direita, menus e modais).

- [x] **Renomear para AgentDeck** (tarefa `t20`; passo 5 do rumo, só o nome, sem visual novo). Crates `agentdeck` e `agentdeck-core` (o executável é `agentdeck.exe`; `ask-agent` e `ollama-agent` mantêm o nome), variáveis `AGENTDECK_*`, cabeçalho `x-agentdeck-token`, marcadores `<!-- agentdeck:start/end -->`, `dados/.agentdeck.lock`, `%APPDATA%/AgentDeck/config.json`, título da janela, textos da ponte e skill `agentdeck-ui`. Migração para quem já usa: `agentdeck_core::env_var` lê `AGENTDECK_X` e, na falta, `COMMANDDECK_X`; a ponte aceita os dois cabeçalhos; o bloco do `AGENTS.md` com marcadores antigos é trocado sem duplicar; o app novo trava também `.commanddeck.lock` (recusa abrir com o antigo aberto); a config do `CommandDeck` é copiada para o lugar novo, com projetos e abas, sem apagar a antiga. Com `AGENTDECK_HOME` o app fica isolado e não lê config do usuário. Os nomes antigos nos itens abaixo ficam como foram escritos. Ver [MANUTENCAO.md](MANUTENCAO.md), seção 6, e [aprendizados/nomes-antigos-aceitos-na-migracao.md](aprendizados/nomes-antigos-aceitos-na-migracao.md).
- [x] **Documentos no centro: chave Agentes | Documentos** (tarefa `t19`; passo 4 do rumo, "diff legível"), fase 1, só leitura. O centro alterna entre os agentes (a tela dividida) e os documentos do projeto: chave na barra de título (`ui/docs.rs::switch`; Ctrl+Shift+D, `Action::ToggleCenter`) com o selo em "Agentes" (abas que terminaram sem ser vistas e, lendo, as que trabalham); abrir um documento muda para Documentos, fechar o último volta; escondidos, os terminais seguem rodando e o PTY não é redimensionado (sem `fit_tab`; o teclado e o IME não chegam a eles). Estado e lógica pura em `crates/app/src/docs.rs` (`Docs`, `DocKind::{TaskDiff, CommitDiff, FileDiff, File, Markdown}`, `diff_files`, `Badge`), pintura em `ui/docs.rs`, faixa de abas reaproveitada de `ui/tabs.rs` (`strip_items`, com as setas ‹ › e o menu ▾ da t12). Diff com a lista de arquivos (nome e +/−; clicar rola) sobre `ui::git::diff_lines_at`; arquivo de texto com número de linha, relido quando o watcher avisa (`Core::read_text`, 1 MB, `safe_rel`); markdown pelo `egui_commonmark` 0.25 (compatível com o egui 0.36; `Cargo.lock` atualizado, `--locked` passa). A mesa abre "Ver diff completo"/"Ver o que foi mesclado" e, nova, "Abrir ao centro" do pedido e do relatório no centro (saiu `diff_screen` da barra direita e o estado `TasksUi::diff`); a aba Git ganhou "Abrir ao centro" no diff de arquivo e no commit. Duplo clique no explorador abre o documento (antes abria no programa do sistema, agora é o botão "Abrir no programa padrão"). Estado de captura `COMMANDDECK_DEBUG_UI=docs-demo*`.
- [x] **Mesa sem becos sem saída depois da mesclagem** (tarefa `t18`; vinha da revisão da `t10`): `Task::merge_commit` (hash completo, gravado em `task_merge`; tarefas antigas carregam com `None`); "Ver o que foi mesclado" em aguardando validação e concluída (`task_diff` usa `git diff <hash>^1 <hash>` e o detalhe mostra o hash curto); reprovada e entregue sem commit novo, `task_merge` vai direto a `Validating` sem novo commit de mesclagem; `--task done` vale em `Validating` com commits novos (volta a `Review`) e dá 409 explicado sem eles; `task_validate` conclui com aviso quando a base foi apagada; `OPENCODE_CONFIG_CONTENT` não vaza mais para o ambiente dos agentes (`build_agent_env`), o que também corrige o teste da ponte dentro de uma aba do OpenCode com modelo.
- [x] **Memória: aprendizados e arquivos sem commit na devolução e na reprovação** (tarefa `t16`): `return_message` e `reject_message` entregam também os aprendizados específicos dos arquivos que o feedback novo cita (`Core::feedback_learnings`, `learnings::render_specific`, até 3 000 caracteres) e a "Situação atual" passa a listar, por tarefa, os arquivos ainda sem commit da worktree (`worktree_dirty_unlocked`, sem tomar o `index.lock` do outro agente).
- [x] **Memória: `--learn` sem argumentos e nas instruções da ponte** (tarefa `t15`): `ask-agent --learn` sem arquivos mostra os aprendizados globais por inteiro e o índice dos demais; as instruções da ponte (`agents.rs`) passam a mandar rodar `ask-agent --learn` antes de mexer em arquivos.
- [x] **Inventário de agentes e modelos** (tarefa `t14`): `ask-agent --models [agente]` (`GET /models[?agent=]`, `Core::models_inventory`, `models::format_inventory`) lista, por agente instalado, os modelos como o `--agent`/`--model` aceitam (`-free` primeiro e marcados "grátis", OpenCode por provedor, Ollama como `ollama:<modelo>`), usando o cache de `known_models`; as instruções da ponte e as regras da tarefa só apontam o comando.
- [x] **Memória em duas camadas, fase 1** (tarefa `t13`). Longo prazo: `memory/aprendizados/*.md` (frontmatter `titulo`, `resumo`, `touches`), lidos por `crates/core/src/learnings.rs` (funções puras e testadas: frontmatter, globs `*`/`**`, caminhos citados no brief) e entregues no `start_message` (texto completo dos globais e dos que casam com arquivos do pedido, até 6 000 caracteres; os demais como índice); `ask-agent --learn <arquivo>…` consulta sob demanda (`POST /learn`); a regra da tarefa manda registrar o que for novo na própria branch. Seis aprendizados semeados, todos tirados do que já estava escrito no repositório. Curto prazo: seção "Situação atual" (`situation.rs`: outras tarefas em andamento com os arquivos da branch e os últimos 8 commits da base, 1 500 caracteres) e `Task::feedbacks`, repetido nas mensagens de devolução e reprovação. Detalhes em [ARCHITECTURE.md](ARCHITECTURE.md); o que ficou para depois, em "Memória (fase 2)" de [pendencias.md](pendencias.md).
- [x] **Abas escondidas: indicador e acesso** (tarefa `t12`): quando as abas de uma janela não cabem (janela estreita, tela dividida, muitas abas), cada lado com aba escondida ganha um degradê curto (fundo do cabeçalho → transparente) e uma seta ‹ / › clicável que rola a faixa até a aba escondida mais próxima daquele lado (a dica cita o nome dela); sem aba escondida nada aparece. Com a faixa rolando, um botão ▾ no fim abre o menu com todas as abas da janela (clicar mostra a aba). A aba visível só é trazida à vista quando muda (antes era a cada quadro, o que impedia rolar para longe dela) e fica fora da área coberta pelas setas. A conta de "o que está escondido / para onde rolar" é pura e testada (`hidden_tabs`, `scroll_to_show` em `ui/tabs.rs`). Isto também resolve o item antigo "Janela estreita: as de trás encolhem até virar só o ponto colorido" (as abas nunca encolhem: a faixa rola).
- [x] **Conversa só leitura, microfone e anexo no cabeçalho da janela e anexos na "Nova tarefa"** (tarefa `t11`): removido o compositor (`ui/composer.rs`) da Conversa integrada; botões 20×20 de microfone e de anexar arquivo colocados ao lado do `+` no cabeçalho de cada janela (`ui/panes.rs`), agindo sobre a aba visível da janela (`plan.shown`), enquanto a aba guarda só o indicador de estado (ponto vermelho pulsando ou `"Abrindo…"`/`"Transcrevendo…"`); anexos para uma aba copiam para `dados/anexos` e digitam `@caminho` no Claude ou o caminho nos demais CLIs via `paste_bytes` (sem Enter); arrastar arquivos destaca a janela de destino ou o formulário "Nova tarefa"; formulário "Nova tarefa" com botão "Anexar arquivos", chips removíveis e caminhos anexados ao final do pedido (`util::append_brief_attachments`).
- [x] **Mesa de tarefas: etapa "Aguardando validação"** (tarefa `t10`). Mesclar já não encerra o agente: fluxo Em andamento → Em revisão → (mesclar) → **Aguardando validação** (`TaskStatus::Validating`) → (usuário testa) → **Concluída**. `task_merge` alinha a branch com a base (`git merge --ff-only`) e deixa aba, worktree e branch; `task_validate` faz a limpeza e marca `Merged` (o valor fica no JSON, o rótulo é "concluída"); `task_reject(feedback)` volta ao mesmo agente na mesma worktree com `reject_message` (reabre a aba e recria a pasta se preciso); descartar vale em `Validating` e avisa que o merge continua na base. `ask-agent --task validate <id>` e `--task reject <id> "motivo"|--file|-`, rotas `/tasks/<id>/validate|reject`, grupo "Aguardando validação" na lista, botões Validar/Reprovar e selo do ícone contando revisão + validação. Na revisão do fluxo saíram duas correções: `Failed` com relatório agora abre o terminal na worktree (não no repositório principal) e validar não conclui com a pasta ainda presa.
- [x] **Mesa de tarefas: o agente não começa sozinho** (resolvido pela tarefa `t6`: o brief termina com uma ordem explícita, "Comece agora, por conta própria…"; e pela `t10`: no Antigravity o texto digitado se perdia porque a carência de 5 s caía na abertura da TUI, então a aba que precisa abrir passa o pedido como prompt inicial na linha de comando, apontando para um arquivo; ver Tarefas em `ARCHITECTURE.md`).
- [x] **Ditado preso a cada aba de CLI** (tarefa `t5`): microfone (e atalho `Ctrl+Shift+M`) que grava, transcreve com o Whisper local e digita o texto no PTY daquela aba (como colagem segura, sem Enter). O destino fica fixo no `uid` da aba onde a gravação começou (`crates/app/src/dictation.rs`), descartando com aviso se a aba fechar antes do fim da transcrição.
- [x] **OpenCode: escolher o provedor antes do modelo** (tarefa `t3`, ajustes da revisão incluídos). O `opencode models` mistura os
  provedores conectados (`opencode`, `opencode-go`, `ollama`…), e a lista única oferecia modelos de um plano
  que o usuário não tem. Agora o menu de modelo (no formulário "Nova tarefa" e no "+ Novo agente") pede o
  provedor primeiro — `models::by_provider`, com a contagem de modelos por provedor — e mostra só os modelos
  dele; `‹ Provedores` volta um passo. Os modelos `-free` levam a marca "grátis" e vêm primeiro (ordenação
  estável, também na lista plana). Com um provedor só, `models::sole_provider` vai direto aos modelos, sem o
  passo extra.
- [x] **Regras da tarefa: o agente não pede merge.** O `start_message` (`tasks.rs`) agora diz que o agente não
  pede a ninguém que mescle nem rode `--task merge`: o trabalho dele termina no `--task done`. Veio do primeiro
  uso real (o `opencode@t2` pediu ao Claude que mesclasse a própria tarefa).
- [x] **Mapa de agentes removido** (tarefa `t1`): saíram a aba Mapa da barra direita, o módulo `agent_map` (e
  `ui/map.rs`), o ícone de diagrama, o `Core::conversation_map` com o rastreio de perguntas headless e os estados
  de depuração `map`/`map-demo`. A árvore de agentes da barra lateral já cobre o "quem é de quem".
- [x] **Mesa de tarefas, fase B: a interface** (branch `feat/mesa-ui`, dev Sonnet):
  - A aba **Tarefas** é a primeira da barra direita, com selo = tarefas em revisão.
  - Lista por estado: em revisão, em andamento, rascunho, falhou e concluídas (recolhida).
  - Detalhe com pedido, relatório, alterações por arquivo, diff completo (o mesmo desenho da aba Git, só leitura) e histórico.
  - Ações por estado: iniciar; mesclar e descartar, com confirmação; devolver, com retorno; tentar de novo; reabrir a aba na worktree.
  - Formulário "Nova tarefa" com agente, modelo, base e "Iniciar agora". Tudo fora da thread da interface.
  - `Core::task_diff`. Testes de ponta a ponta do Deck com git real.
- [x] **Mesa de tarefas, fase A** (core, ponte e CLI; branch `feat/mesa-tarefas`, dev Sonnet; desenho em [AGENTDECK.md](AGENTDECK.md)):
  - Uma tarefa tem pedido, agente `<tipo>@tN` com modelo, worktree e branch próprias em `<repo>/.agentdeck/worktrees/tN` (excluída pelo `.git/info/exclude`), relatório, `diff --stat` e eventos.
  - Ciclo: criar → iniciar (abre a aba já na worktree e entrega o pedido) → `done` (só o próprio agente) → devolver, ou mesclar com `--no-ff` na base e limpar → ou descartar.
  - A mesclagem recusa a árvore principal suja ou em outra branch e desfaz conflito, deixando a tarefa em revisão.
  - Gravação em `dados/tarefas/<projeto>.json`.
  - Rotas `/tasks` e `ask-agent --task …`.
  - A interface veio na fase B (acima).
- [x] **Modelo de LLM por instância** (branch `feat/modelo-por-agente`, dev Sonnet; desenho do orquestrador):
  - O modelo fica ao lado do id (`ProjectCfg.tabModels`) e vale nesta ordem: o pedido (`ask-agent --model`, menu), o salvo, o `Config.roleModels` (`"claude@dev": "sonnet"`) e por fim o padrão do CLI.
  - Chega ao `--model` no claude, agy e codex. No OpenCode, a aba abre com `--standalone` e `OPENCODE_CONFIG_CONTENT`, e o headless usa `run -m`.
  - Modelo diferente numa instância aberta dá 409 explícito, sugerindo `--new`.
  - Aparece na aba, na barra lateral, em `--list` e em `--team`, e o "+ Novo agente" tem um segundo passo com os modelos conhecidos (`agy models`, `opencode models`, numa thread com cache), com o provedor antes do modelo desde a tarefa `t3`.
  - Módulo `core/models.rs`.
- [x] **Revisão do `core/github.rs` por modelos gratuitos do OpenCode**, com achados conferidos: a credencial do remoto não vaza mais na mensagem, os logs do ureq ficam no máximo em DEBUG, as cotas da API estão separadas por login e só o 410 vira lista vazia de issues. Também foi corrigido o teste instável do 401 (RST do Windows).
- [x] **Integração com o GitHub** (branch `claude/github`, dev Sonnet a partir do wip do antigravity@github): aba GitHub na barra direita com PRs (e os checks), issues e execuções do Actions, jobs e o fim do log do job vermelho. "Abrir no navegador" e "Enviar para agente…", que entrega uma mensagem pronta pela fila de entrega (`Deck::send_to_instance`, abrindo `modelo@cargo` se preciso). API REST com o token do Git Credential Manager (`git credential fill`) ou do `gh`, só em memória; sem login, repositório público com 60/h. Conferido ao vivo com o repositório do projeto. Módulo `core/github.rs` com testes sem rede.
- [x] **Git completo e bugs de git da auditoria** (branch `claude/git`, dev Sonnet a partir do wip do antigravity@git):
  - Stash: guardar, com ou sem os não rastreados, listar, ver o diff, aplicar, pop e apagar.
  - Preparar, tirar e descartar por trecho, inclusive em arquivo CRLF.
  - Histórico paginado, com o detalhe do commit.
  - Merge e rebase pelo menu de branches, com a faixa "em andamento" e continuar/abortar (também para cherry-pick e revert).
  - Aba de conflitos: o nosso, o deles ou ambos, por bloco ou no arquivo inteiro; marcar como resolvido; abrir no programa do sistema.
  - Bugs: descartar arquivo em conflito restaura do HEAD; pasta com `.git` usa `clean -ffd`; o `diff --cc` é lido certo.
  - O gpgsign falha na hora, com a explicação, quando a chave está travada.
  - A falha de ssh explica o ssh-agent, com os comandos para Windows e Linux.
- [x] Captura de tela (`COMMANDDECK_SMOKE`) não começa mais o download do Whisper; cada pasta temporária baixava até 1,6 GB.

- [x] **Auditoria: workspace, download, Whisper e uso** (branch `claude/infra`, dev Sonnet a partir do wip do antigravity@infra):
  - O watcher acompanha o git dir real (worktree, submódulo, subpasta) e trata `\\?\` e a caixa da unidade.
  - O download tem leitura do socket com prazo perto do `stall_timeout` (sem thread órfã por horas), e o `rename` do `.part` tenta de novo.
  - O motor do Whisper é extraído numa pasta de preparo e só troca no fim. O pacote só é apagado depois do sucesso, e o `.wav` tem nome único.
  - Uso: fica a primeira credencial não vencida; janela com `resetTime` no passado zera; com vários buckets, fica o de maior uso; 401 tenta o segundo host; o Codex consulta até 5 rollouts; `rfc3339_ms` é estrito; nova tentativa em 20 s depois de falha.

- [x] **Auditoria, interface** (branch `fix/interface`, claude@interface):
  - Estreitar a janela não apaga as proporções salvas da tela dividida (`Ratios::fitted` só ajusta para desenhar).
  - Renomear uma aba atualiza a Conversa e o "pensando".
  - Os modais seguram o clique e o teclado (`ui::modal_backdrop`). Ctrl+/ abre com o terminal focado. "Remover projeto" pergunta numa thread.
  - Rascunho e anexos do compositor ficam por projeto, e o "×" apaga a cópia em `dados/anexos`.
  - O microfone abre fora da thread da interface e grava no máximo 5 min.
  - O círculo de uso zera a janela vencida sem dados novos.
  - Com o terminal focado, Ctrl+L e Ctrl+B vão ao terminal. A opção "Atalhos do app acima do terminal" (☰) inverte isso.
  - O `crash.log` só registra o que derruba o app e gira acima de 256 KB.
- [x] **Auditoria, ponte e equipe** (branch `fix/ponte`, claude@ponte):
  - Quem perguntou e o destino são lidos do `Exchange` pelo número na hora de usar, o que aguenta renomear no meio de uma pergunta. A resposta que chega no instante do prazo não se perde.
  - Depois de um 202, a forma antiga (`ask-agent <quem> "..."`) vira pergunta nova, e a resposta vai por `--reply N` ou `/reply`. Antes do 202 a forma antiga resolve apelidos.
  - Remover projeto revoga os tokens primeiro e não recria a entrada. `PairQueues`, `team_changed` e `activity` são limpos.
  - O aviso de equipe é anexado pela interface na hora de digitar, e o chefe que perde um subordinado é avisado.
  - `Host` validado desde a primeira requisição; orçamento de 64 MB de corpos em andamento (503 acima); a thread da resposta atrasada segura a vaga em `MAX_ACTIVE`.
  - O `ollama-agent` responde pelo número da pergunta.
- [x] **Auditoria, terminal e processos** (branch `fix/terminal`, claude@terminal):
  - A escrita no PTY passa por uma fila com thread própria por aba (`pty-write`), e as respostas do emulador (DA/DSR/cores) vão pela mesma fila. Nem a interface nem a leitora bloqueiam numa colagem grande, e a ordem continua igual.
  - `cmd_safe_arg` escapa para o cmd em vez de apagar `& % ! ^`. O shim roda com `cmd.exe /d /e:on /v:off /c`, testado com o cmd.exe real.
  - OSC 52 só vale para a aba ativa do projeto ativo.
  - No Windows, os headless (e o whisper-cli) entram num Job Object com KILL_ON_JOB_CLOSE e morrem junto com o app.
  - A roda do mouse manda Ctrl/Alt ao programa, e Shift+roda rola o histórico local.
- [x] **Terminal congelava ao redimensionar as janelas** (visto na aba do Claude, que ficou preta). O Claude Code usa a atualização sincronizada (`ESC[?2026h` … `ESC[?2026l`), e o parser (`vte`) retém a saída até o fim dela, mas o prazo de 150 ms fica a cargo de quem o usa: nada o cobrava, então um fim perdido no redesenho segurava a tela até chegarem 2 MB. `Emulator::expire_sync` solta o que estava retido quando o prazo vence, chamado pela leitora antes de cada bloco e pelo `tick` da interface (testes em `terminal/mod.rs`).
- [x] **Conferido em uso (2026-10-15 e 16).** O que o usuário testou no app e o que o Claude verificou ao vivo pela ponte:
  - **Usuário:** projetos (cada um lista os seus agentes abertos, migração da config antiga com as abas), botão de recolher a barra lateral, chave da tela dividida na ORGANIZAÇÃO, barra de título própria, círculos de uso (Claude e Antigravity) e o "+ Novo agente" do projeto.
  - **Ponte, ao vivo:** pergunta digitada na aba do Antigravity e resposta de volta por `--reply`; resposta atrasada (202) digitada depois na aba do Claude como mensagem nova; delegação por cargo em paralelo (`claude@dev`, `antigravity@sidebar`, `antigravity@design`, `antigravity@quota`); `ask-agent --team` mostra a equipe e `--boss` sem chefe explica que se responde ao usuário.
  - **Resolvidos sem mudança de código:** os acentos das respostas do Antigravity chegam certos pela ponte (`ação, coração, você, já, pendências` intacto). Se voltar em respostas longas, reabrir o item. A aba aberta à mão já ganha cargo pelo "Renomear" (duplo clique ou clique direito: `dev`, `dev@ana`).
  - `dados/hooks/` e `dados/.commanddeck.lock` (gerados pelo app) entraram no `.gitignore`.

- [x] **Auditoria de 2026-10-15 (regressão + correções).** Quatro revisores Claude, o Antigravity e capturas de todos os estados de depuração; 235 testes, clippy e fmt limpos. Corrigido:
  - **Dados:** config gravada por arquivo temporário + `rename`, uma gravação por vez e na ordem (a ponte e a interface gravavam juntas); config ilegível guarda uma cópia `.json.ilegivel-*` antes de voltar aos padrões. Capturas e `COMMANDDECK_DEBUG_UI` não gravam mais a config (o estado `empty` apagava as abas salvas).
  - **Git:** trocar de projeto zera o estado do Git e descarta respostas do projeto anterior (geração): uma confirmação de "Descartar" aberta rodava `git restore/clean` no outro repositório. Checkout de branch remota com a local existente; `unstage` de renomeado; `unstage_all` sem HEAD; `git init` comparando com a home sem diferença de caixa; `GCM_INTERACTIVE=never`; a mensagem do commit só sai da caixa se ele der certo; o "+" de cada seção prepara só aquela seção.
  - **Abas e ponte:** Enter sem saída não deixa a aba "trabalhando" para sempre (prendia a fila e o prazo das perguntas); pergunta cancelada na fila não é mais digitada; renomear/menu de contexto seguem a aba certa depois de fechar outra; `set_boss` confere ciclo e grava sob a mesma trava; profundidade negativa recusada; instância órfã na "pergunta a si mesmo".
  - **Terminal e CLIs:** colagem filtra ESC/C1 (marcador aninhado escapava do *bracketed paste*); Ctrl+letra pela tecla física (layouts não latinos); `ask-agent` não trava lendo o stdin de um terminal, aceita cp1252 e recusa `--new --boss`; `ollama-agent` sabe o próprio nome/cargo, responde à ponte mesmo quando falha e não imprime controles vindos do modelo; headless não reabre processo ao encerrar o app; `MSYS_NO_PATHCONV` para o Git Bash não reescrever `/comando`.
  - **Outros:** `read_file` do Ollama lê no máximo 1 MB; aviso honesto ao parar no limite de passos; anexos com o mesmo nome não se sobrescrevem; Conversa limitada a 1000 mensagens; ditado só envia sozinho se a aba de destino ainda for a ativa; watcher com teto de 1 s sob eventos contínuos e comparação de caminho sem caixa; `AGENTS.md` mantém CRLF e é gravado atomicamente; caminhos relativos endurecidos (`safe_rel`, árvore); Whisper encerra com o app; Ctrl+T com a barra recolhida abre o menu de agentes; menus não saem da tela por baixo; o chip `ask-agent` da Conversa não quebra de linha.

- [x] **Equipe de agentes reorganizável e conhecida pelos agentes.** Arrastar um agente sobre outro o põe na equipe dele; soltar no projeto o devolve à raiz (ciclos recusados, fantasma com o destino). Hierarquia no core (`team.rs`, `Core::set_boss`), `GET /team`, `ask-agent --team` e `--boss`, aviso "sua posição na equipe mudou" na próxima mensagem, linha nas instruções da ponte. Fechar passa a equipe ao avô; renomear leva a equipe junto. Skills do projeto em `.claude/skills/` (`commanddeck-ui`, `rust-desktop-applications`, `game-ui-design`).
- [x] **Agentes de todos os projetos, em árvore, com logo.** Seta ▸/▾ em cada projeto e "expandir/recolher todos" no cabeçalho PROJETOS (estado em `ProjectCfg.collapsed`); clicar num agente de outro projeto troca de projeto e ativa a aba. A aba aberta por `ask-agent` fica sob quem a chamou (`ProjectCfg.tab_parents`), com linhas-guia; o ponto virou o logo do modelo (halo trabalhando, anel ao concluir). Ordem da árvore testada em `ui/projects.rs`.
- [x] **Novo agente dentro do projeto.** O "+" solto da barra de título saiu; o projeto ativo ganhou a linha "+ Novo agente" no fim da lista de agentes (abre o menu de agentes; Ctrl+T continua e, com a barra recolhida, abre sob a barra de título). O "+" do cabeçalho PROJETOS segue criando projeto.

- [x] **Projetos como unidade principal.** Cada aba pertence a um projeto (pasta) e só aparece com ele; as de outros projetos continuam rodando escondidas. Instâncias, tokens e `ask-agent` por projeto (`instances::scoped`), config por projeto (`projectList`, com migração), Conversa do projeto ativo, abas dos outros projetos abertas sob demanda. Sem workspace padrão. Seção PROJETOS na barra lateral.

- [x] **Barra de título própria, uso dos agentes e barra lateral nova.**
  - Janela sem a barra do sistema: ☰ (configurações), "CommandDeck", recolher barra, "+", tela dividida e botões de janela menores; arrastar, duplo clique e bordas pelo egui (`ui/titlebar.rs`).
  - Uso por agente aberto: logo + círculo que enche com a sessão + anel semanal (âmbar a 80 %, vermelho fechado a 100 %). Claude pela statusline (`ask-agent --usage` → `POST /usage`), Codex pelos `rollout-*.jsonl`, Antigravity pela API de cota, Ollama local (`core/src/usage.rs`).
  - Seletor de workspace no topo da barra lateral (lista de projetos, "Abrir pasta…"); explorador sem a linha raiz, com atualizar e recolher tudo.
  - O diff do Git deixou de alargar a barra direita: rola dentro dela.

- [x] **Tela dividida por tipo de agente.**
  - Uma janela por tipo (todos os Ollama juntos), numa grade automática (2 lado a lado, 2+1, 2×2, 3+2, 3×2); os tipos que não cabem viram abas da última janela.
  - Abas no cabeçalho de cada janela (saíram da topbar), "+" por janela, contorno na janela focada, Ctrl+Tab dentro da janela e Ctrl+Alt+setas entre janelas.
  - Divisores arrastáveis (duplo clique iguala), troca de lugar pela alça ⋮⋮ e botão liga/desliga; ordem, proporções e liga/desliga salvos em `Config`.
  - Cálculo puro e testado em `crates/app/src/split.rs`.
- [x] **Barra direita mais estreita.** Padrão de 380 para 330 px e mínimo de 300 para 280 px (quem tinha o padrão antigo salvo passa ao novo); o detalhe do download termina em reticências.
- [x] **Gemini removido.** Substituído pelo Antigravity em todo o projeto (catálogo de agentes, headless, prompts, ditado, testes e documentos). Só os filtros da pasta `.gemini` ficaram: é a configuração do próprio Antigravity.

- [x] **Aba Git e ícones no cabeçalho da barra direita.**
  - Conversa e Git viraram ícones (balões, branch) com dica ao passar o mouse; o número de mensagens novas fica sobre o ícone da Conversa.
  - Git local do workspace (`core/src/git.rs`): status, diff, preparar/tirar/descartar, commit (e corrigir o último), branches, buscar/puxar/enviar e `git init`, sempre sem shell e sem prompts que travam. Testado com o git real em `tests/git.rs` (inclusive conflito e push/pull num remoto local).
  - Chip da branch na topbar; releitura automática por `CoreEvent::GitChanged` (watcher do `.git/`).

- [x] **Barra lateral direita fixa com a Conversa.**
  - A Conversa integrada, o compositor e o menu "Enviar para" saíram de baixo dos terminais para uma barra à direita que não recolhe e tem largura ajustável (salva em `Config.right_panel_width`; `bottomHeight`/`feedCollapsed` antigos são ignorados).
  - Compositor redesenhado para a barra estreita; menus ficam dentro da tela e rolam quando não cabem (corrigido também o `constrain` do menu "+", que fechava o menu no clique).
  - Uma pergunta com resposta atrasada (202) deixou de causar falsos "ciclo de espera".

- [x] **Segurança da ponte e das abas.**
  - A colagem nas abas remove ESC e caracteres de controle: uma mensagem não fecha mais o *bracketed paste* para virar teclas em outra aba.
  - Cada processo de agente tem token próprio; a ponte sabe quem pergunta pelo token, não pelo `from`, e só a instância perguntada responde (`ask-agent --reply <n>`).
  - Ids validados (sem aspas, espaços nem controle).
  - No Windows, shims `.cmd` não recebem texto livre na linha de comando.
  - Limites: 16 MB por requisição, 128 requisições simultâneas, `Host` obrigatório, token comparado em tempo constante.
- [x] **Profundidade e perguntas órfãs entre abas.** O limite de profundidade vale também no modo abas (medido pela cadeia de perguntas pendentes), e fechar a aba de quem perguntou descarta as perguntas dela.
- [x] **Entrega previsível nas abas** (`crates/core/src/delivery.rs`, com testes).
  - O prazo conta desde a entrada na fila.
  - Nada é digitado logo após uma tecla do usuário (nem com texto começado).
  - Hooks do Claude/Codex marcam início e fim de turno.
  - O Enter sai em ordem com as teclas do usuário.
  - A resposta lenta é entregue depois na aba de quem perguntou, em vez de prender o `ask-agent` (que estourava o timeout de 2 min do Bash do Claude).
- [x] **Codex e OpenCode** no catálogo de agentes (abas e headless).
- [x] **Ambiente isolado.** Nenhuma `COMMANDDECK_*` herdada vaza para os agentes, e uma segunda janela na mesma pasta de dados é recusada com uma mensagem clara.
- [x] **Testes independentes da máquina** (`StartOptions::system_dirs = false`), teste do spawn do Claude corrigido, `rustfmt.toml` + `cargo fmt --check` na CI.

- [x] **Download do Whisper robusto** (motor e modelo): retomada por `Range`, retry com espera crescente, detecção de conexão parada, redirecionamentos, verificação de SHA-256 e barra de progresso no cabeçalho da Conversa integrada; suporte a Linux. Módulo `crates/core/src/download.rs`.
- [x] **Mouse no terminal** (`crates/app/src/terminal/view.rs`, `input.rs`): clique, soltar, arrasto e movimento são enviados ao programa quando ele ativa os modos 1000/1002/1003, em SGR (1006) ou no formato legado; Shift devolve o mouse ao terminal para selecionar texto. Coberto por testes headless com eventos reais do egui.
- [x] **IME no terminal**: com o terminal focado o IME do sistema é habilitado ancorado no cursor; a confirmação vai ao processo como texto digitado e a pré-edição aparece sublinhada sobre o cursor (antes o evento era descartado). Falta só testar com um IME real (teclado CJK, `Win+.`).
- [x] **Instâncias com identidade e conversa entre abas reais.** Cada aba é `modelo[@cargo[@nome]]`; `ask-agent` digita a mensagem no terminal da aba (abrindo-a se preciso, inclusive um segundo Claude) e a resposta volta por `ask-agent` de volta, sem raspar a tela; ciclos de espera são recusados e fechar a aba libera quem esperava. Verificado ao vivo com duas abas do Claude, Claude ↔ Antigravity, dois Ollama (4B e 9B) e um orquestrador delegando a `claude@dev` e `claude@design`. Ver `ARCHITECTURE.md`.
- [x] **Colar no Windows derrubava o app** (corrupção de heap): o clipboard agora só é lido na thread da interface. Reproduzido e corrigido.
- [x] **Validação no Windows** (feita à mão): abas, ConPTY, microfone/Whisper, som, arrastar arquivos, colar imagem e restauração de abas.

## Auditoria de 2026-10-15

Todos os achados foram corrigidos (ver "Auditoria de 2026-10-15 (regressão + correções)" acima). A melhoria 5 do
relatório do Antigravity (tentar o motor da CPU quando o da GPU sai com código 0 sem saída) ficou de fora.

# Pendências

Lista do **trabalho de desenvolvimento** que falta (melhorias e limitações conhecidas), em ordem sugerida de
prioridade. Ao concluir um item, mova-o para "Concluído" com uma linha do que foi feito (regras dos documentos em
[GERAL.md](GERAL.md)).

Os **testes manuais** (validar no Windows, com os agentes reais) ficam em [testes.md](testes.md). Um teste que
falhar vira item aqui.

## 0. Direção: IA local como serviço interno (adiado)

> **Adiado** até o projeto estar mais completo.

Decisão: com Claude e Antigravity disponíveis, o Ollama deixa de ser "agente par" e vira **infraestrutura** do app.

- [ ] **Embeddings locais para RAG.** Modelo de embeddings no Ollama (`nomic-embed-text` ou `bge-m3`, < 1 GB, cabe na RX 7600 8 GB).
  - Indexar o workspace em pedaços (*chunks*) e manter o índice atualizado pelo watcher existente (`workspace.rs`).
  - Expor a busca de duas formas: `ask-agent`/CLI `search` (sob demanda) e um `ContextProvider` com orçamento fixo de tokens (ex.: 1.500).
  - Índice local (ex.: em `dados/`), sem enviar nada para fora.
- [ ] **Compressor de contexto.** Modelo pequeno (`qwen3:8b`, 100% GPU) para resumir:
  - respostas longas no `ask-agent` antes de entregar ao agente que perguntou (`bridge.rs`), mantendo o original na Conversa integrada.
- [ ] **Medir tokens** (`prompt_eval_count` do Ollama, tamanho dos prompts enviados a Claude/Antigravity) para comparar antes/depois.
- [ ] **Configuração** em `Config`: modelo de embeddings, modelo compressor, limiares (ex.: comprimir acima de N caracteres), ligar/desligar.
- [ ] Aba Ollama-agente passa a ser opcional (os itens da seção 1 ficam como baixa prioridade).

## 1. Agente local (Ollama) — adiado: contexto e comportamento

> **Adiado** até o projeto estar mais completo.

Problema observado: o modelo local esquece que é um agente ("age como chatbot") e para no meio da colaboração com outros agentes (ex.: Antigravity) em vez de continuar a conversa sozinho.

- [ ] **Configurar `num_ctx`**. O Ollama usa por padrão uma janela de contexto pequena (2–4k tokens). O prompt de sistema, as 6 ferramentas e o `AGENTS.md` ocupam boa parte dela, e o começo, onde está o "você é um agente", acaba cortado. Enviar `options.num_ctx` nas chamadas a `/api/chat` (ex.: 16k–32k), configurável em `Config`.
  - Arquivo: `crates/core/src/ollama.rs` e `crates/core/src/config.rs`.
- [ ] **Prompt de sistema mais curto para modelos pequenos (4B–9B).** O atual é longo e cheio de regras em caixa alta, que modelos pequenos seguem mal. Ter uma versão enxuta.
- [ ] **Instrução de colaboração mais perto do fim do contexto.** O lembrete de "continue conversando com o agente parceiro" hoje vai no fim de uma saída grande do `ask-agent`. Avaliar reinjetar a instrução como mensagem curta de sistema/usuário logo após o resultado.
- [ ] **Verificação antes de encerrar.** Hoje, quando o modelo responde em texto sem chamar ferramenta, o loop considera a tarefa concluída. Adicionar um passo de checagem ("a tarefa terminou ou falta chamar `ask-agent`/ferramentas?") antes de devolver a resposta.
- [ ] Revisar o limite de 25 passos (`MAX_STEPS`) para conversas longas entre agentes.

## 2. Injeção de contexto como um ponto único (para qualquer IA) — adiado

> **Adiado** (gestão de contexto) até o projeto estar mais completo.

Hoje cada agente recebe instruções por um caminho diferente:

| Agente | Como recebe as instruções hoje |
| :-- | :-- |
| Claude | `--append-system-prompt` (`agents.rs` / `headless.rs`) |
| Antigravity | bloco marcado no `AGENTS.md` do workspace |
| Ollama | prompt fixo em `ollama.rs` (`system_prompt`) |

Proposta:

- [ ] **Perfis de agente em arquivo**, editáveis sem recompilar:
  - `prompts/_comum.md`: regras de colaboração válidas para todos (usar `ask-agent`, não resumir a Conversa integrada, não parar para perguntar ao usuário etc.);
  - `prompts/<agente>.md`: ajustes por agente (`ollama.md`, `claude.md`, `antigravity.md`...).
- [ ] **`ContextProvider` para todos os agentes.** O trait (`crates/core/src/context.rs`) hoje só é consultado pelo Ollama. Passar a consultá-lo também no executor headless e no spawn das abas, entregando o contexto pelo canal que cada CLI suporta (flag, `AGENTS.md` ou mensagem de sistema).
- [ ] **RAG local/remoto** como um `ContextProvider` registrado via `Core::register_context_provider`.

## 3. Terminal

- [ ] **Claude Code duplica a conversa no histórico a cada mudança de largura** (Windows). Ao mudar de largura, ele limpa só a tela e reimprime a conversa, sem `ESC[3J` (que ele só manda fora do Windows). A cópia anterior fica no histórico, na largura antiga. Medido numa sonda (portable-pty + alacritty_terminal + o claude.exe real): uma cópia a mais por largura; o mesmo com o `OpenConsole.exe` do Windows Terminal. Limpar o nosso histórico antes não serve: ao alargar, ele redesenha só a tela e a conversa sumiria. Hoje o PTY só muda com o mouse solto e o tamanho parado por 150 ms (um arrasto = uma cópia). Ideia para depois: marcar onde começa a reimpressão e descartar o trecho antigo do histórico.

## 4. Interface

- [ ] **Identidade visual e temas** (pedido do usuário, 18/10). O app funciona, mas o visual é inconsistente: a barra lateral esquerda parece de outro tema que a barra direita, e algumas partes estão sem personalidade.
  - Primeiro, padronizar: revisar barra lateral, barra direita, barra de título, abas e modais com as mesmas cores, espaçamentos, cantos e estados (hover, seleção), tudo vindo dos tokens de `theme.rs`, sem cor fixa espalhada pelo código.
  - Depois, temas para o usuário escolher: trocar as constantes de `theme.rs` por uma paleta carregada em tempo de execução, escolhida nas configurações e gravada em `Config`. A paleta inclui o fundo e as 16 cores do terminal (`TERM_BG`/`TERM_FG`, `terminal/colors.rs`), então um tema muda o fundo de todas as abas de uma vez (CLIs com tema próprio, como o OpenCode, continuam usando o tema deles).

- [ ] Janela estreita (perto do mínimo de 820 px): com muitas abas, as de trás encolhem até virar só o ponto colorido.

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.
- [ ] **Destacar o agente selecionado** (seção ORGANIZAÇÃO da barra lateral): realçar a janela/aba do agente escolhido e o que é dele (pedido do usuário; a seção já existe com a chave da tela dividida).
- [ ] **Tela dividida com destaque**: terceiro modo de organização (além de aba única e tela dividida), em que a janela do agente/projeto selecionado fica grande e as outras menores ao lado. Entra na seção ORGANIZAÇÃO (que continua seção, não vira ícone).
- [ ] **Equipe: as instruções da ponte são fixadas na partida.** Um agente aberto antes desta versão não sabe do `--team`/`--boss` até reabrir a aba. O aviso de equipe só chega com a próxima mensagem (de propósito, para não gastar turno).
- [ ] **Equipe: teste de integração do "chefe na criação" só roda em Unix** (`an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told`, usa agente falso em shell); ver a seção 5.
- [ ] **Contexto de curto/longo prazo** (injeção de contexto + memória): só discutir por enquanto (ver seção 2).
- [ ] **Projetos: lógica da Deck sem teste automático** (filtro das abas visíveis, gravação por projeto, carga sob demanda); hoje só o core tem testes. Extrair para funções puras testáveis, no estilo de `split.rs`.
- [ ] **Projetos: o AGENTS.md de projetos não ativos** só é reescrito quando se abre a próxima aba deles (a lista de agentes pode ficar velha até lá).
- [ ] **Projetos: a árvore e o Git não guardam o estado por projeto** (pastas abertas, arquivo selecionado): voltam do zero a cada troca.
- [ ] Com a barra lateral recolhida, a chave da tela dividida (agora na ORGANIZAÇÃO) fica inacessível: avaliar um atalho de teclado.
- [ ] **Uso do Antigravity com o token vencido:** o app usa só o token que o CLI mantém (sem guardar client ID/secret de terceiros), então com o CLI fechado por mais de ~1 h a barra mostra "sem dados". Avaliar ler o client ID/secret do CLI instalado para renovar em memória.
- [ ] **Uso do Antigravity depende de API interna** (`v1internal:retrieveUserQuotaSummary`): pode mudar sem aviso; hoje qualquer falha vira "sem dados". Avaliar um aviso discreto quando falhar por muito tempo.
- [ ] **Uso do Codex só testado com arquivos de exemplo** (o Codex não estava instalado): conferir o formato real de `rate_limits` nos `rollout-*.jsonl`.
- [ ] **Statusline do usuário perde o `padding`**: a nossa (`ask-agent --usage`) fixa 0 e repassa a dele.
- [ ] **Janela sem decoração:** validar no Windows real (arrastar, snap, Win+setas, redimensionar pelas bordas, maximizada sem cobrir a barra de tarefas); no Linux, conferir X11/Wayland ([testes.md](testes.md), seção 12).

## 5. Testes automáticos

- [ ] Os testes de integração do fluxo entre abas (`a_question_is_typed_into_the_tab_and_answered_by_callback` e vizinhos) só rodam em Unix (agentes falsos em shell): criar agentes falsos para Windows (`.cmd`/`.exe` de teste) para a CI do Windows cobrir a ponte.
- [ ] Testes de interface (egui) para a barra direita, a aba Git e a tela dividida (hoje só capturas de tela no Linux).
- [ ] Tela dividida: com várias janelas, cada uma redesenha a grade inteira a cada quadro (e qualquer saída de qualquer aba pede um quadro). Se pesar com 5–6 janelas, guardar as linhas que não mudaram (cache por aba).
- [ ] Tela dividida, depois: arrastar uma **aba** para outra janela (hoje o agrupamento é sempre pelo tipo) e maximizar uma janela temporariamente.

## 6. Novas funcionalidades (comparação com o Orca)

- [ ] **GitHub, depois:** comentar e aprovar PRs, criar issue e reexecutar um job pelo app (hoje é só leitura + "Enviar para agente"); o caminho `gh auth token` não foi exercitado (o `gh` não está instalado aqui).
- [ ] **Editor de arquivos.** Abrir e editar arquivos do workspace dentro do app, em vez de mandar para o programa padrão do sistema (`Core::open_path`).
  - egui não tem editor de código pronto: avaliar `egui_extras::syntax_highlighting`/`syntect` para realce, ou um componente de editor de terceiros.
  - Mínimo útil: abas de arquivo ao lado das abas de terminal, realce de sintaxe, salvar (Ctrl+S), aviso quando um agente alterar o arquivo aberto (o watcher de `workspace.rs` já avisa) e pré-visualização de Markdown.
  - Clique na árvore continua inserindo `@arquivo`; o duplo clique passaria a abrir no editor (com opção de manter o programa do sistema).

## Instâncias (abas) — pontos em aberto

- [ ] Pré-autorizar o `ask-agent` nos CLIs que pedirem aprovação na aba (hoje só o Claude abre com `--allowedTools`; quais pedem é verificado em [testes.md](testes.md), seção 7).
- [ ] Se o agente da aba não responder por `ask-agent` (modelo fraco, ou o usuário assumiu o terminal), a pergunta expira em 10 minutos. Quem perguntou não fica mais bloqueado (recebe o aviso por escrito no terminal), mas só o `ollama-agent` responde sozinho.
- [ ] Hooks de início/fim de turno só no Claude e no Codex (este só avisa o fim, e só quando não é shim `.cmd`). Antigravity e os demais seguem a heurística da saída (1,5 s parado / 30 s na fila).
- [ ] Falta `ask-agent --cancel <n>` para desistir de uma pergunta (hoje ela expira em 10 min).
- [ ] A ponte não percebe quando o cliente (`ask-agent`) cai no meio da espera (limitação do `tiny_http`); a pergunta fica pendente até ser respondida ou expirar.
- [ ] No headless, aider (`--yes-always`) e interpreter (`-y`) rodam sem aprovação recebendo texto de outros agentes. Avaliar modos mais restritos.
- [ ] A ponte sem interface ainda usa o modo headless: não aparece nas abas.
- [ ] **Mesa de tarefas, depois:** pedido e relatório aparecem como texto puro (o markdown não é desenhado); a linha do tempo das mensagens entre agentes de cada tarefa (para aposentar a Conversa integrada); rodar os testes do projeto ao receber o relatório; editar uma tarefa em rascunho; os dados de demonstração (`tasks-demo`) usam nomes de modelos antigos.
- [ ] **Modelo por instância, depois:** trocar o modelo de uma aba aberta (hoje o menu só mostra); lista de modelos do Codex no "+ Novo agente" (hoje só por `--model`/`roleModels`); campo livre para um modelo fora da lista; editar `roleModels` pela interface (hoje só no config.json). No OpenCode a variante (`modelo#alta`) só vale no headless, e cada aba com modelo sobe um servidor próprio (`--standalone`).
- [ ] **OpenCode: escolher o provedor antes do modelo** (pedido do usuário, 2026-10-18). Hoje o `opencode models` mistura todos os provedores conectados (`/connect`), e o formulário de nova tarefa oferece modelos sem acesso: a `t1` foi registrada com `opencode-go/deepseek-v4.1-flash` (plano Go, que o usuário não tem; 0 tokens no `opencode stats`). Fazer em dois passos: provedor (`opencode`, `opencode-go`, `ollama`…, o prefixo antes da `/`) e depois os modelos daquele provedor. Avaliar destacar os `-free`.
- [ ] **Mesa de tarefas: áudio e anexos no pedido** (pedido do usuário, 2026-10-18). O formulário "Nova tarefa" só aceita texto. Reaproveitar o microfone (Whisper local) e os anexos do compositor (`ui/composer.rs`, cópia em `dados/anexos`), com os anexos citados no arquivo do pedido.
- [ ] **Ditado em qualquer aba de CLI** (pedido do usuário, 2026-10-18). Hoje o áudio só sai pelo compositor, que envia para a aba ativa: dá para mandar para o modelo errado. O usuário só não aposenta a Conversa integrada por causa do ditado. Proposta: microfone (e atalho) presos à aba do terminal, que grava, transcreve e digita o texto no PTY daquela aba **sem Enter**, para conferir antes de enviar. Funciona em qualquer CLI, porque é só entrada de teclado; o destino fica fixo na aba onde a gravação começou.

## 7. Git

- [ ] **O `AGENTS.md` que o app grava aparece como alterado/não rastreado** no repositório do usuário. Desde a auditoria, com o bloco igual ao do commit o conteúdo não muda (conferido em 2026-10-16 com `core.autocrlf=true`: mesmo hash do HEAD). Continua aparecendo quando a lista de agentes muda (outra máquina, outros modelos do Ollama) e, em repositório sem `AGENTS.md`, como não rastreado. Avaliar escrever as instruções fora do arquivo versionado (ex.: `.git/info/exclude`, ou só pelo canal de cada CLI).
- [ ] Com `commit.gpgsign` e pinentry gráfico, a chave travada é recusada na hora (com a explicação de como destravar numa aba) em vez de abrir o diálogo de senha. Avaliar deixar o diálogo abrir quando houver pinentry gráfico.

## 8. Auditoria de 2026-10-15

Todos os achados foram corrigidos (ver "Concluído"). Relatório do Antigravity em `dados/anexos/revisao-antigravity.md`;
a melhoria 5 dele (tentar o motor da CPU quando o da GPU sai com código 0 sem saída) ficou de fora.

## Concluído

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
  - Aparece na aba, na barra lateral, em `--list` e em `--team`, e o "+ Novo agente" tem um segundo passo com os modelos conhecidos (`agy models`, `opencode models`, numa thread com cache).
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

# Arquitetura do AgentDeck

Visão geral e mapa dos documentos: [GERAL.md](GERAL.md). Guia prático e mapa de arquivos: [MANUTENCAO.md](MANUTENCAO.md).

Aplicação desktop nativa em Rust (reescrita da antiga versão Electron, sem Node em tempo de execução).

## Stack e decisões de base

| Camada | Escolha | Por quê |
| :-- | :-- | :-- |
| Janela e eventos | **winit 0.30** (`ApplicationHandler`) | controle total do loop de eventos, arrastar e soltar e janela sem decoração; sob WSLg força X11 (`app/src/wslg.rs`) |
| Renderização e UI | **glutin + glow** com **egui 0.36** (`egui-winit` + `egui_glow`) | modo imediato estilizado à mão (`theme.rs`); OpenGL leve que roda até em `Xvfb`/llvmpipe no CI |
| Terminal | **alacritty_terminal** (parser VT + grade) + **portable-pty** (ConPTY no Windows, PTY no Unix) | emulação VT madura; pintura própria da grade no egui (`app/src/terminal/`) e resize com debounce para compensar a repintura do ConPTY |
| Ponte HTTP | **tiny_http** (thread por requisição, até 128 simultâneas) | servidor local em `127.0.0.1:0` para o `ask-agent`, autenticado por token de processo + validação de `Host` |
| Cliente HTTP | **ureq** (com rustls) | síncrono e sem OpenSSL; roda em threads para GitHub, cota do Antigravity e downloads |
| Sistema e arquivos | **notify**, **rfd** (portal no Linux), **open**, **arboard**, **tray-icon** / **ksni**, **secret-service** (D-Bus no Linux) | integrações nativas sem puxar GTK3 |
| Áudio e ditado | **cpal** (feature `audio`) + **whisper.cpp** (`whisper/`) | transcrição local opcional; áudio apagado logo após transcrever |

## Crates e fronteiras

```
crates/
├── core   (agentdeck-core)   backend sem interface gráfica
├── app    (agentdeck)        janela winit + egui e emulador de terminal
└── cli    (agentdeck-cli)    binário ask-agent
```

- **Fronteira `app` ↔ `core`:** a interface só fala com o backend pelos métodos de `agentdeck_core::Core` (`crates/core/src/lib.rs`) e recebe eventos por canal (`events.rs`), acordando o loop do winit via `EventLoopProxy`. Operações bloqueantes (git, GitHub, listagem de modelos, transcrição, leitura/gravação de documentos) rodam sempre em threads fora da UI.
- **Trabalho com e sem quadro (`crates/app/src/app.rs`, `crates/app/src/deck/`):** o que não depende de desenhar (eventos do core, respostas de threads, entregas de `ask-agent` nos terminais, respostas atrasadas, Enter agendado, fim de turno e temporizadores) roda antes de cada quadro com a janela visível e, quando a janela está na bandeja, minimizada ou com área 0×0 (no Windows não há `WM_PAINT`), num passo de segundo plano no loop de eventos a cada ≥ 50 ms (`ControlFlow::WaitUntil`). Sem quadro o PTY nunca é redimensionado às cegas.
- **Extensão para RAG (`crates/core/src/context.rs`):** trait `ContextProvider`, consultado pelo executor headless antes de enviar o prompt.

---

## Protocolo entre agentes e contrato da ponte (`ask-agent`)

### Ambiente de cada processo de agente (`crates/core/src/agents.rs`, `env.rs`)

Cada aba interativa (e execução headless) recebe um ambiente higienizado:

- **Injetadas:** `AGENTDECK_PORT` (porta TCP em `127.0.0.1`), `AGENTDECK_TOKEN` (token de 128 bits **exclusivo da aba ou execução**), `AGENTDECK_AGENT` (id da instância, ex.: `claude@dev`), `AGENTDECK_ROLE` (quando há cargo), `AGENTDECK_DEPTH` (profundidade de delegação), `PATH` (com a pasta dos executáveis na frente), `TERM=xterm-256color`, `COLORTERM=truecolor` e, no Windows, `PYTHONUTF8=1`, `PYTHONIOENCODING=utf-8`, `MSYS_NO_PATHCONV=1` e `MSYS2_ARG_CONV_EXCL=*` (impede o Git Bash de converter `/comando` em caminho).
- **Filtradas:** qualquer `AGENTDECK_*` herdada do ambiente externo (exceto `AGENTDECK_ASK_WAIT`, configuração do usuário), variáveis de sessão aninhada do Claude (`CLAUDECODE`, `CLAUDE_CODE_*`), `ELECTRON_RUN_AS_NODE` e `OPENCODE_CONFIG_CONTENT` (inserida depois só para a aba do OpenCode a que pertence).

### Instruções do AgentDeck e confiança da pasta (`agents.rs`, `trust.rs`, `workspace.rs`)

As instruções da ponte (uso do `ask-agent`, proibição de subagentes internos do CLI sem pedido do usuário, `--models`/`--team`/`--learn`/`--outline` e `memory/aprendizados/`) chegam pelo canal próprio de cada CLI, sem depender de um `AGENTS.md` prévio:

| CLI | Canal de entrega das instruções |
| :-- | :-- |
| Claude | `--append-system-prompt <texto>` (inclui nome e cargo), `--allowedTools "Bash(ask-agent:*)"` e `--settings dados/hooks/claude-settings.json` |
| Codex | `-c developer_instructions=<TOML>` (ou texto cru em shim `.cmd`) e, fora de shim, `-c notify=["ask-agent","--signal","idle"]` |
| OpenCode | `--standalone` + `OPENCODE_CONFIG_CONTENT={"model":…,"instructions":["dados/agentdeck-instructions.md"]}` (`models.rs`) |
| Aider | `--read dados/agentdeck-instructions.md` |
| Open Interpreter | `--custom_instructions <texto>` |
| Antigravity | `--add-dir dados/agy/<instância>` (onde o app grava um `AGENTS.md` com nome e cargo da instância, somado ao do projeto pelo `agy`) |
| Goose, Copilot | leem apenas o `AGENTS.md` da pasta: se não existir, o app cria um só com o bloco `<!-- agentdeck:start/end -->` e avisa na Conversa; se já existir, atualiza só o bloco |
| Amazon Q, Terminal | sem canal de instruções |

O texto em arquivo compartilhado (`dados/agentdeck-instructions.md` e bloco de `AGENTS.md`) não lista os agentes instalados na máquina, para não sujar o `git status` entre computadores.

**Confiança da pasta (`crates/core/src/trust.rs`):** antes de abrir uma aba, o app registra a confiança da pasta de trabalho e da pasta do projeto para que abas abertas em segundo plano não travem em *"Do you trust this folder?"*: no Claude em `~/.claude.json` (se não houver pasta-mãe já aceita), no Codex via `-c projects={…}` na linha de comando e no Antigravity em `trustedWorkspaces` de `~/.gemini/antigravity-cli/settings.json` (caminho exato, removido junto com a worktree da tarefa). Pasta pessoal e raiz de disco nunca são adicionadas.

### Identidade por token e isolamento por projeto (`bridge.rs`, `instances.rs`)

- **Identidade:** a ponte identifica quem chama pelo cabeçalho `x-agentdeck-token` (token por instância aberta ou por execução headless). O campo `"from"` do JSON é ignorado para tokens de agente; só o token mestre do próprio app pode informar `"from"` e `"project"`.
- **Escopo de projeto:** o token carrega o projeto (`p` + 8 hex). Internamente, perguntas pendentes, filas por par, atividade e caixa de entrada usam a chave `<projeto>#<id>`, permitindo que dois projetos tenham cada um o seu `claude@dev` sem cruzar mensagens nem ciclos. Para agentes e usuários, os ids aparecem sem o prefixo.

### Contrato HTTP da ponte (`crates/core/src/bridge.rs`)

Toda requisição exige `x-agentdeck-token` (comparado em tempo constante) e `Host: 127.0.0.1:<porta>` ou `localhost:<porta>` (outro valor ou ausente → **403**). Limites globais: até **128** requisições simultâneas (**503** acima; perguntas em HTTP 202 ocupam vaga até a resposta atrasada terminar), corpo individual até **16 MB** (**413** acima) e soma dos corpos em leitura/atendimento até **64 MB** (**503** acima).

| Rota | Corpo / parâmetros | Resposta e códigos HTTP |
| :-- | :-- | :-- |
| `POST /activate` | `{}` (só token mestre) | **200** `{"ok":true}` e traz a janela à frente; **403** para token de agente |
| `GET /agents` | — | **200** `{"agents":[…],"models":{id:modelo},"quota":{id:cota}}` do projeto do chamador |
| `GET /models[?agent=<id>]` | `agent` opcional | **200** `{"text"}` com o inventário (`ask-agent --models`): os 4 CLIs oficiais do mais folgado ao mais gasto, linha `cota:` por grupo e bloco `capacidade`; **400** se inválido; **404** se não instalado |
| `GET /team` | — (só token de aba) | **200** `{"id","boss","crew","models","quota"}` e consome aviso de mudança de equipe; **400** com token mestre |
| `POST /ask` | `{"to","message","depth","new","wait"[,"reply_to"][,"model"][,"ack"]}` (+ `"from"`, `"project"` com token mestre) | **200** `{"answer","request"}` no prazo; **202** `{"pending":true,"answer","request"}` após `wait` (resposta digitada depois na aba); **400** destino desconhecido, auto-pergunta, `depth >= 2`, `model` inválido ou CLI sem escolha de modelo; **404** `reply_to` inexistente; **409** ciclo de espera, instância já aberta com outro modelo (sugere `--new`) ou cota esgotada (≥ 99,5%); **413** > 16 MB; **500** erro ou timeout (10 min) |
| `POST /reply` | `{"to","message"[,"reply_to"]}` | **200** `{"answer"}` concluindo a pergunta pendente; **400** argumentos inválidos; **404** sem pergunta pendente daquele remetente |
| `GET /inbox[?all=1]` | — (só token de aba) | **200** `{"answers":[{"request","from","text","failed","ts","delivered"}]}` (sem `all`, não entregues; `all=1`, 20 últimas) e marca como entregues; **400** com token mestre |
| `POST /inbox/delivered` | `{"request":<n>}` (só token de aba) | **200** `{"ok":true}` confirmando entrega da resposta `<n>` após imprimir no stdout (quando `/ask` enviou `"ack":true`); **400** inválido/token mestre; **404** não pertence ao chamador |
| `POST /learn` | `{"paths":[…]}` (até 50 caminhos) | **200** `{"text"}` com aprendizados e situação atual (sem caminhos) ou avisos de choque + aprendizados que casam (com caminhos); **400** sem repositório |
| `POST /outline` | `{"paths":[…]}` (1 a 50 arquivos) | **200** `{"text"}` com assinaturas e linhas de cada arquivo; **400** sem arquivos ou sem repositório |
| `POST /signal` | `{"state":"busy"\|"idle"}` | **200** `{"ok":true}` atualizando o estado de turno da aba (só token de aba) |
| `POST /usage` | `{"session":{"used","resets_at_ms"}\|null,"weekly":…}` | **200** `{"ok":true}` registrando uso do plano (`0.0..=1.0`, só token de aba; `shell` → **400**) |
| `GET /tasks`, `GET /tasks/<id>` | `?project=<id>` opcional com token mestre | **200** `{"tasks":[…]}` (resumo com `scope` e `waiting_for`) ou `{"task":{…}}` completo; **404** não existe |
| `POST /tasks` | `{"title","brief"[,"kind"][,"model"][,"effort"][,"level"][,"base"][,"scope"][,"start"]}` | **200** `{"task"}` (com `start:true`, em `running` ou `queued`); **400** dados inválidos, escopo inválido ou sem git; **403** agente de tarefa ativa |
| `POST /tasks/<id>/start`, `/merge`, `/validate`, `/discard` | `{}` | **200** `{"task"}`; **403** agente de tarefa; **404** não existe; **409** estado incompatível, árvore suja, conflito de merge ou worktree presa; **500** falha do git |
| `POST /tasks/<id>/done` | `{"report"}` | **200** `{"task"}` passando a `review` (só dono da tarefa ou token mestre; vale em `running` ou `validating` com commits novos); **400** vazio; **403** outro agente; **409** estado inválido |
| `POST /tasks/<id>/return`, `/reject` | `{"feedback"}` | **200** `{"task"}` devolvendo de `review` (`return`) ou reprovando de `validating` (`reject`) para `running`; **400** vazio; **403** agente de tarefa; **409** estado inválido |

**Códigos de saída do `ask-agent` (`crates/cli/src/bin/ask-agent.rs`):** `0` sucesso, `1` erro da ponte/agente, `2` erro de argumentos ou ambiente fora do AgentDeck (`--signal` sai sempre com `0` para não quebrar hooks).

### Instância única e bandeja (`activation.rs`, `app/src/tray.rs`)

- **Segunda abertura:** o core mantém a trava `dados/.agentdeck.lock` e grava porta + token mestre em `dados/.agentdeck.bridge`. Uma segunda abertura detecta a trava ocupada, chama `AllowSetForegroundWindow` (no Windows) + `POST /activate` e encerra; a instância em execução restaura e foca a janela.
- **Bandeja:** fechar a janela (×, `Alt+F4` ou botão da barra de título) esconde a janela na bandeja (`tray-icon` no Windows, `ksni`/StatusNotifierItem no Linux) mantendo agentes e ponte ativos em segundo plano; sem bandeja no Linux, minimiza. Só **Encerrar** no menu da bandeja fecha o app, pedindo confirmação na janela se houver documento não salvo ou agente em turno.

---

## Instâncias, equipe, modelos, cota e capacidade

- **Identidade e equipe (`instances.rs`, `team.rs`):** instâncias usam `<modelo>[@<cargo>[@<nome>]]` (`claude`, `claude@dev`, `claude@dev@ana`, `claude@main@bia`). Quando um agente cria outra instância via `ask-agent`, torna-se chefe dela; o usuário reorganiza arrastando na barra lateral (ciclos são recusados). Fechar uma aba repassa os subordinados ao chefe dela; renomear preserva a árvore. Mudanças de posição geram aviso anexado à próxima mensagem entregue na aba (ou consumido por `ask-agent --team`). A equipe orienta, mas não bloqueia chamadas diretas.
- **Modelo de LLM e esforço (`models.rs`):** resolvido ao abrir a aba na ordem: (1) pedido explícito (`--model` ou menu), (2) modelo já salvo para a instância no projeto, (3) padrão do cargo/CLI na configuração, (4) padrão do próprio CLI. Só `claude`, `antigravity`, `codex` e `opencode` aceitam modelo; pedir outro modelo para aba já aberta retorna **409**. Passado como `--model <m>` em `claude`, `agy` e `codex` (e `opencode run -m` no headless); na TUI do OpenCode vai via `--standalone` + `OPENCODE_CONFIG_CONTENT`. Modelos de `agy` e `opencode` são listados em thread (timeout de 15 s, cache de 10 min, `-free` primeiro). O esforço (`low`, `medium`, `high`) vira `--effort` no Claude e sufixo `-low|medium|high` no Antigravity (recusado com 400 nos demais).
- **Uso dos planos e cota (`usage.rs`, `usage/codex.rs`, `usage/antigravity.rs`):** acompanha sessão (5 h) e semana (`0.0..=1.0` + horário de renovação) e salva em `dados/usage.json`. No Claude vem do hook de `statusLine` (`ask-agent --usage` lê `rate_limits` do stdin, envia `POST /usage` e repassa o stdin à statusline do usuário); no Codex lê a cada 60 s o `rollout-*.jsonl` mais recente de `~/.codex/sessions`; no Antigravity consulta a cada 3 min `retrieveUserQuotaSummary` (fallback `fetchAvailableModels`) com a versão e o token OAuth do `agy` (Credenciais do Windows, Secret Service D-Bus no Linux ou arquivo), separando por grupo (`Gemini 4 Argon`, `Gemini Models`, `Claude and GPT models`). Na ponte, `/ask` recusa com **409** destinos com grupo ≥ 99,5% (leitura < 1 h) e avisa quando o destino passa de 80% havendo outro CLI oficial < 50%.
- **Capacidade e seleção para tarefas (`capability.rs`, `capability.json`):** cruza o nível da tarefa (`simples`, `media`, `complexa`) com o *AA Coding Agent Index v2.0* (`≥ 53` complexa, `40..53` média, `< 40` simples; esforço `low` desce um nível; sobreponível por `dados/capacidade.json`). Com ≥ 3 tarefas avaliadas de um modelo num nível (`dados/tarefas/*.json`), o histórico real prevalece (mais da metade descartada ou ≥ 2 devoluções desce um nível; 3 últimas do nível seguinte aprovadas de primeira sobem um nível). Sem agente explícito, a criação de tarefa escolhe entre os CLIs instalados capazes do nível o de menor cota usada; escolha explícita abaixo do nível gera aviso sem bloquear.

---

## Fluxo de perguntas entre abas e entrega nos terminais

### Pergunta, resposta e detecção de ciclos (`exchange.rs`, `inbox.rs`)

1. **Resolução e fila:** `POST /ask` resolve o destino no projeto do chamador (ou cria a instância e abre a aba em segundo plano, até 8 abas automáticas). Chamadas do mesmo par `de->para` seguem fila FIFO; no headless, sessões do par são retomadas no Claude (`--session-id`/`--resume`) e no Antigravity (`--conversation`).
2. **Ciclos e profundidade:** se o destino já espera direta ou indiretamente por quem pergunta, `/ask` recusa com **409**. A cadeia de perguntas pendentes entre abas é recusada a partir de profundidade 2.
3. **Resposta síncrona ou atrasada (HTTP 202):** o `ask-agent` espera até `AGENTDECK_ASK_WAIT` (padrão 90 s). Se `ask-agent --reply <n>` chegar no prazo, sai no stdout com HTTP 200 e o cliente confirma o recebimento em `POST /inbox/delivered` (pois o `tiny_http` não detecta se o cliente morreu antes de ler o 200). Se passar de 90 s, `/ask` devolve **202** liberando o `ask-agent` (a pergunta deixa de bloquear ciclos, mas segue contando na profundidade) e, quando o destino responder (em até 10 min), a resposta é digitada no terminal de quem perguntou. Abas `shell` e execuções headless esperam sempre até os 10 min.
4. **Caixa de entrada (`inbox.rs`):** toda resposta (até 20 por instância) fica como "não entregue" até o `/inbox/delivered` ou a digitação na aba. Se a aba que perguntou fechou ou morreu, o app avisa na Conversa e a resposta fica disponível em `ask-agent --inbox`.

### Quando digitar no terminal (`crates/core/src/delivery.rs`, `crates/app/src/deck/`)

A política em `delivery.rs` evita atropelar o usuário ou a TUI:
- **Respeito ao usuário:** não digita nos 2 s após tecla do usuário nem nos 20 s enquanto há texto digitado ainda sem Enter.
- **Prontidão da aba:** aguarda 5 s na abertura de abas sem hooks; com hooks (Claude e Codex), aguarda o sinal `idle` + 300 ms sem saída (fallback de 10 min); sem hooks, 1,5 s sem saída ou 30 s esperando na fila.
- **Colagem e Enter:** limpa `ESC` e controles C0/C1 (protegendo o *bracketed paste*) e envia pela thread de escrita do PTY. O Enter vai 250 ms depois (mais um segundo Enter em 600 ms quando há `@menção` no Claude/Antigravity; no Codex do Windows soma 1 ms por caractere colado, até 10 s). Teclas do usuário durante a espera do Enter saem logo após ele.
- **Shims `.cmd` e processos no Windows (`agents.rs`, `headless.rs`, `process.rs`):** abas interativas abrem shims via `cmd.exe /d /e:on /v:off /c` com argumentos escapados; prompts livres nunca vão na linha de comando de um shim (no headless vão por arquivo em `dados/prompts/`). Processos headless e o `whisper-cli` entram num Job Object com `KILL_ON_JOB_CLOSE`.

---

## Mesa de tarefas (`crates/core/src/tasks/`, `scope.rs`, `crates/app/src/tasks_ui.rs`)

Cada tarefa (`t1`, `t2`…, contador por projeto nunca reaproveitado) isola um trabalho num agente (`<modelo>@tN`), numa branch (`tarefa/<id>-<slug>`) e numa worktree própria.

```
Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged ("concluída")
  │  │              ▲  ▲  ▲           │                  │ └─discard──▶ Discarded (de qualquer estado não final)
  │  │              │  │  └──return───┘                  │
  │  │              │  └────────────reject───────────────┘
  │  └─ escopo ocupado ▶ Queued ──(a outra sai de Running/Review)──▶ Running
  └─ erro ao iniciar ▶ Failed ──start de novo──▶ Running
```

- **Armazenamento e worktrees (`tasks/store.rs`, `tasks/git_ops.rs`):** salvas em `dados/tarefas/<projeto>.json` (fora do repo; remover e readicionar a pasta preserva o id do projeto). As worktrees ficam em `<raiz do repo>/.agentdeck/worktrees/<id>` — dentro do repositório para que agentes confinados ao workspace (como o Antigravity) tenham acesso — e `/.agentdeck/` é registrado em `<git comum>/info/exclude` (nunca no `.gitignore`).
- **Início e entrega (`tasks/lifecycle.rs`, `tasks/text.rs`):** cria worktree e branch a partir da base, registra `<modelo>@<id>` subordinado a quem criou e entrega o pedido com memória e regras. Quando a aba precisa ser aberta, grava o pedido (ou feedback) em `dados/tarefas/<projeto>-<id>-brief.md` (ou `-feedback.md`) e passa uma linha curta em ASCII como argumento inicial do CLI (`claude "<linha>"`, `agy -i "<linha>"`, `opencode --prompt "<linha>"`), evitando perder texto enquanto a TUI inicializa.
- **Escopo e fila (`scope.rs`, `tasks/lifecycle.rs`):** o escopo lista caminhos relativos (sem globs); dois escopos colidem quando um é igual ou prefixo por componente do outro (`memory/` e `Cargo.lock` não reservam nada). Se colidir com tarefa em `Running`, `Review` ou na fila antes dela, entra em `Queued` sem criar worktree nem aba. Quando a tarefa bloqueante é mesclada, descartada ou falha ao iniciar, a fila avança sozinha criando a worktree a partir do topo atual da base (`Validating` não segura a fila). No `done`, arquivos tocados fora do escopo geram aviso na revisão.
- **Mesclagem (`tasks/merge.rs`, `tasks/stash.rs`):** exige repo principal na branch base e limpo (ignorando e restaurando o bloco automático do `AGENTS.md`) e worktree limpa. Faz `git merge --no-ff`, alinha a branch da tarefa (`git merge --ff-only`) e passa a `Validating` mantendo aba, worktree e branch vivas. Na UI, bloqueios oferecem resolução num clique — incluindo guardar alterações locais num `git stash`, mesclar e reaplicar pelo hash SHA após pré-verificar com `git merge-tree --write-tree`.
- **Validação, reprovação, descarte e permissões (`tasks/finish.rs`, `file_lock.rs`, `bridge.rs`):** `validate` exige ausência de commits não mesclados, fecha a aba, remove a worktree (diagnosticando processos que prendam a pasta no Windows) e a branch e marca `Merged`. `reject` reabre aba/worktree se preciso e volta a `Running` com o motivo. `discard` limpa aba, worktree e branch de qualquer estado não final. Na ponte, o agente de uma tarefa ativa só pode consultar tarefas e chamar `/done` da própria tarefa (demais rotas de tarefa → **403**).

---

## Memória em duas camadas (`learnings.rs`, `situation.rs`, `outline.rs`)

1. **Longo prazo — Aprendizados (`learnings.rs`):** arquivos `memory/aprendizados/*.md` com frontmatter `titulo`, `resumo` e `touches` (globs relativos à raiz; vazio = global), lidos da worktree da tarefa ou do repo principal. No início da tarefa, o agente recebe os globais e os que casam com caminhos citados no pedido por inteiro (até 6 000 caracteres) mais o índice dos demais (até 2 500); em devoluções/reprovações, recebe os que casam com arquivos novos citados no feedback (até 3 000).
2. **Curto prazo — Situação atual e histórico (`situation.rs`, `tasks/`):** entrega as outras tarefas ativas com os arquivos que tocam (commits da branch + sujos da worktree via `git --no-optional-locks status`) e os últimos 8 commits da base (até 1 500 caracteres). Devoluções e reprovações repetem os últimos 5 feedbacks, e a tarefa guarda até 300 mensagens (6 000 caracteres cada) trocadas pelo agente.
3. **Consulta sob demanda (`--learn` e `--outline`):** `ask-agent --learn [<arquivo>…]` (`POST /learn`, até 12 000 caracteres) devolve globais + índice + situação atual (sem argumentos) ou avisos `ATENÇÃO:` de choque com outras tarefas + aprendizados que casam (com arquivos). `ask-agent --outline <arquivo>…` (`POST /outline`, `outline.rs`, até 6 000 caracteres/arquivo e 16 000/chamada) extrai assinaturas e linhas (itens e membros de `impl`/`trait`/`mod` em Rust, títulos em Markdown) sobre o léxico de `highlight.rs`.

---

## Interface: Organização, Modo Documentos, Git e GitHub

- **Organização do centro (`crates/app/src/split.rs`, `ui/panes.rs`, `ui/tabs.rs`):** agrupa abas do projeto por tipo de CLI. Em `Split`, monta uma grade automática com proporções salvas; em `Highlight` (Destaque), põe o grupo da aba ativa em 2/3 à esquerda (trocar a aba ativa troca o destaque) e empilha os demais à direita; em `Tabs`, usa janela única. O PTY só é redimensionado quando o tamanho estabiliza e o mouse é solto (debounce de 150 ms no arrasto, 600 ms na troca de Destaque e 300 ms após saída do resize anterior, teto de 3 s).
- **Modo Documentos e editor (`crates/app/src/docs.rs`, `ui/docs.rs`, `ui/code.rs`, `ui/diff_view.rs`, `highlight.rs`):** alterna o centro (`Ctrl+Shift+D`) entre terminais e abas de documentos (`TaskDiff`, `CommitDiff`, `FileDiff`, `File`, `Markdown`). Com terminais ocultos, nenhum resize ou tecla chega a eles e fins de turno viram selo em "Agentes". Arquivos até 256 KB (linhas ≤ 4 000 bytes, UTF-8 sem mistura CRLF/LF) abrem no editor com cache de layout/realce e gravação atômica que preserva CRLF/LF e BOM; alterações externas detectadas pelo watcher com edição não salva não sobrescrevem o buffer e abrem a faixa de resolução. Arquivos até 1 MB e diffs usam pintura linha a linha com seleção própria (`ui/select.rs`) e busca (`Ctrl+F`); Markdown usa `egui_commonmark`.
- **Git local e GitHub (`crates/core/src/git.rs`, `github.rs`, `crates/app/src/git_ui.rs`, `github_ui.rs`):** o Git local roda o binário `git` sem shell, passando caminhos e mensagem de commit via stdin com ambiente não interativo (`GIT_OPTIONAL_LOCKS=0`, `GIT_TERMINAL_PROMPT=0`), enquanto o watcher observa `.git/` (`HEAD`, `index`, `refs/`). O painel GitHub lê o remoto `origin`, obtém o token só em memória (`git credential fill` ou `gh auth token`, descartado em 401), consulta `api.github.com` em threads (cache de 60 s) e baixa os 32 KiB finais de logs de jobs falhos sem reenviar `Authorization` após redirecionamento.

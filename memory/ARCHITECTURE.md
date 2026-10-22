# Arquitetura do CommandDeck Terminal

Visão geral e mapa dos documentos: [GERAL.md](GERAL.md).

Aplicação nativa em Rust. (Origem: reescrita da antiga versão Electron, com as mesmas funções e o mesmo visual.)

## Stack (decisões do Lead)

| Camada | Escolha | Por quê |
| :-- | :-- | :-- |
| Janela/eventos | **winit 0.30** (loop próprio, `ApplicationHandler`) | pedido do projeto; controle total do loop e de DnD |
| Render | **glutin + glow** com **egui** (`egui-winit` + `egui_glow`) | widgets prontos e estilização fina para reproduzir o CSS; OpenGL leve (roda até em Xvfb/llvmpipe para testes) |
| Terminal | **alacritty_terminal** (parser VT + grade) + **portable-pty** (ConPTY no Windows, PTY no Unix) | emulação madura; o render da grade é nosso (pintado no egui) |
| Ponte HTTP | **tiny_http** (thread por requisição, até 128 simultâneas) | API `/agents`, `/models`, `/team`, `/ask`, `/reply`, `/signal`, `/usage`, `/learn`, `/tasks`; token por processo de agente + validação de Host |
| Cliente HTTP | **ureq** (Ollama `/api/chat`, `/api/tags`, downloads) | bloqueante e simples, roda em threads |
| Arquivos | **notify** (watcher), **rfd** (diálogos), **open** (abrir no SO), **arboard** (clipboard texto/imagem) | |
| Áudio | **cpal** (microfone, feature `audio`) + whisper.cpp (binários em `whisper/`) | |

Sem Node/Electron em runtime: `ask-agent` e `ollama-agent` viram binários Rust ao lado do app.

## Crates

```
(raiz)
├── crates/core   commanddeck-core   backend sem GUI
├── crates/app    commanddeck        UI winit+egui
└── crates/cli    commanddeck-cli    bins ask-agent e ollama-agent
```

A UI fala com o backend apenas via `commanddeck_core::Core` (métodos ≈ canais IPC) e recebe
`CoreEvent` por um `crossbeam_channel::Receiver`, com um `Waker` que acorda o loop do winit
(`EventLoopProxy`). Operações bloqueantes (listar agentes, transcrição) rodam em threads.

## Ponto de extensão para RAG (futuro)

`core::context::ContextProvider` — hoje vazio. O motor Ollama e o executor headless
consultam os provedores registrados antes de enviar o prompt; plugar RAG local/remoto
não exige alterar o resto do sistema.

## Protocolo entre agentes (`ask-agent`)

Cada aba recebe no ambiente: `COMMANDDECK_PORT` (porta da ponte), `COMMANDDECK_TOKEN` (token de 128 bits **da
própria aba**), `COMMANDDECK_AGENT` (nome do agente da aba), `COMMANDDECK_DEPTH` (profundidade de chamadas) e a
pasta dos executáveis no início do `PATH`. Nenhuma outra `COMMANDDECK_*` do ambiente do app passa para os
agentes (exceto `COMMANDDECK_ASK_WAIT`, configuração do usuário).

**Identidade pelo token.** Cada processo de agente tem um token próprio: o da aba vale enquanto a instância
existir (`Core::tab_token`); o de uma execução headless, só durante ela (`Core::headless_token`). A ponte
descobre **quem** pergunta pelo token, e o campo `from` do corpo é ignorado. Assim uma aba não consegue se
passar por outra nem responder no lugar dela. O token mestre (`Core::bridge_token`) não vai para nenhum
agente: serve a testes e ferramentas de depuração, e com ele o `from` do corpo é aceito se for um id válido.

**Projetos.** Cada aba (instância) pertence a um projeto (`ProjectCfg`: uma pasta, com id `p` + 8 hex), e a
ponte só enxerga o projeto de quem chama: cada projeto pode ter o seu `claude`, e um `ask-agent claude@dev`
cria `claude@dev` no projeto de quem perguntou. O projeto vai **no token**: `Core::tab_token(projeto, id)` dá
um token por par projeto + instância (o mesmo vale para os tokens headless). Com o token mestre, o projeto
vem do campo `"project"` do corpo (id de um projeto existente; 400 se desconhecido) ou, sem ele, é o
projeto ativo (400 se não há nenhum). Por dentro, as chaves das perguntas pendentes, da atividade, das
filas por par e das sessões headless têm escopo: `instances::scoped(projeto, id)` = `"<projeto>#<id>"`
(`#` nunca aparece num id), então ciclos e profundidade nunca misturam projetos. O que agentes e pessoas
veem é sempre o id **sem** o projeto: o texto digitado, as mensagens de erro (lista de "Disponíveis"), os
`FeedEntry.from/to` e `PendingInfo { project, asker, target }` (pergunta pendente entre abas).
Os eventos levam o projeto: `Delivery.project`, `AgentSignal { project, id, busy }` e `FeedEntry.project`
(`None` = global, como os avisos do Whisper). As abas de agentes rodam na pasta do projeto, e o AGENTS.md
de um projeto que não é o ativo é escrito na primeira aba aberta nele.

| Rota | Descrição |
| :-- | :-- |
| `GET /agents` | `{"agents": [...], "models": {id: modelo}}` com os agentes instalados e as instâncias abertas no projeto de quem chama, e o modelo de LLM das instâncias que têm um |
| `GET /models`, `GET /models?agent=<id>` | `{"text"}`: o inventário de `ask-agent --models` (um bloco por agente instalado, ou só o de `agent`; ver "Inventário para os agentes" abaixo); 400 para `agent` fora de `[A-Za-z0-9:._/#@+-]{1,128}` e 404 para um agente que não está instalado |
| `GET /team` | `{"id","boss","crew","models"}`: a posição de quem chama na equipe (só token de aba); `models` traz o modelo de quem aparece na resposta |
| `POST /ask` | `{"to","message","depth","new","wait"[,"reply_to"][,"model"]}` (+ `"from"`, `"project"` só com o token mestre) → `{"answer"}`; 202 `{"pending","answer"}` quando a resposta vai chegar depois (ver abaixo); 400 para agente desconhecido, auto-pergunta, remetente inválido, profundidade `>= 2`, `model` fora de `[A-Za-z0-9:._/#@+-]{1,128}` ou agente que não escolhe modelo (`o agente X não aceita escolher modelo`); 404 para `reply_to` que não está pendente; 409 para ciclo de espera ou para `model` diferente do da instância já aberta (`claude@dev já está aberta com sonnet; use --new para abrir outra com opus`); 413 para corpo acima de 16 MB; 500 em erro/timeout (10 min) |
| `POST /learn` | `{"paths": [...]}` (até 50 caminhos; vazio = sem arquivos) → `{"text"}`: o que o projeto já aprendeu (`memory/aprendizados/`) sobre esses arquivos (`ask-agent --learn`); sem caminhos, os globais por inteiro e o índice dos demais; 400 se não acha o repositório do projeto (ver "Memória em duas camadas") |
| `POST /reply` | `{"to","message"[,"reply_to"]}`: conclui uma pergunta pendente (404 se não há); nunca cria pergunta |
| `POST /signal` | `{"state":"busy"\|"idle"}`: hook do agente avisando início/fim de turno (só tokens de aba) |
| `POST /usage` | `{"session":{"used","resets_at_ms"}\|null,"weekly":…}`: uso do plano do modelo da aba (`used` em 0..=1; só tokens de aba; `shell`/`ollama` recusados) |
| `GET /tasks`, `GET /tasks/<id>` | as tarefas do projeto de quem chama (`{"tasks": […]}`, sem brief, relatório nem histórico) ou uma tarefa completa (`{"task": {…}}`); com o token mestre, `?project=<id>` escolhe o projeto. Qualquer token pode ler |
| `POST /tasks` | `{"title","brief"[,"kind"][,"model"][,"base"][,"start"]}` → `{"task"}`: cria a tarefa (`kind` padrão `claude`; `start: true` já a inicia) e grava quem pediu como criador; 400 para título vazio, agente ou modelo inválidos, base que não é branch local ou projeto sem git/commit; se `start` falha, devolve o erro com a tarefa criada |
| `POST /tasks/<id>/start`, `/merge`, `/validate`, `/discard` | `{}` → `{"task"}`: inicia, mescla (a tarefa fica aguardando validação), valida (conclui e limpa) ou descarta (409 se o estado não permite, se o repositório principal não está pronto, se há conflito ou se a validação não pode limpar; 500 em erro do git) |
| `POST /tasks/<id>/done` | `{"report"}` → `{"task"}`: o agente da tarefa entrega o relatório (só o dono da tarefa, ou o token mestre) |
| `POST /tasks/<id>/return` | `{"feedback"}` → `{"task"}`: devolve a tarefa em revisão ao agente, com o feedback |
| `POST /tasks/<id>/reject` | `{"feedback"}` → `{"task"}`: reprova a tarefa aguardando validação: volta ao mesmo agente, na mesma worktree, com o motivo (400 para motivo vazio; 409 fora de `Validating` ou com as abas dos agentes desligadas) |

Toda requisição exige o cabeçalho `x-commanddeck-token` (comparado em tempo constante) e um `Host`
`127.0.0.1:<porta>` ou `localhost:<porta>` (sem `Host`, recusa). No máximo 128 requisições são atendidas ao
mesmo tempo (503 além disso; uma pergunta com resposta atrasada continua ocupando a vaga até a resposta chegar), e
os corpos sendo lidos ou atendidos somam no máximo 64 MB (503 além disso). Pedidos ao mesmo par `de->para` são serializados, e o histórico de cada par é
mantido entre chamadas: `--session-id`/`--resume` no Claude, `--conversation` no Antigravity, e memória em
processo no Ollama.

`ask-agent` sai com código 0 (resposta no stdout), 1 (erro do agente) ou 2 (uso incorreto ou fora do CommandDeck).
`ask-agent --signal` sai sempre com 0: um hook que falha não pode atrapalhar o agente.

**Uma janela por pasta de dados.** O core trava `dados/.commanddeck.lock` ao iniciar; uma segunda janela
na mesma pasta é recusada com uma mensagem (defina `COMMANDDECK_HOME` para abrir outra instância isolada).

### Instâncias (abas) e perguntas entre elas

Cada aba é uma **instância** com identidade própria, usada em `COMMANDDECK_AGENT` e como destino do
`ask-agent`: `<modelo>[@<cargo>[@<nome>]]`.

| Id | Significado |
| :-- | :-- |
| `claude` | a instância principal (sem cargo), a que o usuário abre pelo "+ Novo agente" |
| `claude@dev`, `claude@design` | instâncias com cargo, abertas por um agente (ou pelo usuário) |
| `claude@dev@ana` | quando já existe outra do mesmo modelo **e** cargo, a nova ganha um nome |
| `claude@main@bia` | segunda instância principal (sem cargo) aberta à mão |

A aba aparece como `Claude`, `Claude · Dev`, `Claude · Dev · Ana`. O id é gerado pelo core
(`crates/core/src/instances.rs`), único dentro do projeto, e salvo em `ProjectCfg.open_tabs` do projeto
(`Config.projectList`), então as abas voltam com o mesmo nome.
**Equipe (chefe e subordinados).** O core guarda o chefe de cada instância em `ProjectCfg.tab_parents`
(lógica pura em `crates/core/src/team.rs`). Quando um agente abre uma instância nova pelo `ask-agent`, vira
o chefe dela; o usuário reorganiza arrastando agentes na barra lateral (`Core::set_boss`, que recusa ciclos).
Fechar uma instância passa a equipe dela ao chefe dela; renomear leva a equipe junto. Os agentes sabem da
equipe sem turno extra: as instruções mandam consultar `ask-agent --team` (`GET /team`, pelo token) e quem
mudou de posição recebe o aviso anexado à **próxima** mensagem digitada na aba dele (a interface chama
`Core::take_team_notice` na hora de digitar, então uma entrega descartada não gasta o aviso; consultar o `--team`
já dá o aviso por lido). O chefe de quem fecha também é avisado, mesmo que a equipe dela fosse vazia. `ask-agent --boss "msg"` pergunta ao chefe. A barra lateral desenha a árvore
(`ui/projects.rs::agent_tree`). A equipe orienta, não restringe: qualquer agente continua podendo chamar qualquer outro.
O cargo também vai em `COMMANDDECK_ROLE` e no prompt de sistema.

**Modelo de LLM da instância** (`crates/core/src/models.rs`). O modelo (`sonnet`, `opencode/glm-5.3-flash-free`…) é uma
propriedade da instância, **não** parte do id (que continua `modelo[@cargo[@nome]]`): fica em
`ProjectCfg.tab_models` (id → modelo), no mesmo molde de `tab_parents` (renomear leva o modelo, fechar o esquece).
Ao abrir a instância vale, na ordem: o pedido (`ask-agent --model` → `"model"` do `/ask`; o menu "+ Novo agente"),
o gravado em `tab_models` (reabrir o app mantém), o padrão de `Config.role_models` (chave `modelo@cargo` ou só
`modelo`; `modelo@cargo` vence) e, por fim, o do próprio CLI (nenhum argumento). O resultado é gravado em
`tab_models` por `Core::assign_model` (idempotente; chamado pela ponte ao criar a instância e por `spawn_spec_for`).
Só aceitam modelo `claude`, `antigravity`, `codex` e `opencode` (os modelos do Ollama já vão no tipo); o nome é
validado (`valid_model`) antes de chegar a qualquer linha de comando, e os shims `.cmd` ainda passam por `cmd_safe_arg`.
- **Nas abas** (`spawn_spec_for`): `claude`, `agy` e `codex` recebem `--model <m>`. O TUI do **OpenCode** não tem
  `--model` (só `run` e `mini` têm `-m`): a aba abre `opencode --standalone` com `OPENCODE_CONFIG_CONTENT={"model":"<m>"}`
  no ambiente. A configuração por variável só é lida por quem sobe o servidor, e o TUI normal usa o serviço em segundo
  plano compartilhado, que ignora o nosso ambiente; por isso o servidor próprio (`--standalone`). A variável vale por
  cima da configuração do usuário e do projeto (verificado no OpenCode 2.0.32: a linha de status do TUI mostra o modelo
  pedido). Sem modelo, o OpenCode abre como sempre. A variante (`modelo#alta`) só existe no `run -m`: no TUI vai só o modelo.
- **No headless** (`headless.rs`): `--model` no `claude`, `agy` e `codex exec`; `opencode run -m <m>`.
- **Conflito explícito:** `/ask` com `model` para uma instância já aberta com **outro** modelo (ou com o padrão do CLI)
  devolve 409 com a saída pelo `--new`; o mesmo modelo, ou nenhum pedido, entrega normalmente; `--new` + `--model` abre
  outra instância com aquele modelo.
- **Lista de modelos** (`Core::known_models`, para o menu "+ Novo agente" e o formulário de nova tarefa):
  `claude` = os apelidos `opus, sonnet, haiku, fable`; `antigravity` = `agy models` (primeira coluna);
  `opencode` = `opencode models`. As duas últimas rodam numa thread (a interface chama por
  `Deck::request_models`), com limite de 15 s e cache de 10 min (60 s se falhou); sem lista o menu mostra
  só "Padrão". Como o `opencode models` mistura todos os provedores conectados, os dois menus usam
  `models::by_provider` e pedem o **provedor antes do modelo** (`opencode`, `opencode-go`, `ollama`…, o
  prefixo antes da `/`); os modelos `-free` levam a marca "grátis" (`models::is_free`) e vêm primeiro em
  cada lista. Com um provedor só, `models::sole_provider` pula o passo do provedor e mostra os modelos
  dele direto.
- **Inventário para os agentes** (`ask-agent --models [agente]`, `GET /models[?agent=<id>]`, `Core::models_inventory`):
  imprime, por agente instalado, o nome como vale no `--agent` e os modelos como o `--model` aceita, com os `-free`
  primeiro e marcados "(grátis)"; o OpenCode sai agrupado por provedor, uma linha cada (`models::by_provider`); os
  modelos do Ollama saem juntos como `ollama:<modelo>`; o Codex não tem lista e os CLIs sem escolha de modelo
  (aider…) saem como tal. Agente não instalado não aparece (e `--models <agente>` dele dá 404); uma listagem que
  falha vira uma linha ("não consegui listar") sem derrubar o resto. Usa o cache de `known_models` e consulta os
  CLIs em paralelo, então demora no máximo o `LIST_TIMEOUT`. A formatação é a função pura `models::format_inventory`.
  As instruções da ponte e as regras da tarefa (`start_message`) só apontam o comando, sem colar a lista no prompt.
- **Onde aparece:** `ask-agent --list` e `--team` (`claude@dev (sonnet)`, pelos campos `models` de `/agents` e `/team`), o
  cabeçalho da aba, a barra lateral e o menu de contexto da aba (só leitura: trocar o modelo de uma aba aberta exige
  reiniciar o agente e não existe ainda).

`ask-agent <id> "msg"` (e `--new` para forçar outra instância) funciona assim:

1. O core resolve o destino: o id exato, a instância principal do modelo ou a única que existe;
   se nenhuma existe, **cria** (`claude@dev` abre uma aba nova). Destino ambíguo ou desconhecido dá erro.
2. A pergunta vira `CoreEvent::Deliver`. A interface abre a aba (em segundo plano, sem roubar o foco),
   espera o terminal ficar parado e **digita a mensagem nele**, uma por vez por aba (fila).
3. A mensagem digitada ensina a responder: `ask-agent --reply <n> "resposta"`, com o número da pergunta.
   Só a instância que recebeu a pergunta consegue respondê-la (`crates/core/src/exchange.rs`). A forma
   antiga, `ask-agent <quem-perguntou> "resposta"` (o nome passa pelos apelidos, como `agy`), continua valendo
   enquanto o `ask-agent` de quem perguntou ainda espera: uma pergunta nova para ele fecharia um ciclo de espera,
   então só pode ser resposta. Depois do 202 (item 4) quem perguntou está livre, e a forma antiga vira uma
   pergunta nova; a resposta vai pelo número. Quem perguntou e o destino são lidos pelo número na hora de usar,
   então renomear uma aba no meio da pergunta não perde a resposta. Nada de raspar a tela.
4. **Resposta síncrona ou atrasada.** O `ask-agent` de quem perguntou espera até `COMMANDDECK_ASK_WAIT`
   segundos (padrão 90, abaixo do timeout padrão do Bash do Claude Code). Se a resposta não chegou, ele
   termina com HTTP 202 e um aviso, e a resposta, quando chegar (até 10 minutos), é **digitada no terminal
   de quem perguntou** como uma mensagem nova (`Delivery { answer: true }`). Isso só vale para abas de
   agentes: num terminal comum (`shell`) o texto seria executado como comando, então ali o `ask-agent`
   espera até o fim, assim como nas execuções headless e no `ollama-agent` (`COMMANDDECK_ASK_WAIT=0`).
5. O `ollama-agent` devolve a resposta sozinho (`POST /reply` com o número lido da instrução, que só conclui
   perguntas pendentes e nunca cria uma nova), porque modelos pequenos esquecem de rodar o comando.
6. Fechar a aba ou o processo terminar libera quem esperava, com erro; fechar a aba de **quem perguntou**
   descarta as perguntas dela. Ciclos de espera (A espera B, B espera C, C espera A) são recusados na hora
   (HTTP 409). A profundidade também vale entre abas: como ela não viaja pelo ambiente (a pergunta é
   digitada), é medida pela cadeia de perguntas pendentes (`Exchange::chain_depth`).

**Tarefas** (`crates/core/src/tasks.rs`; ponte em `bridge.rs`, `ask-agent --task`). A unidade de trabalho do AgentDeck: um
pedido (brief), um agente com modelo, uma worktree + branch próprias, um relatório, um diff e um ciclo de vida.

```
Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged ("concluída")
  │                 ▲  ▲  ▲           │                  │ └─discard──▶ Discarded (de qualquer estado não final)
  │                 │  │  └──return───┘                  │
  │                 │  └────────────reject───────────────┘
  └─ erro ao iniciar ▶ Failed ──start de novo──▶ Running
```

Mesclar quer dizer "o diff parecia certo"; o teste de verdade o usuário faz depois, no app compilado. Por isso mesclar **não**
encerra o agente: a tarefa espera em `Validating` ("aguardando validação") com a aba, a worktree e a branch vivas. O valor
`Merged` continua no enum e no JSON (tarefas antigas), mas para o usuário é "concluída".

- **Dados.** `Task { id (t1, t2…: contador por projeto, nunca reaproveitado), title, brief, status, base, branch
  (tarefa/<id>-<slug>), worktree, kind, model, instance (<modelo>@<id>, ex.: claude@t3), creator, created_ms, started_ms,
  finished_ms, report, diff_stat, error, events (últimos 200) }`. Ficam em `dados/tarefas/<projeto>.json`
  (`{"version","next","tasks"}`), gravados por arquivo temporário + `rename`, uma gravação por vez, **fora do repositório do
  usuário**; um arquivo ilegível é copiado para `….json.ilegivel-<ms>` antes de recomeçar. Remover o projeto não apaga o arquivo.
- **Worktrees.** Em `<raiz do repositório>/.agentdeck/worktrees/<id>` (dentro do repositório de propósito: agentes confinados
  ao workspace, como o Antigravity, conseguem chegar lá). Na primeira tarefa iniciada, `/.agentdeck/` entra uma vez em
  `<git comum>/info/exclude` (nunca no `.gitignore`); o explorador e o watcher ignoram a pasta. O core só apaga pastas que
  estejam mesmo dentro de `.agentdeck/worktrees`.
- **Iniciar** (`Core::task_start`, só com as abas dos agentes ligadas: tarefas não rodam em headless). `git worktree add -b
  <branch> <pasta> <base>`; registra a instância `<modelo>@<id>` com o modelo da tarefa (`models.rs`); quem criou a tarefa,
  se for um agente aberto, vira o chefe da instância (`ask-agent --boss` chega nele); `agents::spawn_spec_for` abre o terminal
  **na worktree** (`Core::task_cwd`: tarefa em andamento, em revisão ou aguardando validação); o brief mais as regras da tarefa (trabalhar só na
  worktree, commits pequenos, nada de merge nem push, o relatório em um arquivo com `ask-agent --task done <id> --file <arq>`, dúvidas ao chefe) vão
  para a aba por `CoreEvent::Deliver { request: 0 }`, o mesmo caminho das mensagens entre agentes (fila e regras de `delivery.rs`,
  sem esperar resposta). **Aba que precisa abrir** (nova, ou processo encerrado): digitar o texto num CLI que ainda sobe o perde (visto no Antigravity, que mostra um aviso na abertura). Então o texto completo (brief, devolução ou reprovação) vai para `dados/tarefas/<projeto>-<id>-brief.md` (ou `-feedback.md`) e `Delivery::boot` leva só uma linha curta em ASCII (`tasks::boot_line`: "Tarefa t11: … Leia o arquivo <caminho> e comece agora…"); a interface a guarda com `Core::set_boot_prompt` e `agents::spawn_spec_for` a põe na linha de comando, uma vez: Claude `claude "<linha>"` (posicional, **antes** das opções, porque `--add-dir`/`--allowedTools` aceitam vários valores), Antigravity `agy -i "<linha>"`, OpenCode `opencode --prompt "<linha>"` (`models::boot_prompt_flag`). Os demais CLIs, a aba já viva e o caminho com não-ASCII seguem digitando. Erro ao iniciar = `Failed` com a mensagem; `start` de novo tenta outra vez.
- **Relatório** (`task_done`): só `Running`; guarda o relatório e `git diff --stat base...branch`, vai a `Review` e avisa se
  a worktree tem trabalho sem commit. **Devolver** (`task_return`): `Review → Running`, entrega o feedback (a aba é reaberta
  se foi fechada).
- **Mesclar** (`task_merge`): só `Review` (ou `Failed` com relatório, para repetir). No repositório principal recusa, sem mudar
  nada e com a explicação, se ele não está na base, está no meio de outra operação, tem alterações sem commit em arquivos
  rastreados (o `AGENTS.md`, que o app reescreve, não conta; o git ainda protege o arquivo se a mesclagem o tocar), se a worktree da
  tarefa tem trabalho sem commit ou se a branch não tem commits novos. Depois `git merge --no-ff -m "Merge tarefa <id>: <título>"`.
  Conflito: `git merge --abort`, a tarefa continua em `Review` e a mensagem lista os arquivos. Sucesso: **não** fecha a aba nem
  remove worktree/branch; alinha a branch da tarefa com a base já mesclada (na worktree, `git merge --ff-only <base>`: a branch é
  ancestral do commit de mesclagem) e vai a `Validating` ("mesclada em <base> (<hash>); aguardando validação"). Se o alinhamento
  falhar, só registra um aviso (o merge já aconteceu); a reprovação tenta de novo. Erro inesperado do git = `Failed`.
- **Validar** (`task_validate`): só `Validating`. Recusa, sem mudar nada, se a worktree tem trabalho sem commit ou se a branch tem
  commits que a base não tem (o agente seguiu trabalhando: reprove ou descarte). Senão fecha a aba (`CoreEvent::CloseInstance`),
  `git worktree remove --force` (com tentativas, porque no Windows o terminal ainda segura a pasta logo depois de a aba fechar),
  `git branch -D` e vai a `Merged`. Se a pasta não sai, a tarefa continua em `Validating` com a explicação (dá para tentar de novo).
- **Reprovar** (`task_reject(feedback)`): só `Validating`, com o motivo e as abas dos agentes ligadas. Recria a pasta da worktree se
  foi apagada à mão (a partir da branch, ou da base se a branch também sumiu), alinha a branch com a base, volta a `Running` na
  mesma worktree e entrega ao mesmo agente `tasks::reject_message` (o trabalho anterior já foi mesclado em <base>, o usuário testou e
  reprovou pelo motivo X, corrija na mesma branch e avise com `--task done` de novo); a aba é reaberta se foi fechada. O ciclo
  segue: done → revisão → mesclar → aguardando validação.
- **Descartar** (`task_discard`): qualquer estado não final (inclusive `Validating`: o que já foi mesclado continua na base, descartar
  não desfaz o merge); fecha a aba, remove a worktree à força (mesmo que a pasta já tenha sumido) e a branch (`-D`),
  `git worktree prune`, `Discarded`. Sem repositório disponível, só o registro muda.
- **Permissões na ponte** (`bridge::task_permission`). O **agente de uma tarefa** (a instância `<modelo>@<id>` de uma tarefa
  que ainda não terminou) só consulta (`GET`) e entrega o relatório da **própria** tarefa; criar, iniciar, devolver, mesclar,
  validar, reprovar e descartar dão 403, e entregar o relatório de outra tarefa também. Qualquer outro agente faz o resto, menos entregar relatório
  por outro. O token mestre (nenhum agente o recebe) pode tudo. O projeto vem do token.
- **Erros** (`TaskError` → HTTP): `Invalid` 400, `NotFound` 404, `Conflict` 409, `Failed` 500.
- **Para a interface** (`Core`, tudo bloqueante: chame numa thread): `tasks(projeto)`, `task(projeto, id)`,
  `task_of_instance(projeto, instância)`, `task_create(projeto, TaskSpec)`, `task_start`, `task_done`, `task_return`,
  `task_merge`, `task_validate`, `task_reject`, `task_discard`, e `task_diff(projeto, id)` (só leitura: o `git diff base...branch` no mesmo `GitDiff` da aba Git).
  Eventos: `CoreEvent::TasksChanged { project }` a cada gravação (também as vindas da ponte) e
  `CoreEvent::CloseInstance { project, id }` quando o core encerra a instância (a interface fecha a aba). Reabrir a aba de uma
  tarefa em andamento, em revisão ou aguardando validação é abrir a instância dela (`restore_instance` + aba): o terminal já abre na worktree.
- **Interface** (`crates/app/src/tasks_ui.rs`, `ui/tasks.rs`): a aba Tarefas da barra direita. Cada chamada acima roda numa thread
  (`Deck::spawn_bg`) e volta como `UiMsg::Tasks`; a lista é relida a cada `TasksChanged` do projeto ativo. Grupos da lista: Em
  revisão, **Aguardando validação**, Em andamento, Rascunho, Falhou, Concluídas, Canceladas; o selo do ícone conta revisão + validação.
  Botões por estado (`actions_for`): em revisão Mesclar/Devolver; aguardando validação **Validar** (sem confirmação) e **Reprovar**
  (reaproveita o formulário de feedback do Devolver). Descartar em `Validating` avisa que o que foi mesclado continua na base.

**Memória em duas camadas** (fase 1 do passo 3 do roadmap; `learnings.rs`, `situation.rs` e `tasks.rs`). O app entrega ao
agente de uma tarefa o que o projeto já aprendeu e o que está acontecendo agora, no `start_message`:
- **Longo prazo: aprendizados.** Um `.md` por aprendizado em `memory/aprendizados/` (versionado no git; formato no `README.md` da
  pasta): frontmatter `titulo`, `resumo` e `touches` (globs relativos à raiz do repositório; `*` não cruza pasta, `**` cruza, padrão sem
  `/` vale para o nome do arquivo em qualquer pasta; vazio = projeto todo). `learnings::load` lê a pasta (arquivo inválido, grande demais
  ou ilegível é ignorado com `log::warn!`; nunca derruba a tarefa), `glob_match`/`matching` casam os `touches` com um conjunto de
  caminhos (os específicos antes dos globais), `paths_in_text` extrai do brief só os arquivos que existem no repositório (sem `..` nem
  caminho absoluto) e `render` monta o texto: os que valem por inteiro até `START_BUDGET` (6 000 caracteres); o que passar do orçamento
  e os demais viram uma linha de índice (`arquivo — resumo`), com teto de 2 500 caracteres e "e mais N". Os arquivos são lidos da
  **worktree da tarefa** (já tem a base), então um aprendizado que o agente cria na branch dele é visível a ele mesmo.
- **Sob demanda: `ask-agent --learn [<arquivo>…]`** (`POST /learn` na ponte → `Core::learn`): imprime os que valem para esses
  arquivos (até `LEARN_BUDGET`, 12 000 caracteres); sem arquivos, os globais por inteiro e o índice de todos os outros
  (`learnings::learn_report`). As instruções da ponte (`agents.rs`) mandam rodá-lo antes de mexer em arquivos. Caminhos absolutos dentro da worktree ou do repositório viram caminhos do
  repositório (`learnings::relativize`). Lê a pasta da worktree da tarefa de quem pergunta ou, sem tarefa, do repositório do projeto.
- **Escrita.** A regra da tarefa manda criar um arquivo em `memory/aprendizados/` na própria branch para algo não óbvio e citá-lo no
  relatório: o aprendizado passa pela mesma revisão e mescla que o código.
- **Curto prazo: "Situação atual"** (`situation::render`, orçamento de 1 500 caracteres): as **outras tarefas em andamento** do projeto
  (em andamento, em revisão e aguardando validação; id, título, instância, estado e os arquivos dela: os da branch via
  `git diff --name-only base...branch` **mais** os ainda sem commit na worktree, lidos por `worktree_dirty_unlocked`
  (`git --no-optional-locks status`, sem tomar o `index.lock` do outro agente), sem repetição, em ordem alfabética e sem o `AGENTS.md`;
  em `Validating` a lista é vazia; com um aviso quando algum deles é citado no pedido desta tarefa) e os **últimos 8 commits da
  base** (`git log --oneline -8`). O que não couber é cortado do fim (primeiro os commits). `Core::task_memory` junta as duas
  camadas (`tasks::memory_block`) e entra entre o pedido e as regras; qualquer falha deixa só aquela parte de fora.
- **Histórico de feedbacks.** `Task::feedbacks` (`Vec<Feedback { at_ms, origin, text }>`, `serde(default)`: tarefas gravadas antes
  continuam abrindo; no máximo 20) guarda cada devolução da revisão e cada reprovação da validação. `return_message` e
  `reject_message` repetem os anteriores (os últimos 5, cada um cortado em 700 caracteres) antes das regras, para o agente não
  repetir o que já lhe foi pedido; o feedback que está sendo entregue não entra na lista.
- **Aprendizados na devolução e na reprovação.** `Core::feedback_learnings` pega os arquivos que o feedback novo cita
  (`paths_in_text` na worktree), tira os que já apareciam no pedido (`brief`, ou o título) ou nos feedbacks anteriores e entrega
  só os aprendizados **específicos** que casam (`learnings::render_specific`, até `FEEDBACK_BUDGET`, 3 000 caracteres; sem índice
  e sem globais, que o agente já recebeu no início). O texto pronto entra em `return_message`/`reject_message` (que continuam
  puras, sem ler disco) como "O que o projeto já aprendeu sobre os arquivos citados", depois do histórico; sem nada novo, a
  mensagem fica como antes.
- **Fora desta fase** (ver `pendencias.md`, seção 2): resumo de API por arquivo (estilo skeleton), RAG/embeddings, injeção nas abas que
  não são de tarefa e o `ContextProvider` geral.

**Git** (`crates/core/src/git.rs`, aba Git da interface). Usa o `git` instalado (`Core::git_*`, todos
bloqueantes; a UI chama numa thread):
- sempre `Command::new(git).args([...])`, nunca um shell; caminhos e a mensagem de commit vão pela entrada padrão
  (`--pathspec-from-file=- --pathspec-file-nul`, `commit -F -`), e todo caminho passa por `safe_rel`;
- ambiente que não trava: `GIT_TERMINAL_PROMPT=0`, ssh em `BatchMode` nas operações de rede (a menos que o
  usuário tenha ssh próprio configurado), `GIT_OPTIONAL_LOCKS=0` (o status não regrava o índice),
  `GIT_LITERAL_PATHSPECS=1`, `LC_ALL=C` (os erros são reconhecidos em `classify_error`);
- limites de tempo (status/diff 5 s, commit 120 s, rede 60 s) e de saída (diff de até 1 MiB / 10 000 linhas,
  5000 entradas de status); uma operação de escrita por vez (`index.lock`);
- o watcher do workspace separa as mudanças de estado do repositório (`.git/HEAD`, `index`, `refs/…`, sem os
  `*.lock`) e emite `CoreEvent::GitChanged`; a interface relê o status 400 ms depois. Se o repositório fica
  acima da pasta de trabalho (o `.git` não é vigiado), a aba relê a cada 10 s enquanto está aberta;
- pull só por avanço direto (`--ff-only`); push sem upstream publica no único remoto (`push -u`).

**GitHub** (`crates/core/src/github.rs`, aba GitHub da interface). PRs abertos (com o estado dos checks), issues
abertas (sem os PRs) e execuções do Actions do repositório do remoto `origin` (`https://`, `git@github.com:` e
`ssh://`; outro host = a aba explica e não faz nada):
- API REST (`ureq`, só `https://api.github.com`) em threads de segundo plano; o resultado volta por
  `CoreEvent::Github` (`GithubPayload::Overview`, `PrDetail`, `RunJobs`, `JobLog`), com cache em memória de 60 s
  por repositório/PR/execução/job e atualização manual (`force`) pelo botão da aba;
- **sem guardar token**: o token vem de `git credential fill` (`protocol=https`, `host=github.com`, com
  `GCM_INTERACTIVE=never` e `GIT_TERMINAL_PROMPT=0`: nunca abre janela nem trava) ou, se o `gh` existir, de
  `gh auth token`; vive só na memória (`SecretToken`, cujo `Debug` não o mostra) e nunca vai para log, config ou
  mensagem de erro. Sem credencial, repositório público funciona (60 consultas/h; o log dos jobs exige login). Um
  token recusado (401) é esquecido e a leitura tenta de novo sem ele;
- o fim do log de um job que falhou: no máximo 200 linhas / 32 KiB (lê no máximo 8 MiB do fluxo); o redirecionamento
  do GitHub para o armazenamento do log é seguido sem o cabeçalho `Authorization`;
- erros 401/403/404/limite/rede viram `GithubError` com mensagem em português que diz o que fazer;
- "Abrir no navegador" só aceita `https://github.com/…` (`Core::open_github_url`).

**Janela e barra de título.** A janela é criada sem decoração (`with_decorations(false)`, com a sombra de volta
no Windows por `with_undecorated_shadow`); `ui/titlebar.rs` desenha a barra e manda ao winit os comandos do egui
(`ViewportCommand::StartDrag`, `Minimized`, `Maximized`, `BeginResize` nas faixas de 5 px das bordas), que o
`EguiGlow::run` já executa. O "×" liga `Deck::quit_requested`, e o `App` encerra como no `CloseRequested`.

**Uso dos planos** (`crates/core/src/usage.rs`, faixa em `ui/usage.rs`). `Core::usage_snapshot()` devolve o último
uso de cada modelo (`AgentUsage`: sessão e semana em 0..=1, quando renovam); mudanças emitem `CoreEvent::UsageChanged`.
- **Claude**: os hooks gravam `statusLine: ask-agent --usage`. O Claude Code passa `rate_limits.five_hour/seven_day`
  no stdin; o `ask-agent` manda à ponte (`POST /usage`) e imprime "sessão 34% · semana 12%" (ou repassa o stdin à
  statusline do próprio usuário, se houver, e mostra a dela).
- **Codex** (`usage/codex.rs`): a thread `cd-usage` relê a cada 60 s o fim do `rollout-*.jsonl` mais novo em
  `~/.codex/sessions` (`rate_limits.primary/secondary`), só com aba do Codex aberta.
- **Antigravity** (`usage/antigravity.rs`): a cada 3 min, com aba aberta, `v1internal:retrieveUserQuotaSummary`
  (janelas `5h` e `weekly`; alternativa: `fetchAvailableModels`) com o token do CLI (Gerenciador de Credenciais do
  Windows `gemini:antigravity` ou `~/.gemini/oauth_creds.json`) enquanto ele vale; quem o renova é o próprio CLI. API interna: qualquer falha
  vira "sem dados" (anel tracejado). O token nunca vai para log, Feed ou interface.
- **Ollama**: local, sem limite.

**Tela dividida** (`crates/app/src/split.rs` + `ui/panes.rs`). As abas continuam numa lista só
(`Deck::tabs`); o grupo de cada uma é `split::group_key(kind)` (todo `ollama:*` vira `ollama`). A cada quadro,
`Deck::plan_panes` calcula a grade (`grid_shape` → `layout` com as proporções salvas, que voltam a iguais quando
o formato muda), distribui os grupos (`assign`: os que não cabem dividem a última janela) e escolhe a aba
visível de cada janela (a de `shown` mais alto). `Deck::active` continua sendo a aba que recebe o teclado: é a
visível da janela focada, por isso o resto do código (teclado, IME, cores) não mudou. O cabeçalho de cada janela leva, além das
abas (`ui/tabs.rs`), a alça de arrastar, o microfone, o anexo e o "+" (`ui/panes.rs`): o microfone e o anexo agem na aba visível da
janela (`PanePlan::shown`, em `deck.rs`), e a aba guarda só o indicador de gravação/transcrição (`dictation.rs`); arquivos soltos na janela vão para a
aba visível da janela sob o ponteiro ou para o formulário "Nova tarefa" (`Deck::resolve_drop_zone`). Quando as abas não cabem, a faixa
rola: o lado com aba escondida ganha um degradê e uma seta ‹ / › e o fim da faixa, o menu ▾ com todas as abas (contas puras em
`ui/tabs.rs`: `hidden_tabs`, `scroll_to_show`). Cada aba
visível é redimensionada para a sua janela (`fit_tab`, com o debounce de 60 ms); as escondidas mantêm o tamanho
até aparecerem. O retrato das janelas do último quadro (`Deck::panes`) serve aos atalhos (Ctrl+Tab dentro da
janela, Ctrl+Alt+setas) e ao tamanho inicial das abas novas. Ordem, proporções e liga/desliga ficam em
`Config` (`paneOrder`, `splitRows`, `splitCols`, `splitEnabled`).

**Perguntas com resposta atrasada.** Uma pergunta com resposta atrasada (`detached`: o `ask-agent` já voltou
com 202) não entra na detecção de ciclos (quem perguntou não está bloqueado), mas continua contando na
profundidade. As perguntas pendentes entre abas viram `PendingInfo` (id, quem perguntou, a quem, profundidade,
desde quando e se está `detached`; ver `exchange::snapshot`, usado por `remove_project`).

**Quando digitar** (`crates/core/src/delivery.rs`, regra pura com testes; a interface só consulta):

- nunca logo depois de uma tecla do usuário na aba (2 s), nem com texto começado e não enviado (20 s);
- com **hooks** (Claude: `UserPromptSubmit`/`Stop` via `--settings dados/hooks/claude-settings.json`;
  Codex: `notify`), a aba só fica livre depois do aviso de fim de turno, mais 300 ms de saída parada;
- sem hooks, quando o terminal fica parado por 1,5 s; se nunca sossegar, depois de 30 s **na fila**
  (antes contava desde que a aba abriu, o que fazia digitar no meio de uma resposta);
- a colagem vai entre `ESC[200~ … ESC[201~` sem ESC nem caracteres de controle no texto (uma mensagem
  não consegue fechar a colagem e virar teclas), e o Enter sai da thread da interface, na ordem certa em
  relação ao que o usuário digitar nesse meio-tempo.

**Windows e shims `.cmd`.** Um agente instalado como shim do npm/pip roda como `cmd.exe /d /c <nome>`, e o
cmd interpreta `& | < > ^ % "` e quebras de linha. No headless o shim é chamado direto (a std do Rust aplica
o escape de arquivos em lote) e o prompt vai por arquivo em `dados/prompts`, nunca na linha de comando; nas
abas, os argumentos (gerados pelo app) passam por `cmd_safe_arg`, que os escapa para as duas passagens do cmd
(`cmd.exe /d /e:on /v:off /c` e o `%*` do shim) em vez de apagar caracteres: um perfil `C:\Users\Ana&Bia` funciona.
No Windows, os processos headless entram num Job Object com `KILL_ON_JOB_CLOSE` e morrem junto com o app, mesmo se ele cair.

Tudo fica visível nas abas (raciocínio, ferramentas, aprovações) e o usuário pode intervir digitando.
O Claude abre com `--allowedTools "Bash(ask-agent:*)"`, para responder sem pedir aprovação.

O **modo headless** (`claude -p`, `agy -p`) continua quando a
interface não ligou a entrega em abas ou o usuário desligou "Abrir abas dos agentes" nas Configurações.
Limites: no máximo 8 abas abertas automaticamente.

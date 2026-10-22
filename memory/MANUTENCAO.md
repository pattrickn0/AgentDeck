# Guia de manutenção

Este guia é para quem vai manter o CommandDeck, sozinho ou junto com agentes de IA. Ele não pressupõe experiência com Rust.
Visão geral e mapa dos documentos: [GERAL.md](GERAL.md).

## 1. Como o Rust funciona, na prática

- **Rust é compilado.** O código-fonte (`.rs`) vira um executável nativo (`.exe`). O usuário final não precisa de Rust, Node ou nenhum runtime: basta o `.exe`.
- **`cargo` é a ferramenta de tudo.** Ele baixa dependências, compila, roda testes e formata o código. Equivale a `npm` + `node` juntos.
- **Crate = pacote.** Este repositório é um *workspace*: um `Cargo.toml` na raiz que agrupa 3 crates em `crates/`.
- **Dependências** ficam no `Cargo.toml`, como no `package.json`. As versões exatas ficam travadas no `Cargo.lock`, como no `package-lock.json`. Ambos vão para o git.
- **O compilador é o seu maior aliado.** O Rust recusa compilar código com uso inválido de memória, tipos errados e várias condições de corrida entre threads. Se compilou, boa parte dos bugs já foi eliminada. As mensagens de erro são longas, mas costumam dizer exatamente o que fazer: leia a sugestão `help:`.
- **`Result` em vez de exceções.** Funções que podem falhar retornam `Result<Valor, Erro>`. O operador `?` repassa o erro para quem chamou. `unwrap()` derruba o programa se der erro, então evite em código de produção.
- **Posse (*ownership*).** Cada valor tem um dono. Para compartilhar entre threads usamos `Arc` (contador de referências) e `Mutex`/`RwLock` (trava). É por isso que o `Core` é um `Arc` por dentro e pode ser clonado à vontade.

### Comandos do dia a dia

```bash
cargo run -p commanddeck            # compila e abre o app (debug)
cargo build --workspace --release   # gera os .exe otimizados em target/release/
cargo test --workspace              # roda os testes
cargo clippy --workspace --all-targets -- -D warnings   # "linter": aponta código suspeito
cargo fmt --all                     # formata o código (estilo em rustfmt.toml; a CI confere com --check)
cargo update                        # atualiza dependências dentro das versões permitidas
```

Versão mínima do Rust: **1.89** (`rust-version` no `Cargo.toml`). A primeira compilação demora alguns minutos, porque compila todas as dependências. As seguintes são incrementais e rápidas. A pasta `target/` pode ser apagada a qualquer momento: ela é só cache de build.

## 2. Mapa do código

```
Cargo.toml                 workspace + versões das dependências compartilhadas
crates/
├── core/   (commanddeck-core)   "backend": nada de interface gráfica
│   ├── lib.rs        Core: a API que a interface usa (como os canais IPC)
│   ├── events.rs     CoreEvent: o que o backend avisa à interface
│   ├── agents.rs     lista de agentes, onde achar cada CLI, comando de cada aba, instruções da ponte
│   ├── bridge.rs     servidor HTTP local usado pelo ask-agent (/agents, /models, /team, /ask, /reply, /signal, /usage, /learn, /tasks)
│   ├── exchange.rs   perguntas entre abas esperando resposta (ciclos, profundidade, resposta por número)
│   ├── instances.rs  nomes das instâncias (modelo@cargo@nome) e resolução do destino
│   ├── team.rs       equipe: quem é chefe de quem (funções puras)
│   ├── activity.rs   última atividade de cada aba (o que a entrega nas abas consulta)
│   ├── models.rs     modelo de LLM de cada instância: resolução (pedido > gravado > role_models), validação, argumentos por CLI, lista de modelos e o inventário do `ask-agent --models`
│   ├── tasks.rs      mesa de tarefas: Task, ciclo de vida (criar/iniciar/relatório/devolver/mesclar/validar/reprovar/descartar), worktrees, mensagens ao agente, armazenamento em dados/tarefas
│   ├── learnings.rs  memória de longo prazo: aprendizados de memory/aprendizados/ (frontmatter, globs, caminhos do brief, entrega com orçamento, `ask-agent --learn`)
│   ├── situation.rs  memória de curto prazo: "Situação atual" do início da tarefa (outras tarefas, arquivos delas e commits da base)
│   ├── delivery.rs   quando digitar uma mensagem no terminal de uma aba (regra pura, com testes)
│   ├── headless.rs   roda um agente em modo -p (sem terminal) e captura a resposta
│   ├── ollama.rs     agente Ollama: ferramentas, sandbox do workspace, loop de tool calling
│   ├── whisper.rs    transcrição de voz (whisper.cpp); download.rs baixa o motor e o modelo
│   ├── usage.rs      uso dos planos (sessão e semana); usage/codex.rs e usage/antigravity.rs leem cada fonte
│   ├── context.rs    ponto de extensão para RAG (ContextProvider; o Ollama e o headless o consultam)
│   ├── git.rs        git do workspace: parse do status/diff/branches (puro, com testes) e as operações
│   ├── github.rs     GitHub (PRs, issues, Actions): remoto → dono/repo, token do credential helper, API REST, parse (puro, com testes)
│   ├── workspace.rs  pasta de trabalho: listagem da árvore, AGENTS.md, watcher e `read_text` (arquivo de texto até 1 MB para o modo Documentos; com `safe_rel` e conferência de links simbólicos)
│   ├── attachments.rs, sessions.rs, config.rs, paths.rs, process.rs, util.rs
│   ├── tests/bridge.rs   testes de ponta a ponta da ponte
│   ├── tests/git.rs      integração com o git real (pula se o git não estiver instalado)
│   ├── tests/tasks.rs    ciclo de vida das tarefas com o git real (pula se o git não estiver instalado)
│   ├── tests/tasks_bridge.rs   rotas /tasks e permissões do agente de uma tarefa
│   ├── tests/support/tasks_env.rs   cenário compartilhado dos dois (repositório, agente falso, git isolado)
│   └── tests/github.rs   remoto origin com o git real; um teste `--ignored` com a rede e a credencial reais
├── app/    (commanddeck)        interface gráfica (o .exe principal)
│   ├── main.rs / app.rs / window.rs   janela winit, OpenGL, loop de eventos
│   ├── theme.rs      cores, fontes e tamanhos (o "CSS")
│   ├── icons.rs      ícones SVG rasterizados em tempo de execução
│   ├── util.rs       funções puras compartilhadas pela interface
│   ├── agents.rs     cores, nomes e menus dos agentes do lado da interface
│   ├── terminal/     emulador de terminal: mod.rs (pty + alacritty_terminal), view.rs (desenho e mouse), input.rs (teclado), colors.rs (paleta)
│   ├── git_ui.rs     estado e ações da aba Git (seções, chip da topbar, releituras)
│   ├── github_ui.rs  estado e ações da aba GitHub (eventos do core, seleção, menu "Enviar para agente", dados de demonstração)
│   ├── tasks_ui.rs   estado e ações da aba Tarefas (lista, detalhe, formulário, chamadas ao core em threads) e as funções puras (grupos, resumo do diff, textos, botões por estado), com testes; dados de demonstração
│   ├── tasks_flow_tests.rs   o Deck da mesa de ponta a ponta com o core e o git reais (criar, iniciar, revisar, devolver, mesclar, validar, descartar, reabrir a aba)
│   ├── docs.rs       modo Documentos do centro: `CenterView`, `DocKind`/`DocTab`/`DocState`, `Docs` (abrir, focar, fechar, duplicata, por projeto), a lista de arquivos de um diff (`diff_files`), o selo da chave (`Badge`) e, em `impl Deck`, abrir cada tipo de documento e ler fora da thread da interface (`UiMsg::Doc`); testes puros
│   ├── docs_demo.rs  documentos fictícios do `COMMANDDECK_DEBUG_UI=docs-demo*`
│   ├── dictation.rs  ditado por voz: estados do microfone e destino fixo na aba onde a gravação começou (funções puras, com testes)
│   ├── split.rs      tela dividida: grupos, grade, divisores, vizinhos (funções puras, com testes)
│   ├── deck.rs       estado das abas e lógica de envio (consulta `delivery.rs` para digitar nas abas), anexos e arquivos soltos
│   ├── audio.rs      som de notificação e microfone (cpal)
│   ├── crash.rs      registro de quedas em dados/crash.log; smoke.rs, a captura de tela (COMMANDDECK_SMOKE)
│   └── ui/           mod.rs (pintura, modais)
│       ├── titlebar.rs (barra de título) + usage.rs (uso dos agentes na barra)
│       ├── panes.rs (janelas da tela dividida: cabeçalho com alça, microfone, anexo e "+") + tabs.rs (faixa de abas: degradê, setas ‹ ›, menu ▾; pinta uma lista de `Item`, serve às abas dos agentes e às dos documentos)
│       ├── docs.rs (modo Documentos: a chave Agentes | Documentos da barra de título, a faixa de documentos, o diff com a lista de arquivos, o arquivo de texto e o markdown)
│       ├── sidebar.rs (explorador e ORGANIZAÇÃO) + projects.rs (projetos e agentes em árvore)
│       ├── right_panel.rs (barra direita: Tarefas/Conversa/Git/GitHub; a Conversa é só leitura)
│       ├── tasks.rs (aba Tarefas), git.rs + git_views.rs (aba Git), github.rs (aba GitHub)
│       └── menus.rs (nova aba, configurações), shortcuts.rs (atalhos), overlay.rs (arrastar arquivos), empty_state.rs (nenhum agente aberto)
└── cli/    (commanddeck-cli)    ask-agent e ollama-agent
    ├── src/bin/ask-agent.rs     o comando dos agentes (`parse_task`, `learn_command`, `models_command`, com testes)
    ├── src/bin/ollama-agent.rs  o agente local com ferramentas
    └── tests/ask_agent.rs, tests/tasks.rs   o `ask-agent` de verdade contra uma ponte real
```

**Regra de ouro:** a interface (`app`) só fala com o backend pelos métodos de `Core` (em `crates/core/src/lib.rs`). Os eventos voltam pelo `CoreEvent` (`events.rs`). Mudou algo no backend? Nenhuma tela precisa saber como foi feito.

### Como a interface funciona (modo "imediato")

A interface usa **egui**, que funciona em *modo imediato*. Não existe uma árvore de elementos como no HTML. A cada quadro, o código "desenha" a tela inteira a partir do estado atual, e um clique é detectado no mesmo trecho que desenha o botão:

```rust
if ui.button("Enviar").clicked() {
    deck.send_message();
}
```

Para mudar o visual, mexa em `theme.rs` (cores e tamanhos) ou no arquivo de `ui/` da área correspondente. O **winit** só cria a janela e entrega os eventos do sistema operacional: teclado, mouse, arrastar arquivos e redimensionar.

## 3. Receitas comuns

**Adicionar um novo agente CLI.** Edite `crates/core/src/agents.rs`:
1. Crie uma entrada na tabela `AGENTS` com id, rótulo, cor, função de resolução e mensagem de "não instalado".
2. Se ele tiver modo não interativo, adicione os argumentos em `headless.rs` (no `match` por agente). O prompt
   **nunca** vai cru na linha de comando: use a entrada padrão ou `self.prompt_arg(prompt, via_cmd)`, que manda
   por arquivo quando o agente é um shim `.cmd` no Windows (o `cmd.exe` executaria `&`, `|`… do texto).
3. Se o CLI tiver hooks de início/fim de turno, ligue-os a `ask-agent --signal busy|idle` no `spawn_spec_for`
   (como o Claude e o Codex): a entrega nas abas fica mais precisa (`delivery.rs`).
4. Se o CLI escolhe o modelo, ensine `models::accepts_model`, `cli_args` (aba) e `headless_args` (headless); se não houver
   opção no modo interativo (como o TUI do OpenCode), ache outro caminho (configuração por variável de ambiente) e confira
   o resultado observando o CLI de verdade. Para listar modelos no menu, acrescente o agente em `Core::known_models`.
5. Opcionalmente, crie um alias em `ALIASES`, e atualize a lista de ids no teste `workspace_agents_md_and_spawn_specs`.

**Mudar uma cor ou tamanho.** Edite `crates/app/src/theme.rs` (larguras da barra direita: `RIGHT_*_W`).

**Mexer na tela dividida.** O cálculo é puro e testado em `crates/app/src/split.rs` (grupo de cada aba,
formato da grade, retângulos, divisores, vizinho para os atalhos, ordem das janelas, foco ao fechar); o
`Deck` guarda a ordem (`group_order`), as proporções (`split_ratios`) e qual aba aparece em cada janela (a
ativada mais recentemente, `Tab::shown`); a pintura fica em `crates/app/src/ui/panes.rs` (janelas, divisores,
arrasto) e `ui/tabs.rs` (faixa de abas). Para ver o arrasto sem mouse: `COMMANDDECK_DEBUG_UI=split-drag`.

**Mexer no ditado e nos anexos.** Os botões de microfone e de anexo ficam no cabeçalho de cada janela (`ui/panes.rs`) e agem na
aba visível dela; o estado do microfone e o destino fixo da transcrição são funções puras e testadas em `crates/app/src/dictation.rs`;
o que acontece com o arquivo (copiar para `dados/anexos`, digitar `@caminho` ou o caminho, `input::paste_bytes` sem Enter) e a zona de soltar
(`resolve_drop_zone`, `attach_dropped_files`) ficam em `deck.rs`, e o destaque ao arrastar, em `ui/overlay.rs`. O formulário "Nova
tarefa" anexa pelo mesmo caminho (`util::append_brief_attachments` junta os caminhos ao pedido).

**Mexer na aba Git.** As operações ficam em `crates/core/src/git.rs` (`impl Core`, com o ambiente seguro em
`base_command`); o estado e as releituras, em `crates/app/src/git_ui.rs`; a pintura, em `crates/app/src/ui/git.rs`
(o desenho só coleta as ações e aplica no fim). Comando novo do git: sempre `args([...])` com `--` antes de
caminhos, nunca um shell; teste a interpretação da saída com bytes de exemplo e o fluxo em `tests/git.rs`. Para ver
sem um repositório: `COMMANDDECK_DEBUG_UI=git-demo` (ou `git-diff`, `git-branches`, `git-norepo`, `git-nogit`).

**Mexer na aba GitHub.** A leitura fica em `crates/core/src/github.rs` (`impl Core`: `github_refresh`,
`github_load_pr`, `github_load_run_jobs`, `github_load_job_log`, todas em thread própria e devolvendo
`CoreEvent::Github`); o parse da API é puro e testado com JSON de exemplo (`parse_pulls`, `parse_issues`,
`parse_workflow_runs`, `parse_workflow_jobs`, `tail_log`, `parse_github_remote`, `parse_credential_fill`).
O token nunca sai de `SecretToken`: não o coloque em log, `format!` de erro nem config. O estado e o tratamento
dos eventos ficam em `crates/app/src/github_ui.rs`; a pintura, em `crates/app/src/ui/github.rs`. A mensagem
enviada ao agente é montada no core (`format_pr_prompt`, `format_issue_prompt`, `format_failed_job_prompt`) e
entregue por `Deck::send_to_instance` (abre a aba se não existir e enfileira como qualquer mensagem de outro
agente, com `request: 0`). Para ver sem rede: `COMMANDDECK_DEBUG_UI=github-demo` (ou `github-issues`,
`github-actions`, `github-menu`, `github-norepo`, `github-error`). Para testar a rede e o login reais:
`cargo test -p commanddeck-core --test github -- --ignored --nocapture`.

**Novo aprendizado.** Crie `memory/aprendizados/<nome>.md` (formato e critérios no `README.md` da pasta). Um frontmatter inválido é
ignorado com um aviso no log, e o teste `every_learning_in_the_repository_is_valid` (`learnings.rs`) falha se algum arquivo da pasta não
passar na validação ou se um `touches` literal apontar para um arquivo que não existe.

**Mexer nas tarefas.** O modelo e o ciclo de vida ficam em `crates/core/src/tasks.rs` (`impl Core`: `task_create`, `task_start`,
`task_done`, `task_return`, `task_merge`, `task_validate`, `task_reject`, `task_discard`; o git usa o executor seguro de `git.rs`); as rotas e as permissões, em
`bridge.rs` (`tasks_route`, `task_permission`); o comando, em `crates/cli/src/bin/ask-agent.rs` (`parse_task`, com testes). Textos
puros (slug, branch, mensagens ao agente, `exclude`) têm testes no próprio `tasks.rs`; o fluxo com o git real, em
`tests/tasks.rs` e `tests/tasks_bridge.rs` (cenário em `tests/support/tasks_env.rs`; a identidade do git vem de um
`GIT_CONFIG_GLOBAL` do teste). Mudou o que o agente recebe ao iniciar? É `tasks::start_message`.

**Mexer na aba Tarefas (interface).** O estado e as ações ficam em `crates/app/src/tasks_ui.rs`; a pintura, em
`crates/app/src/ui/tasks.rs` (a pintura só *coleta* o que foi clicado em `Act`; `apply` aplica no fim do quadro).
Toda chamada de tarefa do core roda git, então vai para `Deck::spawn_bg` e volta como `UiMsg::Tasks(geração, TaskReply)`
(a geração muda a cada troca de projeto e descarta respostas do anterior); a lista é relida ao abrir a aba, a cada
`CoreEvent::TasksChanged` e depois de cada ação. `tasks_flow_tests.rs` roda esse fluxo com o core e o git reais. O número do ícone da aba é `TasksUi::review_count`. Quais botões cada
estado tem é `actions_for` (função pura, com testes); o agrupamento da lista, o resumo do diff (`parse_diff_stat`) e os
textos de tempo também. O diff completo vem de `Core::task_diff` e abre no centro (`Deck::docs_open_task_diff`, ver "Mexer no modo
Documentos"), desenhado por `ui::git::diff_lines_at`, o mesmo desenho da aba Git (sem os botões por trecho). Reabrir a aba de uma tarefa é `Deck::open_task_tab` (registra a instância
de novo; o terminal abre na worktree sozinho). Para ver sem git nem agentes: `COMMANDDECK_DEBUG_UI=tasks-demo` (ou
`tasks-empty`, `tasks-detail`, `tasks-running`, `tasks-failed`, `tasks-confirm`, `tasks-discard`,
`tasks-return`, `tasks-error`, `tasks-busy`, `tasks-new`, `tasks-new-agent`, `tasks-new-model`; `debug_tasks` em `app.rs`).
Para a largura mínima da barra, ponha `{"rightPanelWidth": 280}` em `cfg/config.json` da pasta de `COMMANDDECK_HOME`.

**Mexer no modo Documentos.** O estado e a lógica pura ficam em `crates/app/src/docs.rs` (`Docs`: abrir/focar/fechar por projeto, sem
duplicata; `diff_files`: a lista de arquivos de um `GitDiff`; `DocTab::apply`: o que cada tipo mostra ao receber a leitura), com
testes; a pintura, em `ui/docs.rs` (a chave é chamada pela barra de título; `ui/mod.rs` escolhe entre `panes::draw` e `docs::draw`) e a
faixa de abas, em `ui/tabs.rs` (`strip_items`, o mesmo desenho dos agentes). Documento novo = um `DocKind`, o `DocState` que ele mostra,
a leitura em `Deck::docs_load` (numa thread, volta como `UiMsg::Doc`) e o corpo em `ui/docs.rs::document`. O arquivo vem de
`Core::read_text` (1 MB, `safe_rel`); o diff, de `Core::task_diff`, `git_commit_detail` ou `git_diff`; o markdown é desenhado pelo
**`egui_commonmark`** (a versão acompanha a do egui: a 0.25 é a do egui 0.36; ver `memory/aprendizados/egui-commonmark-acompanha-o-egui.md`).
Regras que não se quebram: com o centro em Documentos **nenhum `fit_tab`** roda (o PTY não muda de tamanho), o terminal não recebe teclado
nem IME (`handle_key`, `handle_ime`) e o fim de turno marca `Tab.done` mesmo na aba ativa. O watcher (`CoreEvent::WorkspaceChanged`) relê
os arquivos abertos (`docs_reload_files`). `tasks_flow_tests.rs` cobre o fluxo com o core e o git reais. Para ver sem nada disso:
`COMMANDDECK_DEBUG_UI=docs-demo` (ou `docs-demo-file`, `-md`, `-report`, `-unreadable`, `-many`, `-badge`; `docs_demo.rs`).

**Mudar o texto ou as ferramentas do agente Ollama.** Edite `crates/core/src/ollama.rs` (`system_prompt` e `tool_schemas`/`execute_tool`).

**Plugar RAG (futuro).** Implemente o trait `ContextProvider` (`context.rs`) e registre com `core.register_context_provider(...)`. O agente Ollama já consulta os provedores registrados.

**Adicionar um teste.** Coloque um `#[test]` num bloco `#[cfg(test)] mod tests` no fim do arquivo. Para testes de ponta a ponta, use `crates/core/tests/`.
Para chamar a ponte como uma aba, pegue o token dela com `spawn_spec_for(&ident).env["COMMANDDECK_TOKEN"]`
(veja `tab_token` em `tests/bridge.rs`); com o token mestre, o `from` do corpo é aceito. Os testes iniciam o
`Core` com `system_dirs: false`, para não depender dos CLIs instalados na máquina.

## 4. Trabalhando com agentes de IA no projeto

- Peça sempre para o agente rodar `cargo fmt`, `cargo clippy ... -D warnings` e `cargo test --workspace` antes de concluir. É o mesmo que o CI faz.
- Mudanças em `Core` (lib.rs) afetam a interface. Peça mudanças pequenas e uma área de cada vez (core **ou** app).
- O **CI** (Integração Contínua) é o robô do GitHub (`.github/workflows/ci.yml`). A cada push ele compila e testa no Windows e no Linux, e marca o commit/PR com ✅ ou ❌. Se ficar ❌, abra o log da execução para ver o erro.
- Para depurar visualmente sem abrir a janela: `COMMANDDECK_SMOKE=<pasta>` salva uma captura `screen.png` e fecha o app (sem baixar o whisper, para não começar 1,6 GB em cada pasta temporária). Também existem `COMMANDDECK_DEBUG_FEED=1`, `COMMANDDECK_DEBUG_FAKE_SHELL=1` e `COMMANDDECK_DEBUG_UI=…` (`tasks-demo`, `tasks-detail`, `tasks-new`, `newtab`, `newtab-models:<agente>`, `usage-demo`, `git`, `git-demo`, `git-diff`, `github-demo`, `split-drag`, `docs-demo`, `docs-demo-file`, `docs-demo-md`, `docs-demo-report`, `docs-demo-unreadable`, `docs-demo-many`, `docs-demo-badge`, `drop`, `drop-pane:<n>`, `drop-task`, `mic-recording`, `target`, `attach`, `text`, `download`…; lista em `apply_debug_ui`, `app.rs`). No Linux, rode sob `xvfb-run`.
- Logs: rode com `RUST_LOG=debug` para ver mensagens detalhadas no console (em build debug).

## 5. Limitações conhecidas

A lista fica em [pendencias.md](pendencias.md), por área; o que ainda não rodou no Windows está em [testes.md](testes.md).

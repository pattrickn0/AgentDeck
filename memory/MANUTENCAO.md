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
│   ├── agents.rs     lista de agentes, onde achar cada CLI, comando de cada aba
│   ├── bridge.rs     servidor HTTP local usado pelo ask-agent (/agents, /ask, /reply, /signal)
│   ├── exchange.rs   perguntas entre abas esperando resposta (ciclos, profundidade, resposta por número)
│   ├── instances.rs  nomes das instâncias (modelo@cargo@nome) e resolução do destino
│   ├── models.rs     modelo de LLM de cada instância: resolução (pedido > gravado > role_models), validação, argumentos por CLI, lista de modelos
│   ├── delivery.rs   quando digitar uma mensagem no terminal de uma aba (regra pura, com testes)
│   ├── headless.rs   roda um agente em modo -p (sem terminal) e captura a resposta
│   ├── ollama.rs     agente Ollama: ferramentas, sandbox do workspace, loop de tool calling
│   ├── whisper.rs    transcrição de voz (whisper.cpp)
│   ├── attachments.rs, sessions.rs, workspace.rs, config.rs, paths.rs, process.rs, util.rs
│   ├── context.rs    ponto de extensão para RAG (vazio hoje)
│   ├── git.rs        git do workspace: parse do status/diff/branches (puro, com testes) e as operações
│   ├── github.rs     GitHub (PRs, issues, Actions): remoto → dono/repo, token do credential helper, API REST, parse (puro, com testes)
│   ├── tests/bridge.rs   testes de ponta a ponta da ponte
│   ├── tests/git.rs      integração com o git real (pula se o git não estiver instalado)
│   └── tests/github.rs   remoto origin com o git real; um teste `--ignored` com a rede e a credencial reais
├── app/    (commanddeck)        interface gráfica (o .exe principal)
│   ├── main.rs / app.rs / window.rs   janela winit, OpenGL, loop de eventos
│   ├── theme.rs      cores, fontes e tamanhos (o "CSS")
│   ├── terminal/     emulador de terminal (pty + alacritty_terminal + desenho + teclado)
│   ├── agent_map.rs  mapa de conversas entre agentes em texto (render_map: função pura, com testes)
│   ├── git_ui.rs     estado e ações da aba Git (seções, chip da topbar, releituras)
│   ├── github_ui.rs  estado e ações da aba GitHub (eventos do core, seleção, menu "Enviar para agente", dados de demonstração)
│   ├── split.rs      tela dividida: grupos, grade, divisores, vizinhos (funções puras, com testes)
│   ├── ui/           titlebar (barra de título + uso dos agentes em usage.rs), panes (janelas da tela dividida) + tabs (abas de cada janela), sidebar
│   │                 (árvore), right_panel (barra direita: Conversa/Mapa/Git/GitHub), map, git, github, compositor, menus
│   ├── deck.rs       estado das abas e lógica de envio (consulta `delivery.rs` para digitar nas abas)
│   └── audio.rs      som de notificação e microfone (cpal)
└── cli/    (commanddeck-cli)    ask-agent e ollama-agent
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

**Mudar o Mapa de agentes.** O conteúdo é a função pura `agent_map::render_map` (com testes de largura e de
caracteres); os dados vêm de `Deck::map_snapshot`, que junta `Core::conversation_map` (perguntas pendentes no
core) com o que só a interface sabe (filas das abas, quem está ocupado, recentes). Use só caracteres que
existem nas fontes monoespaçadas do Windows (o teste confere). Para ver sem agentes reais:
`COMMANDDECK_DEBUG_UI=map-demo`.

**Mexer na tela dividida.** O cálculo é puro e testado em `crates/app/src/split.rs` (grupo de cada aba,
formato da grade, retângulos, divisores, vizinho para os atalhos, ordem das janelas, foco ao fechar); o
`Deck` guarda a ordem (`group_order`), as proporções (`split_ratios`) e qual aba aparece em cada janela (a
ativada mais recentemente, `Tab::shown`); a pintura fica em `crates/app/src/ui/panes.rs` (janelas, divisores,
arrasto) e `ui/tabs.rs` (faixa de abas). Para ver o arrasto sem mouse: `COMMANDDECK_DEBUG_UI=split-drag`.

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
- Para depurar visualmente sem abrir a janela: `COMMANDDECK_SMOKE=<pasta>` salva uma captura `screen.png` e fecha o app (sem baixar o whisper, para não começar 1,6 GB em cada pasta temporária). Também existem `COMMANDDECK_DEBUG_FEED=1`, `COMMANDDECK_DEBUG_FAKE_SHELL=1` e `COMMANDDECK_DEBUG_UI=…` (`newtab`, `newtab-models:<agente>`, `map`, `map-demo`, `usage-demo`, `git`, `git-demo`, `git-diff`, `github-demo`, `split-drag`, `target`, `attach`, `text`, `download`…; lista em `apply_debug_ui`, `app.rs`). No Linux, rode sob `xvfb-run`.
- Logs: rode com `RUST_LOG=debug` para ver mensagens detalhadas no console (em build debug).

## 5. Limitações conhecidas

A lista atualizada fica em [pendencias.md](pendencias.md). As principais:

- **IME no terminal:** ligado e coberto por testes, mas ainda não exercitado com um IME real (teclado CJK, `Win+.`).
- **Entrega nas abas:** só Claude e Codex têm hooks de turno; os outros agentes dependem da heurística da saída.
- Emojis aparecem monocromáticos (limitação do egui).

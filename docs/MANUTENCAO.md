# Guia de manutenção

Este guia é para quem vai manter o CommandDeck, sozinho ou junto com agentes de IA. Ele não pressupõe experiência com Rust.

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
cargo fmt --all                     # formata o código
cargo update                        # atualiza dependências dentro das versões permitidas
```

A primeira compilação demora alguns minutos, porque compila todas as dependências. As seguintes são incrementais e rápidas. A pasta `target/` pode ser apagada a qualquer momento: ela é só cache de build.

## 2. Mapa do código

```
Cargo.toml                 workspace + versões das dependências compartilhadas
crates/
├── core/   (commanddeck-core)   "backend": nada de interface gráfica
│   ├── lib.rs        Core: a API que a interface usa (como os canais IPC)
│   ├── agents.rs     lista de agentes, onde achar cada CLI, comando de cada aba
│   ├── bridge.rs     servidor HTTP local usado pelo ask-agent (/agents, /ask)
│   ├── headless.rs   roda um agente em modo -p (sem terminal) e captura a resposta
│   ├── ollama.rs     agente Ollama: ferramentas, sandbox do workspace, loop de tool calling
│   ├── table.rs      mesas de orquestração (rodadas entre agentes)
│   ├── whisper.rs    transcrição de voz (whisper.cpp)
│   ├── attachments.rs, sessions.rs, workspace.rs, config.rs, paths.rs, process.rs, util.rs
│   ├── context.rs    ponto de extensão para RAG (vazio hoje)
│   └── tests/bridge.rs   testes de ponta a ponta da ponte
├── app/    (commanddeck)        interface gráfica (o .exe principal)
│   ├── main.rs / app.rs / window.rs   janela winit, OpenGL, loop de eventos
│   ├── theme.rs      cores, fontes e tamanhos (o "CSS")
│   ├── terminal/     emulador de terminal (pty + alacritty_terminal + desenho + teclado)
│   ├── ui/           topbar/abas, sidebar (árvore), feed, compositor, menus, overlay
│   ├── deck.rs       estado das abas e lógica de envio
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
2. Se ele tiver modo não interativo, adicione os argumentos em `headless.rs` (no `match` por agente).
3. Opcionalmente, crie um alias em `ALIASES`.

**Mudar uma cor ou tamanho.** Edite `crates/app/src/theme.rs`.

**Mudar o texto ou as ferramentas do agente Ollama.** Edite `crates/core/src/ollama.rs` (`system_prompt` e `tool_schemas`/`execute_tool`).

**Plugar RAG (futuro).** Implemente o trait `ContextProvider` (`context.rs`) e registre com `core.register_context_provider(...)`. O agente Ollama já consulta os provedores registrados.

**Adicionar um teste.** Coloque um `#[test]` num bloco `#[cfg(test)] mod tests` no fim do arquivo. Para testes de ponta a ponta, use `crates/core/tests/`.

## 4. Trabalhando com agentes de IA no projeto

- Peça sempre para o agente rodar `cargo fmt`, `cargo clippy ... -D warnings` e `cargo test --workspace` antes de concluir. É o mesmo que o CI faz.
- Mudanças em `Core` (lib.rs) afetam a interface. Peça mudanças pequenas e uma área de cada vez (core **ou** app).
- O **CI** (Integração Contínua) é o robô do GitHub (`.github/workflows/ci.yml`). A cada push ele compila e testa no Windows e no Linux, e marca o commit/PR com ✅ ou ❌. Se ficar ❌, abra o log da execução para ver o erro.
- Para depurar visualmente sem abrir a janela: `COMMANDDECK_SMOKE=<pasta>` salva uma captura `screen.png` e fecha o app. Também existem `COMMANDDECK_DEBUG_FEED=1` e `COMMANDDECK_DEBUG_UI=…`.
- Logs: rode com `RUST_LOG=debug` para ver mensagens detalhadas no console (em build debug).

## 5. Limitações conhecidas

- **Mouse no terminal.** Programas de terminal "tela cheia", como `vim`, `htop` ou menus interativos, podem pedir para receber cliques do mouse (o chamado *mouse mode*). Hoje repassamos só a **roda** do mouse. Clicar dentro desses programas não chega até eles, mas a rolagem funciona e a seleção de texto com o mouse continua funcionando normalmente. Nos CLIs de agentes, que se usam pelo teclado, quase nunca faz diferença. A implementação ficaria em `crates/app/src/terminal/` (traduzir clique/movimento nas sequências SGR `\x1b[<...M`).
- **IME no terminal.** IME é o sistema que monta caracteres compostos antes de enviá-los: teclados chinês/japonês/coreano, o teclado de emoji do Windows (`Win+.`) e alguns métodos de entrada. No **compositor** (a caixa de mensagem) funciona. Dentro do **terminal**, o texto em composição ainda não é encaminhado. Acentos do português (á, ç, ã...) funcionam normalmente nos dois, porque não passam por IME.
- Emojis aparecem monocromáticos (limitação do egui).

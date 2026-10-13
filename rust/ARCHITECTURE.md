# CommandDeck Terminal — versão nativa em Rust (winit)

Reescrita da aplicação Electron (`../main.js`, `../ollamaAgent.js`, `../renderer/`, `../bin/`)
com **as mesmas funções e o mesmo visual**, em Rust nativo.

## Stack (decisões do Lead)

| Camada | Escolha | Por quê |
| :-- | :-- | :-- |
| Janela/eventos | **winit 0.30** (loop próprio, `ApplicationHandler`) | pedido do projeto; controle total do loop e de DnD |
| Render | **glutin + glow** com **egui** (`egui-winit` + `egui_glow`) | widgets prontos e estilização fina para reproduzir o CSS; OpenGL leve (roda até em Xvfb/llvmpipe para testes) |
| Terminal | **alacritty_terminal** (parser VT + grade) + **portable-pty** (ConPTY no Windows, PTY no Unix) | emulação madura; o render da grade é nosso (pintado no egui) |
| Ponte HTTP | **tiny_http** (thread por requisição) | mesma API `/agents` e `/ask`, token + validação de Host |
| Cliente HTTP | **ureq** (Ollama `/api/chat`, `/api/tags`, downloads) | bloqueante e simples, roda em threads |
| Arquivos | **notify** (watcher), **rfd** (diálogos), **open** (abrir no SO), **arboard** (clipboard texto/imagem) | |
| Áudio | **cpal** (microfone, feature `audio`) + whisper.cpp (binários existentes em `../whisper`) | |

Sem Node/Electron em runtime: `ask-agent` e `ollama-agent` viram binários Rust ao lado do app.

## Crates

```
rust/
├── crates/core   commanddeck-core   backend sem GUI (main.js + ollamaAgent.js)
├── crates/app    commanddeck        UI winit+egui (renderer/ + preload.js)
└── crates/cli    commanddeck-cli    bins ask-agent e ollama-agent (bin/)
```

A UI fala com o backend apenas via `commanddeck_core::Core` (métodos ≈ canais IPC) e recebe
`CoreEvent` por um `crossbeam_channel::Receiver`, com um `Waker` que acorda o loop do winit
(`EventLoopProxy`). Operações bloqueantes (listar agentes, mesa, transcrição) rodam em threads.

### Mapa IPC (Electron) → Rust

| Electron | Rust |
| :-- | :-- |
| `pty:spawn/write/resize/kill/submit` | `Core::spawn_spec(kind)` + PTY gerenciado na UI (`app/src/terminal`) |
| `feed`, `workspace:changed` | `CoreEvent::Feed`, `CoreEvent::WorkspaceChanged` |
| `table:run` / `table:stop` | `Core::run_table` / `Core::stop_table` |
| `attach:*` | `Core::attach_files`, `attach_bytes`, `attach_rgba_png` |
| `audio:engine` / `audio:transcribe` | `Core::transcribe_engine`, `Core::transcribe_pcm16k` |
| `agents:list` | `Core::list_agents` |
| `workspace:get/pick/readDir/openPath` | `Core::workspace`, `set_workspace`, `read_dir`, `open_path` |
| `config:get/save` | `Core::config`, `Core::update_config` |

## Ponto de extensão para RAG (futuro)

`core::context::ContextProvider` — hoje vazio. O motor Ollama e o executor headless
consultam os provedores registrados antes de enviar o prompt; plugar RAG local/remoto
não exige alterar o resto do sistema.

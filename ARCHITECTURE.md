# Arquitetura do CommandDeck Terminal

Aplicação nativa em Rust. (Origem: reescrita da antiga versão Electron, com as mesmas funções e o mesmo visual.)

## Stack (decisões do Lead)

| Camada | Escolha | Por quê |
| :-- | :-- | :-- |
| Janela/eventos | **winit 0.30** (loop próprio, `ApplicationHandler`) | pedido do projeto; controle total do loop e de DnD |
| Render | **glutin + glow** com **egui** (`egui-winit` + `egui_glow`) | widgets prontos e estilização fina para reproduzir o CSS; OpenGL leve (roda até em Xvfb/llvmpipe para testes) |
| Terminal | **alacritty_terminal** (parser VT + grade) + **portable-pty** (ConPTY no Windows, PTY no Unix) | emulação madura; o render da grade é nosso (pintado no egui) |
| Ponte HTTP | **tiny_http** (thread por requisição) | mesma API `/agents` e `/ask`, token + validação de Host |
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
(`EventLoopProxy`). Operações bloqueantes (listar agentes, mesa, transcrição) rodam em threads.

## Ponto de extensão para RAG (futuro)

`core::context::ContextProvider` — hoje vazio. O motor Ollama e o executor headless
consultam os provedores registrados antes de enviar o prompt; plugar RAG local/remoto
não exige alterar o resto do sistema.

## Protocolo entre agentes (`ask-agent`)

Cada aba recebe no ambiente: `COMMANDDECK_PORT` (porta da ponte), `COMMANDDECK_TOKEN` (token de 128 bits por sessão),
`COMMANDDECK_AGENT` (nome do agente da aba), `COMMANDDECK_DEPTH` (profundidade de chamadas) e a pasta dos
executáveis no início do `PATH`.

| Rota | Descrição |
| :-- | :-- |
| `GET /agents` | `{"agents": [...]}` com os agentes instalados |
| `POST /ask` | `{"from","to","message","depth"}` → `{"answer"}`; 400 para agente desconhecido, auto-pergunta ou `depth >= 2`; 403 para token/Host inválido; 500 em erro/timeout (10 min) |

Toda requisição exige o cabeçalho `x-commanddeck-token`. Pedidos ao mesmo par `de->para` são
serializados, e o histórico de cada par é mantido entre chamadas: `--session-id`/`--resume` no
Claude, `--conversation` no Antigravity, e memória em processo no Ollama.

`ask-agent` sai com código 0 (resposta no stdout), 1 (erro do agente) ou 2 (uso incorreto ou fora do CommandDeck).

### Instâncias (abas) e perguntas entre elas

Cada aba é uma **instância** com identidade própria, usada em `COMMANDDECK_AGENT` e como destino do
`ask-agent`: `<modelo>[@<cargo>[@<nome>]]`.

| Id | Significado |
| :-- | :-- |
| `claude` | a instância principal (sem cargo), a que o usuário abre pelo "+" |
| `claude@dev`, `claude@design` | instâncias com cargo, abertas por um agente (ou pelo usuário) |
| `claude@dev@ana` | quando já existe outra do mesmo modelo **e** cargo, a nova ganha um nome |
| `claude@main@bia` | segunda instância principal (sem cargo) aberta à mão |

A aba aparece como `Claude`, `Claude · Dev`, `Claude · Dev · Ana`. O id é gerado pelo core
(`crates/core/src/instances.rs`) e salvo em `Config.open_tabs`, então as abas voltam com o mesmo nome.
O cargo também vai em `COMMANDDECK_ROLE` e no prompt de sistema.

`ask-agent <id> "msg"` (e `--new` para forçar outra instância) funciona assim:

1. O core resolve o destino: o id exato, a instância principal do modelo ou a única que existe;
   se nenhuma existe, **cria** (`claude@dev` abre uma aba nova). Destino ambíguo ou desconhecido dá erro.
2. A pergunta vira `CoreEvent::Deliver`. A interface abre a aba (em segundo plano, sem roubar o foco),
   espera o terminal ficar parado e **digita a mensagem nele**, uma por vez por aba (fila).
3. A mensagem digitada ensina a responder: `ask-agent <quem-perguntou> "resposta"`. Essa chamada é
   reconhecida como resposta a uma pergunta pendente (`crates/core/src/exchange.rs`) e entregue a
   quem espera; o `ask-agent` de quem perguntou fica bloqueado até lá (10 minutos no máximo).
   Nada de raspar a tela do terminal.
4. O `ollama-agent` devolve a resposta sozinho (`POST /reply`, que só conclui perguntas pendentes e
   nunca cria uma nova), porque modelos pequenos esquecem de rodar o comando.
5. Fechar a aba ou o processo terminar libera quem esperava, com erro. Ciclos de espera (A espera B,
   B espera C, C espera A) são recusados na hora (HTTP 409).

Tudo fica visível nas abas (raciocínio, ferramentas, aprovações) e o usuário pode intervir digitando.
O Claude abre com `--allowedTools "Bash(ask-agent:*)"`, para responder sem pedir aprovação.

O **modo headless** (`claude -p`, `agy -p`) continua em dois casos: nas **mesas** (`table.rs`) e quando a
interface não ligou a entrega em abas ou o usuário desligou "Abrir abas dos agentes" nas Configurações.
Limites: no máximo 8 abas abertas automaticamente.

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

### Abas abertas pela ponte (visualizador)

O transporte é sempre o modo headless (o agente de destino **não precisa** de aba aberta). Quando o
destino não tem aba, o app abre uma sozinha:

1. `POST /ask` valida o destino e emite `CoreEvent::EnsureAgentTab { agent, pair }` (se
   `Config.auto_open_agent_tabs`), no mesmo canal da entrada `Ask` do feed. É idempotente: quem
   deduplica é a UI.
2. A UI abre a aba em segundo plano (não rouba o foco). `claude`, `antigravity` e `ollama:<modelo>`
   abrem como **visualizador** (`crates/app/src/viewer.rs`): sem processo, mostra as perguntas e
   respostas da ponte (e as chamadas de ferramenta do Ollama) e uma linha de prompt. Os demais
   agentes abrem como aba interativa comum.
3. **Assumir:** digitar uma mensagem e dar Enter (Enter vazio não faz nada) transforma a aba no CLI
   do agente, retomando a conversa da ponte e já enviando a mensagem: `claude --resume <id> -- <msg>`,
   `agy --conversation <id> -i <msg>`, e `COMMANDDECK_INITIAL_PROMPT` no `ollama-agent` (que começa
   uma conversa nova). Ver `SpawnExtras` em `crates/core/src/agents.rs`.
4. **Um escritor por conversa:** enquanto a aba assumida estiver viva, o par é "reivindicado"
   (`Core::claim_conversation`) e a ponte usa a conversa `<par>@bg`, avisando uma vez no feed. Se a
   ponte ainda estiver respondendo no par, assumir é recusado até ela terminar
   (`Core::conversation_busy`).
5. **Limites:** no máximo 3 abas automáticas; fechar uma aba automática impede que ela reabra até
   fechar o app (abrir à mão limpa isso); visualizadores não são salvos em `open_tabs`; o texto de
   agentes é saneado (sem sequências de escape) antes de ir ao terminal. A opção fica em
   Configurações ("Abrir abas dos agentes").

# CommandDeck Terminal — versão nativa (Rust + winit)

Reescrita nativa do CommandDeck Terminal, com as mesmas funções e o mesmo visual da versão
Electron (na raiz do repositório). Nada de Node ou Electron em runtime: o app,
o `ask-agent` e o `ollama-agent` são binários Rust.

![Tela principal](docs/screenshots/main.png)

## Compilar e rodar

```powershell
cd rust
cargo build --workspace --release
.\target\release\commanddeck.exe
```

- Os três binários (`commanddeck`, `ask-agent`, `ollama-agent`) precisam ficar **na mesma pasta**.
  A pasta deles entra no início do `PATH` de cada aba, e as abas Ollama abrem o `ollama-agent`.
- As pastas `dados/`, `workspace/` e `whisper/` são procuradas subindo a partir do executável
  (em desenvolvimento, isso cai na raiz do repositório). Para usar outra pasta, defina `COMMANDDECK_HOME`.
- Configuração: `%APPDATA%\CommandDeck\config.json`. O mesmo arquivo da versão Electron também é lido.
- Feature `audio` (padrão): som de notificação e microfone via cpal. Para compilar sem ela:
  `cargo build -p commanddeck --no-default-features`.
- Linux (dev/CI): instale `libasound2-dev libgtk-3-dev libxkbcommon-dev`.

## Estrutura

| Crate | Equivale a | Conteúdo |
| :-- | :-- | :-- |
| `crates/core` (`commanddeck-core`) | `main.js`, `ollamaAgent.js` | agentes, ponte HTTP, headless, motor Ollama, mesas, anexos, Whisper, retenção, watcher |
| `crates/app` (`commanddeck`) | `renderer/`, `preload.js` | janela winit + egui (glow), terminal (alacritty_terminal + portable-pty/ConPTY), UI |
| `crates/cli` (`commanddeck-cli`) | `bin/*.js` | `ask-agent` e `ollama-agent` |

Detalhes e decisões em [ARCHITECTURE.md](ARCHITECTURE.md).

## CLI `ollama-agent`

Ainda não há injeção de prompt nem RAG; isso fica para uma etapa futura. A CLI em si já ficou mais completa:

- edição de linha com histórico e Tab para completar comandos e modelos;
- resposta em streaming, com indicador "pensando…" até chegar o primeiro token;
- chamadas de ferramenta exibidas ao vivo, com prévia do resultado;
- Ctrl+C cancela só a resposta atual (e mata o comando em execução); Ctrl+C duas vezes ou Ctrl+D sai;
- `/help`, `/models`, `/switch <modelo|nº>`, `/clear`, `/tools`, `/workspace`, `/history`, `/exit`;
- rodapé com tempo, nº de ferramentas/passos e modelo; avisos claros quando o Ollama está fora do ar.

## Pronto para RAG

`commanddeck_core::context::ContextProvider` + `Core::register_context_provider`: o motor
Ollama consulta os provedores registrados antes de cada mensagem. Hoje nenhum está registrado.

## Verificação

```bash
cargo test --workspace      # 84 testes (core, CLI e UI)
cargo clippy --workspace --all-targets -- -D warnings
```

Modo de captura para inspeção visual: `COMMANDDECK_SMOKE=<pasta>` salva `screen.png` após
`COMMANDDECK_SMOKE_DELAY` ms e encerra. Hooks de depuração: `COMMANDDECK_DEBUG_FEED=1`,
`COMMANDDECK_DEBUG_UI=…`, `COMMANDDECK_DEBUG_FAKE_SHELL=1`.

## Diferenças conhecidas em relação à versão Electron

- Terminal: só a roda do mouse é repassada a apps em modo mouse (cliques e movimento ainda não); IME não chega ao terminal (no compositor funciona).
- Select, spinner e tooltips são desenhados à mão imitando o Chromium; o desfoque atrás do overlay de arrastar é aproximado.
- Emojis aparecem monocromáticos, porque o egui não renderiza emoji colorido.
- Correções de segurança no motor Ollama: `delete_file` não apaga a raiz, `edit_file` substitui o texto literalmente, `run_command` usa `-EncodedCommand` e timeouts encerram a árvore de processos inteira.
- O layout (largura da barra lateral, altura da área inferior, painéis recolhidos) agora é persistido.

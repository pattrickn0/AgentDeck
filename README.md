# ⚡ AgentDeck

**Um terminal desktop onde os seus agentes de IA trabalham em equipe.** Claude Code, Codex, Antigravity e OpenCode
rodam lado a lado, conversam entre si e recebem tarefas isoladas que você revisa e mescla, sem perder nada de vista.

![AgentDeck com Claude, Codex e OpenCode no modo Destaque e a mesa de tarefas à direita](docs/screenshots/main.png)

Feito 100% em Rust, nativo para **Windows** e **Linux**. Licença MIT.

---

## Por que existe

Cada CLI de agente é bom sozinho, mas trabalhar com vários ao mesmo tempo vira uma pilha de terminais soltos: você
copia texto de um para o outro, perde o que cada um está fazendo e torce para dois agentes não mexerem no mesmo
arquivo.

O AgentDeck junta tudo numa janela e dá aos agentes o que falta para colaborar:

- **Um jeito de conversar.** Dentro de qualquer aba, o agente roda `ask-agent codex "revise isto"`. A mensagem é
  digitada no terminal do outro agente, e a resposta volta para quem perguntou. Você vê tudo acontecendo e pode
  intervir em qualquer aba.
- **Trabalho isolado e revisável.** Uma tarefa ganha branch e pasta (git worktree) próprias. O agente trabalha lá,
  entrega um relatório, e você lê o diff e decide se mescla, devolve com ajustes ou descarta.
- **Memória do projeto.** O que o projeto já aprendeu (armadilhas, decisões, convenções) fica em arquivos versionados,
  e o agente recebe o pedaço que vale para os arquivos em que vai mexer.

Nada de trabalho invisível: todo agente tem uma aba de terminal de verdade, e nenhum abre subagentes escondidos.

## O que dá para fazer

### Agentes

- **Abas de terminal reais** (ConPTY no Windows) para cada agente, restauradas quando o app reabre.
- **Várias instâncias por agente**, com cargo e nome: `claude`, `claude@dev`, `claude@dev@ana`. Cada uma tem o seu
  **modelo** (`opus`, `sonnet`, `gpt-6-luna`, `opencode/deepseek-v4.1-flash-free`…), mostrado ao lado do nome.
- **Equipe**: arraste um agente sobre outro na barra lateral para definir chefe e subordinados.
- **Uso dos planos** na barra de título: a sessão de 5 h e a semana de cada agente, com aviso a partir de 80%. Um modelo
  esgotado não recebe pergunta.
- **Instruções do AgentDeck em toda aba**, entregues pelo canal de cada CLI, sem depender do `AGENTS.md` do seu projeto.
- **Sem perguntas que travam**: a pasta aberta como projeto já é registrada como confiável no Claude, no Codex e no
  Antigravity, para uma aba aberta em segundo plano não parar em "Do you trust this folder?".

### Mesa de tarefas

A aba **Tarefas** da barra direita é onde o trabalho grande acontece:

```
rascunho ─→ em andamento ─→ em revisão ─→ aguardando validação ─→ concluída
   │              ↑              │                  │
   └→ na fila ────┘              └ devolver ↺       └ reprovar ↺ (volta ao mesmo agente)
```

Em qualquer etapa antes de concluída, a tarefa pode ser descartada, e a branch e a pasta somem junto.

- **Escolha do agente pela capacidade e pela cota.** Diga o tamanho do trabalho (`simples`, `media` ou `complexa`). A
  mesa filtra os modelos que dão conta, por benchmarks públicos e pelo histórico das tarefas já feitas, e entre eles
  escolhe o de mais folga de cota.
- **Escopo com fila.** A tarefa pode declarar os caminhos que vai mexer. Outra que precise dos mesmos caminhos espera
  **na fila** e começa sozinha quando a primeira termina.
- **Revisão de verdade**: relatório, diff com realce de sintaxe e a linha do tempo das mensagens que o agente trocou.
- **Mesclagem assistida**: se a mesclagem é recusada (alterações pendentes, conflito), a própria janela explica o
  motivo e oferece a saída.

### Interface

- **Projetos**: cada projeto é uma pasta com os seus agentes. Trocar de projeto esconde as abas do anterior, que
  continuam rodando.
- **Três organizações** do centro: **Abas** (uma janela só), **Dividida** (uma janela por tipo de agente, em grade) e
  **Destaque** (a janela da aba ativa grande, as outras empilhadas ao lado).
- **Conversa integrada**: as mensagens entre agentes em tempo real, só leitura.
- **Documentos**: o centro alterna para ler e **editar** arquivos com realce de sintaxe e busca, e para ler diffs e
  relatórios com a largura toda (`Ctrl+Shift+D`).
- **Git** (alterações, stage por trecho, commit, branches, merge e rebase, conflitos, stash) e **GitHub** (PRs com os
  checks, issues, Actions e o log do job que falhou; um clique manda a falha para um agente).
- **Anexos** por botão, arrastar e soltar ou `Ctrl+V` com imagem, e **ditado por voz opcional** com whisper.cpp local
  (CUDA, Vulkan ou CPU). O áudio é apagado logo depois de transcrever.
- **Bandeja do sistema**: o × esconde a janela e os agentes seguem trabalhando; abrir o app de novo traz a mesma janela.

## CLIs suportados

Instale os que quiser usar. O AgentDeck usa o que encontrar instalado.

| CLI | Instalação |
| :-- | :-- |
| Claude Code | `npm install -g @anthropic-ai/claude-code` |
| Codex | `npm install -g @openai/codex` |
| Antigravity | Windows: `irm https://antigravity.google/cli/install.ps1 \| iex` · Linux: instalador em https://antigravity.google |
| OpenCode | `npm install -g opencode-ai` (inclui modelos gratuitos; também roda modelos locais) |

Também abrem como aba, com menos integração: Aider, Goose, GitHub Copilot CLI, Amazon Q, Open Interpreter e o terminal
comum do sistema.

## Instalar e rodar

Não há instalador ainda: o app roda da pasta onde foi compilado.

### Windows

Pré-requisito: [Rust](https://rustup.rs) 1.89 ou mais novo.

```powershell
git clone https://github.com/pattrickn0/AgentDeck.git
cd AgentDeck
cargo build --workspace --release
.\target\release\agentdeck.exe
```

### Linux

Pré-requisitos: [Rust](https://rustup.rs) 1.89 ou mais novo e as bibliotecas de som e teclado.

```bash
# Ubuntu / Debian
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev
# Fedora
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel

git clone https://github.com/pattrickn0/AgentDeck.git
cd AgentDeck
cargo build --workspace --release
./target/release/agentdeck
```

- Para abrir pastas e links e escolher arquivos: `xdg-utils` e `xdg-desktop-portal` com um backend
  (`xdg-desktop-portal-gtk` no GNOME/XFCE, `xdg-desktop-portal-kde` no KDE).
- Sem som nem microfone (dispensa a ALSA): `cargo build -p agentdeck --no-default-features --release`.
- Testado em X11 (Ubuntu); o Fedora é conferido na CI, e no WSLg o app abre pelo X11 sozinho. Wayland ainda não foi
  validado a fundo: se a janela não abrir, use `WINIT_UNIX_BACKEND=x11`. Detalhes e limitações em
  [memory/LINUX.md](memory/LINUX.md).

### Os dois executáveis

O build gera o `agentdeck` (o app) e o `ask-agent` (o comando que os agentes usam). Eles precisam ficar **na mesma
pasta**. Dentro das abas o app já põe o `ask-agent` no `PATH`.

## Primeiros passos

1. Abra o app e clique em **Abrir pasta…** para criar o primeiro projeto (a pasta do seu código).
2. Em **+ Novo agente**, na barra lateral, abra um Claude, um Codex ou outro CLI instalado. No Claude, no Antigravity e
   no OpenCode, a seta `›` escolhe o modelo.
3. Converse com o agente no terminal da aba. Para ele chamar outro agente, basta pedir: ele sabe usar o `ask-agent`.
4. Para um trabalho maior, use **+ Nova tarefa** na aba Tarefas: escreva o pedido, escolha o nível e deixe a mesa
   escolher o agente. Quando ele entregar, revise o diff e mescle.

## `ask-agent`: a linha de comando dos agentes

Os agentes usam estes comandos sozinhos (as instruções chegam a cada aba). Você também pode rodá-los numa aba de
terminal comum.

```bash
# Conversar
ask-agent codex "Revise a função parse_config em src/config.rs"
ask-agent --model sonnet claude@dev "Implemente o item 3"   # abre claude@dev com o modelo sonnet
ask-agent --reply 7 "Feito: a pasta src/ foi dividida"      # responde à pergunta nº 7
ask-agent --boss "Terminei a revisão"                       # reporta ao chefe
ask-agent --inbox                                           # respostas que chegaram depois

# Quem está aí
ask-agent --list                                            # agentes e instâncias abertas
ask-agent --team                                            # seu chefe e sua equipe
ask-agent --models                                          # modelos de cada agente, cota e capacidade

# Tarefas
ask-agent --task new "Tela de login" --level complexa --scope src/ui --start --file pedido.md
ask-agent --task list
ask-agent --task merge t3                                   # mescla e deixa aguardando validação
ask-agent --task validate t3                                # conclui e limpa a worktree

# Memória do projeto
ask-agent --learn src/config.rs                             # o que o projeto já aprendeu sobre esse arquivo
ask-agent --outline src/config.rs                           # as assinaturas com a linha, para ler só o trecho certo
```

Texto com acento vai melhor por arquivo (`--file`): no Windows, a entrada padrão do PowerShell troca acentos por `?`.
A referência completa está em [memory/GERAL.md](memory/GERAL.md#2-o-que-dá-para-fazer).

## Atalhos

| Atalho | Ação |
| :-- | :-- |
| `Ctrl+T` | nova aba do tipo focado |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | próxima / anterior aba da janela |
| `Ctrl+Alt+setas` | foco na janela vizinha |
| `Ctrl+B` | mostra ou esconde a barra lateral |
| `Ctrl+Shift+D` | alterna o centro entre Agentes e Documentos |
| `Ctrl+Shift+M` | ditado na aba visível |
| `Ctrl+S` / `Ctrl+F` | grava / busca no documento aberto |
| `Ctrl+/` | lista os atalhos |

Com o terminal focado, `Ctrl+B` vai para o programa da aba (útil no tmux). A opção "Atalhos do app acima do terminal",
no menu ☰, inverte isso.

## Dados e privacidade

- **Nada passa por um servidor do AgentDeck**, porque ele não tem um. Quem fala com a nuvem são os próprios CLIs; a
  leitura de cota usa as credenciais que eles já guardam na máquina.
- A comunicação entre agentes é uma ponte HTTP local, só em `127.0.0.1`, com um token por aba: um agente não consegue se
  passar por outro.
- O áudio do ditado é apagado logo depois da transcrição. Anexos e prompts longos ficam só das últimas 5 sessões.
- Abrir uma pasta como projeto conta como confiar nela: o app a registra como confiável no Claude Code
  (`~/.claude.json`) e no Antigravity (`~/.gemini/antigravity-cli/settings.json`). Sua pasta pessoal e a raiz de um
  disco nunca são registradas.
- Os dados de execução ficam em `dados/`, ao lado do executável (ou em `AGENTDECK_HOME`). A configuração fica em
  `%APPDATA%\AgentDeck\config.json` no Windows e em `~/.config/AgentDeck/config.json` no Linux.

## Documentação

A documentação do projeto mora em [`memory/`](memory/GERAL.md), versionada junto com o código:

- [GERAL.md](memory/GERAL.md): o ponto de partida, com como o app funciona, o que dá para fazer e o mapa dos documentos.
- [ARCHITECTURE.md](memory/ARCHITECTURE.md): crates, fluxos e o contrato da ponte entre agentes.
- [MANUTENCAO.md](memory/MANUTENCAO.md): como compilar, testar e manter, com uma introdução prática ao Rust.
- [AGENTDECK.md](memory/AGENTDECK.md): visão e rumo do produto.
- [pendencias.md](memory/pendencias.md) e [historico.md](memory/historico.md): o que vem a seguir e o que já foi feito.
- [aprendizados/](memory/aprendizados/README.md): as armadilhas e decisões que o código não mostra.

## Contribuir

Veja [CONTRIBUTING.md](CONTRIBUTING.md). Antes de enviar, rode `cargo fmt --all`,
`cargo clippy --workspace --all-targets -- -D warnings` e `cargo test --workspace`; a CI confere os três em Windows,
Ubuntu e Fedora.

## Licença

MIT. Veja [LICENSE](LICENSE).

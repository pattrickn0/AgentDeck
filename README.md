# ⚡ AgentDeck

Terminal desktop nativo (Rust + winit) que conecta agentes de IA: **Claude Code**, **Antigravity CLI**,
**Codex**, **OpenCode**, **modelos locais via Ollama** e outros CLIs. Os agentes conversam entre si (`ask-agent`) e
trabalham na mesma pasta, enquanto você acompanha tudo na **Conversa integrada**.

![Tela principal](docs/screenshots/main.png)

## Funções

- Abas de terminal reais (ConPTY no Windows) para cada agente, com cores próprias, restauradas ao reabrir. Cada aba é uma
  instância (`claude`, `claude@dev`…) e tem o **modelo de LLM** dela, mostrado ao lado do nome.
- **Mesa de tarefas** (aba Tarefas da barra direita): dê um pedido a um agente e ele trabalha numa branch e numa pasta
  (worktree) só dele; você revisa o relatório e o diff e **mescla**, **devolve** com ajustes ou **descarta**; depois de
  mesclar, a tarefa espera a sua **validação** (**validar** ou **reprovar**, e ela volta ao mesmo agente).
- **Memória do projeto**: o que o projeto já aprendeu (`memory/aprendizados/`, versionado) e a situação atual (as outras
  tarefas em andamento e os últimos commits) chegam ao agente no início de cada tarefa e em cada devolução; o agente
  também consulta com `ask-agent --learn`.
- **Tela dividida por tipo de agente**: cada tipo (Claude, Antigravity, Ollama…) ganha uma janela com as próprias
  abas, numa grade automática com divisores arrastáveis; dá para trocar janelas de lugar ou desligar a divisão.
- **Conversa integrada** numa barra fixa à direita: perguntas e respostas entre agentes em tempo real (somente leitura).
  Quando as abas de uma janela não cabem, setas ‹ › e o menu ▾ levam às escondidas.
- **Git** do projeto numa aba da barra direita: alterações, diff, commit, branches, buscar/puxar/enviar.
- **GitHub** do projeto noutra aba: PRs abertos com os checks, issues, execuções do Actions e o fim do log do job que
  falhou; um clique manda o PR ou a falha para um agente.
- Explorador de arquivos do workspace: clique insere o caminho do arquivo na aba ativa, duplo clique abre o arquivo (texto ou markdown, só leitura) no modo **Documentos** do centro, onde também se leem, com a largura toda, o diff de uma tarefa, de um commit ou de um arquivo e o pedido e o relatório das tarefas.
- Anexos por botão no cabeçalho de cada janela de agente (e no formulário "Nova tarefa"), arrastar e soltar sobre a janela/formulário ou colar imagem (Ctrl+V).
- Ditado por voz **opcional**, com **whisper.cpp** local (CUDA / Vulkan / CPU) no cabeçalho de cada janela de agente (e `Ctrl+Shift+M`), preso à aba visível daquela janela (com indicador de gravação/transcrição na aba); o áudio é apagado logo após transcrever. Na primeira abertura o app **pergunta** se você quer o ditado e avisa do download (cerca de 1,6 GB); "Agora não" não baixa nada, e dá para baixar ou remover depois em Configurações (☰).
- Agente **Ollama** autônomo com ferramentas no workspace (ler/escrever/editar arquivos, rodar comandos).
- Som de notificação quando um agente termina.

Atalhos: `Ctrl+T` nova aba · `Ctrl+Tab` / `Ctrl+Shift+Tab` alternar · `Ctrl+B` barra lateral ·
`Ctrl+Shift+M` ditar na aba · `Ctrl+Shift+D` alterna o centro entre Agentes e Documentos · `Ctrl+/` lista os atalhos.
Com o terminal focado, `Ctrl+B` vai para o terminal (tmux); a opção
"Atalhos do app acima do terminal" (☰) o devolve ao app.

## Compilar e rodar (Windows)

Pré-requisito: [Rust](https://rustup.rs) (stable).

```powershell
cargo build --workspace --release
.\target\release\agentdeck.exe
```

Os três executáveis (`agentdeck`, `ask-agent`, `ollama-agent`) precisam ficar **na mesma pasta**.
As pastas `dados/`, `workspace/` e `whisper/` são procuradas subindo a partir do executável
(ou defina `AGENTDECK_HOME`). Configuração em `%APPDATA%\AgentDeck\config.json`.

CLIs suportados (instale os que quiser usar):
- Claude Code: `npm install -g @anthropic-ai/claude-code`
- Antigravity: `irm https://antigravity.google/cli/install.ps1 | iex`
- Codex: `npm install -g @openai/codex`
- OpenCode: `npm install -g opencode-ai`
- Ollama: https://ollama.com/download

## Compilar e rodar (Linux)

Pré-requisito: [Rust](https://rustup.rs) 1.89 ou mais novo e as bibliotecas de sistema abaixo.

```bash
# Ubuntu / Debian
sudo apt install build-essential pkg-config libasound2-dev libxkbcommon-dev
# Fedora
sudo dnf install gcc gcc-c++ make pkgconf-pkg-config alsa-lib-devel libxkbcommon-devel

cargo build --workspace --release
./target/release/agentdeck
```

Não há pacote nem instalador: o app roda da pasta onde foi compilado (ou de uma pasta portátil para onde você copie
o executável e as pastas dele).

- Sem som nem microfone (dispensa a ALSA): `cargo build -p agentdeck --no-default-features --release`.
- Para abrir pastas e links e escolher arquivos, o sistema precisa do `xdg-utils` (`xdg-open`), do `xdg-desktop-portal`
  e de um backend (`xdg-desktop-portal-gtk` no GNOME/XFCE, `xdg-desktop-portal-kde` no KDE). Sem o portal, os
  diálogos de arquivo não abrem.
- Os três executáveis ficam juntos em `target/release/`; para usar o `ask-agent` fora do app, ponha essa pasta no
  `PATH` (dentro das abas o app já resolve).
- Os dados (`dados/`, `workspace/` e `whisper/`) ficam ao lado do executável, como no Windows, ou em
  `AGENTDECK_HOME`. Se essa pasta não for gravável (app instalado em `/usr/local/bin`, por exemplo), vão para
  `~/.local/share/AgentDeck`. Configuração em `~/.config/AgentDeck/config.json`.
- Ditado (opcional, o app pergunta antes de baixar): o motor de CPU e o modelo são baixados quando você aceita (os binários do Linux pedem `libgomp` e `libstdc++`, que
  já vêm nas instalações desktop). Para usar a GPU, deixe no `PATH` um `whisper-cli` compilado com CUDA ou Vulkan;
  o app não baixa esses motores no Linux. `pciutils` (`lspci`) e `nvidia-smi` ajudam a detectar a GPU.
- CLIs: os mesmos `npm install -g …` acima; Antigravity e Ollama têm instalador de Linux nos sites
  (https://antigravity.google, https://ollama.com/download).
- Testado em X11 (Ubuntu). Wayland é suportado pela biblioteca de janelas, mas ainda não foi validado a fundo; se a
  janela não abrir, force o X11 com `WINIT_UNIX_BACKEND=x11 ./target/release/agentdeck`.
- O Fedora é verificado na CI (build e testes num contêiner). O roteiro de verificação e as limitações conhecidas
  estão em [`memory/LINUX.md`](memory/LINUX.md).

## Comunicação entre agentes

Dentro de qualquer aba:

```bash
ask-agent antigravity "Revise a estrutura de pastas proposta"
ask-agent --reply 7 "Feito: a pasta src/ foi dividida"   # responde à pergunta nº 7
ask-agent --model sonnet claude@dev "Implemente a tarefa"   # abre claude@dev com o modelo sonnet
ask-agent --list                                        # agentes e instâncias abertas, com o modelo
ask-agent --models                                      # modelos que cada agente instalado aceita
ask-agent --team                                        # seu chefe e sua equipe
ask-agent --boss "Terminei a revisão"                   # reporta ao chefe
cat relatorio.txt | ask-agent claude -

# Tarefas (cada uma numa branch e pasta próprias; o pedido e o relatório vão num arquivo, que preserva os acentos)
ask-agent --task new "Corrigir o login" --agent claude --model sonnet --effort high --start --file pedido.md
ask-agent --task list                                   # as tarefas do projeto, com estado e agente
ask-agent --task done t3 --file relatorio.md            # (o agente da tarefa) entrega o relatório
ask-agent --task merge t3                               # mescla e deixa aguardando validação
ask-agent --task validate t3                            # conclui e limpa; ou: --task reject t3 --file motivo.md

# Memória do projeto
ask-agent --learn                                       # aprendizados globais e o índice dos demais
ask-agent --learn crates/core/src/tasks.rs              # o que o projeto já aprendeu sobre esse arquivo
```

O `ask-agent` conversa com **abas de verdade**: a mensagem é digitada no terminal do agente e você vê o
raciocínio dele, podendo intervir. Se o agente não tem aba, ela é aberta em segundo plano. Cada aba é
uma instância com nome `modelo[@cargo[@nome]]` (`claude`, `claude@dev`, `claude@dev@ana`), então dá para
ter várias do mesmo modelo e delegar por cargo: `ask-agent claude@dev "tarefa"` (use `--new` para outra
instância). Cada instância também tem o **modelo de LLM** dela (`--model sonnet`, ou o padrão de `roleModels` na
configuração), mostrado ao lado do nome na aba e na barra lateral. Se o outro agente demorar mais de 90 s (`AGENTDECK_ASK_WAIT`), o `ask-agent` termina avisando e a
resposta é digitada depois no terminal de quem perguntou. Dá para desligar em Configurações.

Detalhes do protocolo e da arquitetura em [memory/ARCHITECTURE.md](memory/ARCHITECTURE.md).

## Documentação

A documentação do projeto fica em **[memory/](memory/GERAL.md)**. Comece por **[memory/GERAL.md](memory/GERAL.md)**:
como o AgentDeck funciona por dentro, o que dá para fazer nele e quais regras cada documento define.

- [memory/ARCHITECTURE.md](memory/ARCHITECTURE.md): arquitetura e protocolo entre agentes.
- [memory/MANUTENCAO.md](memory/MANUTENCAO.md): guia de manutenção, com introdução prática ao Rust.
- [memory/AGENTDECK.md](memory/AGENTDECK.md): para onde o produto vai e quem decide.
- [memory/pendencias.md](memory/pendencias.md): o que falta desenvolver, na ordem em que será feito.
- [memory/historico.md](memory/historico.md): o que já foi feito.
- [memory/testes.md](memory/testes.md): roteiro de testes manuais (Windows, agentes reais).
- [CONTRIBUTING.md](CONTRIBUTING.md): como contribuir.

## Licença

MIT — veja [LICENSE](LICENSE).

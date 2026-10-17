# ⚡ CommandDeck Terminal

Terminal desktop nativo (Rust + winit) que conecta agentes de IA: **Claude Code**, **Antigravity CLI**,
**Codex**, **OpenCode**, **modelos locais via Ollama** e outros CLIs. Os agentes conversam entre si (`ask-agent`) e
trabalham na mesma pasta, enquanto você acompanha tudo na **Conversa integrada**.

![Tela principal](docs/screenshots/main.png)

## Funções

- Abas de terminal reais (ConPTY no Windows) para cada agente, com cores próprias, restauradas ao reabrir.
- **Tela dividida por tipo de agente**: cada tipo (Claude, Antigravity, Ollama…) ganha uma janela com as próprias
  abas, numa grade automática com divisores arrastáveis; dá para trocar janelas de lugar ou desligar a divisão.
- **Conversa integrada** numa barra fixa à direita: perguntas e respostas entre agentes em tempo real, e o
  **Mapa** (em texto de terminal) de quem está conversando com quem agora.
- **Compositor** na barra direita: a mensagem (com anexos e áudio transcrito) vai para a **aba ativa**.
- **Git** do projeto numa aba da barra direita: alterações, diff, commit, branches, buscar/puxar/enviar.
- **GitHub** do projeto noutra aba: PRs abertos com os checks, issues, execuções do Actions e o fim do log do job que
  falhou; um clique manda o PR ou a falha para um agente.
- Explorador de arquivos do workspace: clique insere `@arquivo` na mensagem, duplo clique abre no sistema.
- Anexos por botão, arrastar e soltar ou colar imagem (Ctrl+V).
- Ditado por voz com **whisper.cpp** local (CUDA / Vulkan / CPU); o áudio é apagado logo após transcrever.
- Agente **Ollama** autônomo com ferramentas no workspace (ler/escrever/editar arquivos, rodar comandos).
- Som de notificação quando um agente termina.

Atalhos: `Ctrl+T` nova aba · `Ctrl+Tab` / `Ctrl+Shift+Tab` alternar · `Ctrl+B` barra lateral ·
`Ctrl+L` focar mensagem · `Enter` envia · `Shift+Enter` quebra linha · `Ctrl+/` lista os atalhos.
Com o terminal focado, `Ctrl+B` e `Ctrl+L` vão para o terminal (tmux, limpar a tela); a opção
"Atalhos do app acima do terminal" (☰) os devolve ao app.

## Compilar e rodar (Windows)

Pré-requisito: [Rust](https://rustup.rs) (stable).

```powershell
cargo build --workspace --release
.\target\release\commanddeck.exe
```

Os três executáveis (`commanddeck`, `ask-agent`, `ollama-agent`) precisam ficar **na mesma pasta**.
As pastas `dados/`, `workspace/` e `whisper/` são procuradas subindo a partir do executável
(ou defina `COMMANDDECK_HOME`). Configuração em `%APPDATA%\CommandDeck\config.json`.

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
sudo apt install build-essential pkg-config libasound2-dev libgtk-3-dev libxkbcommon-dev libxkbcommon-x11-0 libgl1
# Fedora
sudo dnf install gcc pkgconf-pkg-config alsa-lib-devel gtk3-devel libxkbcommon-devel libxkbcommon-x11 mesa-libGL

cargo build --workspace --release
./target/release/commanddeck
```

- Sem som nem microfone (dispensa a ALSA): `cargo build -p commanddeck --no-default-features --release`.
- Os três executáveis ficam juntos em `target/release/`; para usar o `ask-agent` fora do app, ponha essa pasta no
  `PATH` (dentro das abas o app já resolve). As pastas `dados/`, `workspace/` e `whisper/` seguem a mesma regra do
  Windows (ou `COMMANDDECK_HOME`). Configuração em `~/.config/CommandDeck/config.json`.
- Ditado: o motor de CPU e o modelo são baixados sozinhos; para usar a GPU, deixe no `PATH` um `whisper-cli`
  compilado com CUDA ou Vulkan.
- CLIs: os mesmos `npm install -g …` acima; Antigravity e Ollama têm instalador de Linux nos sites
  (https://antigravity.google, https://ollama.com/download).
- Testado em X11 (Ubuntu). Wayland é suportado pela biblioteca de janelas, mas ainda não foi testado.

## Comunicação entre agentes

Dentro de qualquer aba:

```bash
ask-agent antigravity "Revise a estrutura de pastas proposta"
ask-agent --reply 7 "Feito: a pasta src/ foi dividida"   # responde à pergunta nº 7
ask-agent --model sonnet claude@dev "Implemente a tarefa"   # abre claude@dev com o modelo sonnet
ask-agent --list                                        # agentes e instâncias abertas, com o modelo
ask-agent --team                                        # seu chefe e sua equipe
ask-agent --boss "Terminei a revisão"                   # reporta ao chefe
cat relatorio.txt | ask-agent claude -
```

O `ask-agent` conversa com **abas de verdade**: a mensagem é digitada no terminal do agente e você vê o
raciocínio dele, podendo intervir. Se o agente não tem aba, ela é aberta em segundo plano. Cada aba é
uma instância com nome `modelo[@cargo[@nome]]` (`claude`, `claude@dev`, `claude@dev@ana`), então dá para
ter várias do mesmo modelo e delegar por cargo: `ask-agent claude@dev "tarefa"` (use `--new` para outra
instância). Cada instância também tem o **modelo de LLM** dela (`--model sonnet`, ou o padrão de `roleModels` na
configuração), mostrado ao lado do nome na aba e na barra lateral. Se o outro agente demorar mais de 90 s (`COMMANDDECK_ASK_WAIT`), o `ask-agent` termina avisando e a
resposta é digitada depois no terminal de quem perguntou. Dá para desligar em Configurações.

Detalhes do protocolo e da arquitetura em [memory/ARCHITECTURE.md](memory/ARCHITECTURE.md).

## Documentação

A documentação do projeto fica em **[memory/](memory/GERAL.md)**. Comece por **[memory/GERAL.md](memory/GERAL.md)**:
como o CommandDeck funciona por dentro, o que dá para fazer nele e quais regras cada documento define.

- [memory/ARCHITECTURE.md](memory/ARCHITECTURE.md): arquitetura e protocolo entre agentes.
- [memory/MANUTENCAO.md](memory/MANUTENCAO.md): guia de manutenção, com introdução prática ao Rust.
- [memory/pendencias.md](memory/pendencias.md): o que falta desenvolver e o que já foi concluído.
- [memory/testes.md](memory/testes.md): roteiro de testes manuais (Windows, agentes reais).
- [CONTRIBUTING.md](CONTRIBUTING.md): como contribuir.

## Licença

MIT — veja [LICENSE](LICENSE).

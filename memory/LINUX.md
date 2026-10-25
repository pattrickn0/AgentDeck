# Suporte a Linux (Ubuntu e Fedora)

Verificação feita nas tarefas `t29` (2026-10-22) e `t33` (2026-10-23). Este documento diz **o que foi conferido, como**,
o que foi corrigido, as dependências de cada distro e o que continua sem validação. O roteiro manual de Windows
continua em [testes.md](testes.md); o de Linux está aqui no fim.

> **Resumo:** o app compila e passa os testes no Ubuntu (CI) e agora também no Fedora (job em contêiner). A interface
> foi validada à mão só no X11 do Ubuntu; Wayland e um desktop Fedora de verdade ainda não foram testados. Não há
> pacote, instalador nem arquivo `.desktop`. A cota do Antigravity passou a ler o chaveiro do sistema (t33) e, sob o
> WSLg, o app abre pelo X11 (t33) — nenhum dos dois foi exercitado numa máquina Linux de verdade.

## 1. Como esta verificação foi feita

Não havia máquina Linux, WSL nem Docker nesta máquina, então a verificação foi:

1. **CI do GitHub** (repositório `pattrickn0/AgentDeck`), lendo os logs dos jobs do Ubuntu e do Windows pelo endpoint
   `/actions/jobs/<id>/logs`, mais a API de execuções.
2. **Revisão do código** de tudo que tem caminho por plataforma: `process.rs`, `paths.rs`, `agents.rs`, `git.rs`,
   `github.rs`, `workspace.rs`, `whisper.rs`, `download.rs`, `usage/*`, `theme.rs`, `terminal/*`, `ui/projects.rs`,
   `main.rs`, `crash.rs` e os testes com `#[cfg(unix)]`.
3. **Conferência dos artefatos baixados**: nomes e SHA-256 dos pacotes do whisper.cpp no release `b5454` batem com as
   constantes de `whisper.rs` (Ubuntu x64 e arm64).
4. **Revisão das dependências** em `Cargo.lock`: não há `gtk-sys`/`glib-sys` (o `rfd 0.17` usa
   `xdg-portal` + `wayland`), e o `ureq` usa rustls (sem OpenSSL).

`cargo check --target x86_64-unknown-linux-gnu` chegou a ser tentado, mas para no `ring` porque não há toolchain
`x86_64-linux-gnu-gcc` nesta máquina — por isso o build de Linux é verificado só pela CI.

A cota do Antigravity e o WSLg (seções 2.7 e 2.8) foram verificados por revisão de código e pelas fontes
(`zalando/go-keyring`, issues do `winit` e do WSLg). O módulo do chaveiro compila para `x86_64-unknown-linux-gnu`
numa crate separada (como em `t29`, o workspace para no `ring` por falta do gcc cruzado) e o trecho que força o X11
foi conferido do mesmo jeito; mas não houve Linux nem WSL para abrir o app.

## 2. O que estava quebrado e foi corrigido

### 2.1 O teste que falhava só no Linux (desde 17/10)

Os jobs do Ubuntu (`runs 42–46`) falhavam em `workspace::tests::read_dir_orders_and_filters`, sempre com
`index out of bounds: the len is 0 but the index is 0` em `workspace.rs`. A linha era:

```rust
let sub_bs = read_dir(ws, "zeta\\");
assert_eq!(sub_bs[0].rel_path, "zeta/in.json");   // panic no Linux
```

`read_dir` validava e juntava o caminho **antes** de trocar `\` por `/`. No Windows `zeta\` é o diretório `zeta`; no
Unix a barra invertida faz parte do nome, então `ws.join("zeta\\")` procura um arquivo chamado `zeta\` e devolve
vazio. Correção em `crates/core/src/workspace.rs`: normalizar o separador no começo de `read_dir`. O teste continua
valendo (e passa) no Windows. Ver
[aprendizados/caminho-com-barra-invertida-no-read-dir.md](aprendizados/caminho-com-barra-invertida-no-read-dir.md).

### 2.2 A CI parou de validar o Linux (de 22/10 até a dívida de formatação ser paga)

A partir do `run 47` (commit `146a486`), o passo **Formatação** (`cargo fmt --all -- --check`) falhava no Windows **e**
no Ubuntu, e os passos de build, teste e clippy nem rodavam. A dívida foi paga depois: em 2026-10-24 a CI passou inteira
nos três jobs (Windows, Ubuntu, Fedora), com a formatação.

### 2.3 Pasta de dados em instalação de sistema

`AppPaths::detect()` usava sempre a pasta do executável para `dados/`, `workspace/` e `whisper/`. Copiar o binário
para `/usr/local/bin` (pasta do root) fazia o app falhar ao abrir a trava (`dados/.agentdeck.lock` → permissão
negada). Agora, no Unix, se a pasta não for gravável (`access(2)`), os dados vão para `~/.local/share/AgentDeck`
(padrão XDG). Pasta portátil e repositório em desenvolvimento continuam como antes, e `AGENTDECK_HOME` continua
mandando. Correção em `crates/core/src/paths.rs`, com teste em `#[cfg(unix)]`.

### 2.4 Referência a um script que não existe

A mensagem de erro do ditado mandava compilar o whisper.cpp com `scripts/build-whisper-linux.sh`, que não existe no
repositório. A mensagem agora diz o que fazer de verdade (compilar com Vulkan/CUDA e pôr o `whisper-cli` no `PATH`, ou
usar o motor de CPU).

### 2.5 Fontes do terminal no Fedora

Os caminhos de fonte do Linux só cobriam o layout do Debian/Ubuntu (`/usr/share/fonts/truetype/...`). O Fedora instala
cada família na própria pasta (`/usr/share/fonts/dejavu-sans-mono-fonts/...`), então a lista não achava nada e o
terminal caía na fonte embutida do egui. `theme.rs` ganhou os caminhos do Fedora (a interface já usa Inter/IBM Plex
embutidas; isso vale para o terminal e para os símbolos).

### 2.6 Documentação e CI

- `README.md` e `CONTRIBUTING.md`: dependências corrigidas (sem GTK3, que não é usado), `xdg-utils`/portal, `libgomp`,
  pasta de dados, URL do repositório novo (`AgentDeck`).
- `.github/workflows/ci.yml`: saiu o `libgtk-3-dev`; entrou um job **`fedora`** (`container: fedora:latest`) que
  instala as dependências do README e roda `cargo build --workspace` e `cargo test --workspace`.

### 2.7 Cota do Antigravity pelo chaveiro do sistema (t33)

No Linux o `agy` não guarda o token em `~/.gemini/oauth_creds.json`: ele usa o **Secret Service** (o chaveiro do
GNOME/KDE, por D-Bus), com `service=gemini`, `username=antigravity` e o mesmo JSON do Gerenciador de Credenciais
do Windows. O app só olhava o arquivo, então a cota ficava "sem dados". `crates/core/src/usage/antigravity.rs`
ganhou a leitura do chaveiro com a crate `secret-service` (Rust puro, por `zbus`; nenhuma biblioteca C nova na
CI), numa thread própria com prazo de 3 s e só para itens já destravados — destravar abriria um pedido de senha
em segundo plano. Sem sessão D-Bus (SSH, contêiner) a busca falha em silêncio e o app cai nos arquivos como
antes; a leitura só acontece quando a home usada é a do sistema, como já era no Windows. Entrou também o arquivo
que o `agy` grava quando não há chaveiro (`GEMINI_FORCE_FILE_STORAGE=true`, o caminho de SSH, contêiner e WSL):
`~/.gemini/antigravity-cli/antigravity-oauth-token`, depois do `~/.gemini/oauth_creds.json` na ordem das fontes.
Ver [aprendizados/antigravity-token-no-secret-service.md](aprendizados/antigravity-token-no-secret-service.md).

Para conferir: `secret-tool lookup service gemini username antigravity` (pacote `libsecret-tools` no Ubuntu,
`libsecret` no Fedora) imprime o JSON, e pede para destravar o chaveiro se ele estiver trancado. Com o token válido,
a cota aparece na próxima consulta (a cada 3 min, ou em 20 s depois de uma falha). Se não aparecer, rode com
`RUST_LOG=debug`: o `agentdeck.log` registra quando o chaveiro não devolveu o token.

### 2.8 WSLg: abrir pelo X11 (t33)

Sob o WSLg o desktop é um Weston 9 (Wayland) com um XWayland ao lado. O cliente Wayland do Weston 9 quebra
janelas que desenham a própria decoração e redimensionam, e o Weston já caiu com clientes Wayland (`winit#4306`,
`microsoft/wslg#1008` e `#633`). O app agora detecta o WSLg (`WSL_DISTRO_NAME` ou "microsoft" no
`/proc/sys/kernel/osrelease`, mais a pasta `/mnt/wslg`) e força o backend X11 do `winit`
(`crates/app/src/wslg.rs`, com teste da função pura; `main.rs`). Fora do WSLg nada muda, e um
`WINIT_UNIX_BACKEND` definido pelo usuário continua mandando.

O que o app não resolve, e por que o README manda usar o app nativo no Windows:

- **Escala sempre 1**: no X11 a escala vem do XSettings (`Xft/DPI`), que o WSLg não publica; em tela de alta
  densidade a interface fica pequena. O `winit` 0.30 não tem mais `WINIT_X11_SCALE_FACTOR` (foi removido), então
  não há variável de ambiente para contornar — mudar isso exige tratar a escala no app.
- **Janela maximizada sem decoração**: já apareceu deslocada em 32 px no WSLg; sem uma máquina Windows com WSLg,
  não dá para confirmar nem corrigir.
- **Redimensionamento e cursor**: são justamente os pontos que quebram no Wayland do WSLg; no X11 devem funcionar,
  mas ninguém abriu o app lá.

Ver [aprendizados/wslg-forca-x11.md](aprendizados/wslg-forca-x11.md).

## 3. Dependências

### 3.1 Para compilar

| | Ubuntu 24.04+ | Fedora 41+ |
| :-- | :-- | :-- |
| Base | `build-essential pkg-config` | `gcc gcc-c++ make pkgconf-pkg-config` |
| Áudio (padrão) | `libasound2-dev` | `alsa-lib-devel` |
| Janela/teclado | `libxkbcommon-dev` | `libxkbcommon-devel` |
| Rust | 1.89+ (`rustup`) | 1.89+ (`rustup`) |

Sem áudio: `cargo build -p agentdeck --no-default-features` dispensa a ALSA (fica sem som e sem microfone).

Não precisa de: GTK3/GTK4 (`rfd` usa o portal), OpenSSL (`ureq` com rustls), X11/Wayland de desenvolvimento (o winit
carrega as bibliotecas em tempo de execução). O `libxkbcommon` não é exigido no link, mas é o que a CI instala e o que
o teclado usa em Wayland.

### 3.2 Para rodar

| Recurso | Ubuntu | Fedora | Sem isso |
| :-- | :-- | :-- | :-- |
| Janela | X11 ou Wayland + driver GL (`mesa`) | idem (`mesa-libGL`) | não abre |
| Abrir pasta / links | `xdg-utils` (`xdg-open`) | `xdg-utils` | falha silenciosa |
| Diálogo de arquivo | `xdg-desktop-portal` + `xdg-desktop-portal-gtk` | idem | o diálogo não abre |
| Fonte do terminal | `fonts-dejavu-core` (ou Liberation/Noto) | `dejavu-sans-mono-fonts` | cai na fonte embutida do egui |
| Chaveiro (cota do Antigravity) | Secret Service (`gnome-keyring`/`kwallet`) | idem | a cota do Antigravity fica sem dados |
| Git / GitHub | `git` (e `gh` opcional) | `git` (`gh` opcional) | abas Git/GitHub sem função |
| Ditado (CPU) | `libgomp`, `libstdc++` | `libgomp`, `libstdc++` | o `whisper-cli` baixado não roda |
| Detectar GPU | `pciutils` (`lspci`), `nvidia-smi` | idem | cai no motor de CPU |

CLIs de agente: `npm install -g` dos mesmos pacotes do Windows; o Antigravity tem instalador próprio. O app
procura no `PATH`, em `~/.local/bin`, em `~/.claude/local/claude` e em `~/.antigravity/bin/agy`.

## 4. O que está coberto pelo código e pelos testes

| Área | Como está no Linux |
| :-- | :-- |
| Processos | grupo próprio por filho (`process_group(0)`) e `kill(-pid)` na árvore; testes `cfg(unix)` em `process.rs` |
| PTY/terminal | `portable-pty` + `alacritty_terminal`; `TERM=xterm-256color`, `COLORTERM=truecolor` |
| Aba Terminal | `$SHELL` ou `/bin/bash`; sem shims `.cmd` (isso é só Windows) |
| Ambiente das abas | `PATH` com a pasta de binários do app na frente; variáveis `AGENTDECK_*`; sem `PYTHONUTF8`/MSYS |
| Instância única | trava de arquivo em `dados/.agentdeck.lock` |
| Git | `git` do `PATH`, sem shell, `LC_ALL=C`, `GIT_TERMINAL_PROMPT=0`, SSH em `BatchMode` |
| GitHub | token por `git credential fill` e, se houver, `gh auth token`; sem gravar em disco |
| Agentes | resolução por `which`/pastas do usuário; sem `cmd.exe` |
| Documentos/workspace | `workspace::read_dir` normaliza `/` e `\`; `read_text` recusa link que sai do projeto |
| Whisper (CPU) | baixa `whisper-bin-ubuntu-x64.tar.gz` (ou arm64), `chmod 0o755`, `LD_LIBRARY_PATH` com a pasta do motor |
| Uso/cotas | Antigravity lê o Secret Service (`service=gemini`, `username=antigravity`) e, sem ele, `~/.gemini/oauth_creds.json` e `~/.gemini/antigravity-cli/antigravity-oauth-token`; Codex lê `~/.codex/sessions` |
| Interface | fontes embutidas; `xdg-open` para pastas; `open` (crate) para URLs |

A suíte tem 196 testes no app (mais 1 ignorado, o de cota real), 253 no core (mais 2 ignorados), 31 da ponte e 37 das
tarefas, além dos de git/github; rodada localmente no Windows em 23/10, todos passam. No Linux, o único que falhava era
o de `read_dir` (corrigido aqui); os demais rodaram
verdes no Ubuntu até o `run 46`.

## 5. O que continua sem verificação (ou sem suporte)

1. **Wayland**: a winit suporta, e o `rfd` também, mas ninguém abriu o app numa sessão Wayland. Testar janela, arrastar
   divisórias, colar, IME e diálogo de arquivo. Contorno: `WINIT_UNIX_BACKEND=x11`.
2. **Desktop Fedora de verdade**: o job novo prova build e testes, não a janela. Falta abrir o app num Fedora
   Workstation (GNOME/Wayland) e repetir o roteiro do fim deste documento.
3. **Empacotamento**: não há `.deb`, `.rpm`, Flatpak, AppImage, `.desktop` nem ícone instalado. O app roda da pasta
   compilada; para o menu de aplicativos, é trabalho futuro.
4. **Ditado com GPU**: o app não baixa motores CUDA/Vulkan para Linux (só o de CPU). Quem tem GPU precisa compilar o
   whisper.cpp e deixar o `whisper-cli` no `PATH`; a detecção usa `lspci`/`nvidia-smi`.
5. **Diálogo de arquivo sem portal**: em ambiente mínimo (i3, container, SSH) sem `xdg-desktop-portal` o diálogo não
   abre. Não há aviso na interface; avaliar uma mensagem.
6. **Textos com “Git Credential Manager”**: as mensagens de erro de Git/GitHub foram escritas no Windows e citam GCM e
   “faça um push para entrar”. No Linux o caminho normal é `gh auth login` ou `credential.helper=store`; o texto
   deveria citar os dois.
7. **“Abrir pasta” ignora falha**: `Command::new("xdg-open").spawn()` sem tratar o erro; sem `xdg-utils` o clique não
   faz nada e não avisa.
8. **Versão do runner**: o rótulo `ubuntu-latest` migra para Ubuntu 26 a partir de 19/10/2026; o job pode mudar de
   base sem aviso. Se aparecer dependência nova, conferir o log do passo “Dependências de sistema”.
9. **Formatação**: se `cargo fmt --all -- --check` voltar a falhar, nenhum job chega a rodar os testes — inclusive o
   Fedora. A base toda está no rustfmt: rode `cargo fmt --all` e inclua no commit o que ele mudar (ver
   [aprendizados/base-toda-no-rustfmt.md](aprendizados/base-toda-no-rustfmt.md)).
10. **WSLg**: o X11 foi forçado (seção 2.8), mas ninguém abriu o app dentro de um WSLg. Faltam: janela maximizada
    (deslocamento de 32 px), escala (sempre 1, interface pequena em 4K), arrastar/redimensionar, cursor e a cota do
    Antigravity lida do chaveiro de lá. Um `WINIT_UNIX_BACKEND=wayland` reproduz o caminho quebrado, para comparar.

## 6. Roteiro manual (Ubuntu e Fedora)

Rodar numa pasta de dados separada para não mexer na configuração real:
`AGENTDECK_HOME=/tmp/agentdeck-teste ./target/release/agentdeck`.

**Instalação e abertura**

- [ ] `cargo build --workspace --release` termina sem erro (Ubuntu e Fedora).
- [ ] O app abre em X11 e em Wayland (GNOME e KDE, se possível), com o tamanho de janela e o DPI corretos.
- [ ] O terminal da aba “Terminal” abre o `$SHELL`, aceita acentos e cores, e redimensiona sem quebrar a linha.
- [ ] Fechar o app e abrir de novo mantém projetos, abas e tamanhos; abrir uma segunda instância na mesma pasta avisa
      que já está em uso; com `AGENTDECK_HOME` diferente, abre as duas.

**Agentes e memória**

- [ ] O menu “+ Novo agente” lista os CLIs instalados e o cartão de cada um abre a aba certa.
- [ ] `ask-agent --list`, `--team`, `--models` funcionam no terminal de fora do app.
- [ ] `ask-agent claude "..."` numa aba e a resposta digitada chegam; a Conversa integrada mostra o par.
- [ ] Criar uma tarefa (`--task new ... --start`), ver a worktree, entregar com `--task done` e ler o relatório.
- [ ] `ask-agent --learn` mostra os aprendizados; o `AGENTS.md` do projeto é atualizado só onde já existia.
- [ ] A cota do Antigravity aparece na barra de título com o `agy` logado; `secret-tool lookup service gemini
      username antigravity` devolve o JSON e, com `RUST_LOG=debug`, o `agentdeck.log` não acusa falha ao ler o chaveiro.

**Interface**

- [ ] Barra lateral, barra direita, abas, menus e modais desenham sem texto cortado nem sobreposição.
- [ ] Arrastar divisórias, trocar para ORGANIZAÇÃO/Destaque e alternar Documentos (Ctrl+Shift+D).
- [ ] Abrir uma pasta pelo botão (precisa de `xdg-open`) e escolher um arquivo no diálogo (precisa do portal).
- [ ] Copiar e colar (Ctrl+C/Ctrl+V) entre o app e outro programa; colar uma imagem como anexo.
- [ ] Aba Git (status, diff, histórico, stash) e aba GitHub (PRs, issues, Actions) num repositório real.

**Ditado**

- [ ] Com microfone: segurar o atalho, falar e ver o texto na aba; o `.wav` temporário desaparece de `dados/`.
- [ ] Sem microfone ou sem `libgomp`: o app explica o que falta em vez de travar.

**WSLg (a partir do Windows, com o app compilado no WSL)**

- [ ] `./target/release/agentdeck` abre pelo X11 (o log diz “WSLg detectado”); a janela não pisca, não some e o
      terminal aceita teclado.
- [ ] Maximizar, restaurar, arrastar e redimensionar; o cursor aparece e o texto não fica cortado.
- [ ] Numa tela de alta densidade, anotar a escala (hoje é sempre 1, então a interface fica pequena) e o que falta.
- [ ] `WINIT_UNIX_BACKEND=wayland ./target/release/agentdeck` reproduz a falha do Wayland do WSLg, para comparar.

**Fim**

- [ ] `dados/agentdeck.log` e `dados/crash.log` existem e não têm erro repetido.
- [ ] Ao fechar o app, não sobra processo `whisper-cli` nem agente órfão (`ps -ef | grep -E "whisper|claude"`).

## 7. Manutenção

- Mudou dependência de sistema, caminho de fonte, pasta de dados ou suporte a GPU? Atualize este arquivo e o README no
  mesmo commit.
- O job `fedora` da CI é o mínimo: ele não abre janela. Uma falha só nele costuma ser dependência de build faltando no
  `dnf install`.
- Ao acrescentar um teste que usa caminho, rode a suíte nos dois sentidos: `cargo test --workspace` no Windows **e** no
  Linux (CI). O caso do `read_dir` mostra que um teste pode passar anos só num sistema.

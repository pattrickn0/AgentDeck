# Pendências

Lista do **trabalho de desenvolvimento que falta** (melhorias e limitações conhecidas), por área. O bloco "Próximo"
vem primeiro, na ordem em que será feito; as áreas depois, e os adiados por último. Ao concluir um item, **tire-o
daqui** e acrescente uma linha do que foi feito em [historico.md](historico.md) (regras dos documentos em
[GERAL.md](GERAL.md)). Este arquivo não guarda item concluído.

Os **testes manuais** (validar no Windows, com os agentes reais) ficam em [testes.md](testes.md). Um teste que
falhar vira item aqui. O rumo do produto e quem decide estão em [AGENTDECK.md](AGENTDECK.md).

## Próximo

Combinado em 2026-10-21, nesta ordem.

### a) O que sobrou da mesa de tarefas

- [ ] **Revisão do fluxo (t10, 2026-10-21): o que sobrou.** Os demais achados saíram na `t18` (ver [historico.md](historico.md)); ficam:
  - **Pasta da worktree que não sai ao validar.** Agora a tarefa fica em `Validating` e dá para tentar de novo, mas não há como ver qual processo segura a pasta; só a mensagem do git (`remove_task_worktree`, `tasks.rs:1439`). Visto em uso real na `t14` (2026-10-21): cinco tentativas de validar falharam com "a pasta ficou" antes de a pasta sair. Um teste desse caso não é possível sem simular o bloqueio do Windows.
  - **Mensagens ao agente não citam a validação.** `start_message` não conta ao agente que, depois de mesclada, a tarefa ainda pode voltar reprovada; hoje ele só descobre quando `reject_message` chega. Conferir no uso real se faz falta.
- [ ] **Seletor de modelos só com os que o usuário pode usar** (filtro por acesso; t14, 2026-10-21). A lista vem do `agy models` / `opencode models`, que mostram o que o CLI conhece, não o que o plano permite: modelos de provedores sem plano ativo (ex.: `opencode-go/…`) aparecem iguais aos que o usuário realmente pode usar. `ask-agent --models` herda o problema: só marca "(grátis)" pelo sufixo `-free` e não confirma acesso. Quando o seletor da Mesa passar a filtrar por acesso, `models::AgentModels`/`Core::models_inventory` devem usar o mesmo filtro (um único ponto). O Codex não tem lista (aparece como "não lista os modelos") e os demais CLIs (aider, goose…) saem como "sem escolha de modelo".

### b) Modo Documentos, fase 2

A fase 1 (a chave **Agentes | Documentos** no centro, só leitura) e o editor com realce, busca e seleção (`t39`) estão em [historico.md](historico.md). O que falta:

- [ ] **Diff lado a lado** (hoje só o unificado).
- [ ] **Gravar as abas de documento** (hoje somem ao fechar o app) e **documento ao lado de um agente** (hoje a chave troca o centro inteiro; ver a razão em [GERAL.md](GERAL.md), "Documentos").
- [ ] **Editor, depois (`t39`, 2026-10-23):** o que ficou de fora de propósito. Recuo automático ao dar Enter; substituir (`Ctrl+H`); ir para a linha (`Ctrl+G`); menu de clique direito (Copiar, Selecionar tudo) no diff e no arquivo grande (hoje é só Ctrl+C); rolagem horizontal em vez de quebra de linha; arquivo com CRLF e LF misturados ou fora do UTF-8 (hoje só leitura, com a razão); busca no markdown formatado e no pedido/relatório (hoje `Ctrl+F` mostra o código do `.md` de arquivo, e o pedido e o relatório não buscam); busca no diff conta linhas, não ocorrências; realce do que está dentro de um bloco de código do markdown formatado; mais linguagens (Go, C/C++, HTML/CSS, SQL, Java, C#: `highlight.rs` é uma tabela por linguagem, ver `memory/aprendizados/editor-de-documentos.md` antes de trocar por `syntect`); remover um projeto com documento sujo descarta a edição sem perguntar (o × da aba e o fechar do app perguntam).
- [ ] **Explorador mais útil:** hoje só mostra a árvore. Ideias: marcar o que mudou (git), filtrar/buscar e abrir os arquivos de uma tarefa a partir do detalhe dela.

### c) Temas (depois da `main`)

Fica para **depois** da promoção para a `main` (decidido com o usuário em 2026-10-22: tema é recurso novo, não estabilidade). O nome (`t20`) e a identidade visual (`t21`–`t23`) já estão feitos.

- [ ] **Temas** (pedido do usuário, 18/10). A identidade visual (visual C) está feita nas três partes (`t21`, `t22`, `t23`, ver [historico.md](historico.md)), com os tokens de `theme.rs` e sem cor solta. Falta o resto do pedido, os temas para o usuário escolher: trocar as constantes de `theme.rs` por uma paleta carregada em tempo de execução, escolhida nas configurações e gravada em `Config`. A paleta inclui o fundo e as 16 cores do terminal (`TERM_BG`/`TERM_FG`, `terminal/colors.rs`), então um tema muda o fundo de todas as abas de uma vez (CLIs com tema próprio, como o OpenCode, continuam usando o tema deles).

### d) Estabilizar e promover a `dev` para a `main`

- [ ] **Substituir a `main` pela `dev`** (aprovado pelo usuário em 2026-10-22). A `main` de hoje é o piloto em TypeScript: ela é **trocada inteira** pelo conteúdo da `dev`, sem mesclagem; a `main` antiga fica numa tag para consulta. O usuário usa a versão em Rust todo dia e considera a maior parte de [testes.md](testes.md) conferida no uso, mesmo sem marcar (os itens continuam lá). Antes da troca: a `t29` (suporte a Linux), a tarefa de validação para a `main` e a correção da janela maximizada.
- [ ] **Tirar a migração da config `CommandDeck/`** numa versão depois da promoção (a única reserva do nome antigo que sobrou; `config::legacy_config_files`, lista e razão em [MANUTENCAO.md](MANUTENCAO.md), seção 6).
- [ ] **Renomear a pasta local `commanddeck-dev`** (ação do usuário): o repositório no GitHub já se chama `AgentDeck` e as URLs antigas redirecionam; falta só a pasta.

## Mesa de tarefas

O que sobrou da mesa está no bloco "Próximo", item a).

- [ ] **Mesa de tarefas, depois:** a linha do tempo das mensagens entre agentes de cada tarefa (para aposentar a Conversa integrada); rodar os testes do projeto ao receber o relatório; editar uma tarefa em rascunho.

## Modelos

- [ ] **Modelo por instância, depois:** trocar o modelo de uma aba aberta (hoje o menu só mostra); lista de modelos do Codex no "+ Novo agente" (hoje só por `--model`/`roleModels`); campo livre para um modelo fora da lista; editar `roleModels` pela interface (hoje só no config.json). No OpenCode a variante (`modelo#alta`) só vale no headless, e cada aba com modelo sobe um servidor próprio (`--standalone`).
- O filtro de modelos com acesso está no bloco "Próximo", item a).

## Memória (fase 2)

A fase 1 (aprendizados com `touches`, "Situação atual", histórico de feedbacks, `--learn`) está em [historico.md](historico.md)
(`t13`, `t15`, `t16`) e descrita em [ARCHITECTURE.md](ARCHITECTURE.md).

- [ ] **Resumo de API por arquivo (estilo skeleton)**: entregar a assinatura das funções/tipos de um arquivo em vez do arquivo inteiro, para o agente ler menos. Casa com o `touches` dos aprendizados (mesmo gancho, `ask-agent --learn <arquivo>` poderia anexá-lo).
- [ ] **Injeção nas abas que não são de tarefa**: hoje só o `start_message` de uma tarefa leva a memória; os agentes das abas normais não recebem os aprendizados nem a situação atual.
- [ ] **`ContextProvider` geral**: virar os aprendizados e a situação atual provedores registrados e consultados por todos os caminhos de entrega. O trait (`crates/core/src/context.rs`) hoje é consultado só pelo executor headless (`headless.rs`); falta o spawn das abas, entregando o contexto pelo canal que cada CLI suporta (flag, `AGENTS.md` ou mensagem de sistema). RAG/embeddings entram aqui depois (ver "Adiados").
- [ ] **Perfis de agente em arquivo**, editáveis sem recompilar. Hoje cada agente recebe as instruções por um canal diferente (tabela em ARCHITECTURE.md, "Protocolo entre agentes"): Claude por `--append-system-prompt`, Codex por `-c developer_instructions`, OpenCode/Aider por arquivo, Antigravity pelo `AGENTS.md`. Proposta:
  - `prompts/_comum.md`: regras de colaboração válidas para todos (usar `ask-agent`, não resumir a Conversa integrada, não parar para perguntar ao usuário etc.);
  - `prompts/<agente>.md`: ajustes por agente (`claude.md`, `antigravity.md`...).
- [ ] **Situação atual mais rica**: decisões recentes (as outras tarefas, os arquivos delas, os sem commit e os últimos commits já entram).
- [ ] **Aprendizados que envelhecem**: nada avisa quando um `touches` aponta para um arquivo que foi renomeado ou removido (só o teste dos aprendizados do repositório, para caminhos literais); pensar em um comando de conferência e em como aposentar um aprendizado.
- [ ] **Aprendizado criado pelo agente chega à base só ao mesclar**: outro agente que comece antes da mescla não o vê. Aceitável na fase 1; se incomodar, a "Situação atual" poderia listar os arquivos de aprendizado novos nas branches em andamento.
- [ ] **RAG local/remoto** como um `ContextProvider` registrado via `Core::register_context_provider`.

## Ponte e instâncias

- [ ] Pré-autorizar o `ask-agent` nos CLIs que pedirem aprovação na aba (hoje só o Claude abre com `--allowedTools`; quais pedem é verificado em [testes.md](testes.md), seção "Ponte e instâncias").
- [ ] Se o agente da aba não responder por `ask-agent` (modelo fraco, ou o usuário assumiu o terminal), a pergunta expira em 10 minutos. Quem perguntou não fica mais bloqueado (recebe o aviso por escrito no terminal).
- [ ] Hooks de início/fim de turno só no Claude e no Codex (este só avisa o fim, e só quando não é shim `.cmd`). Antigravity e os demais seguem a heurística da saída (1,5 s parado / 30 s na fila).
- [ ] Falta `ask-agent --cancel <n>` para desistir de uma pergunta (hoje ela expira em 10 min).
- [ ] A ponte não percebe quando o cliente (`ask-agent`) cai no meio da espera (limitação do `tiny_http`); a pergunta fica pendente até ser respondida ou expirar. A resposta que não chegou a quem perguntou (200 sem leitor, aba fechada) fica em `ask-agent --inbox`; falta só um jeito de reentregá-la sozinha.
- [ ] No headless, aider (`--yes-always`) e interpreter (`-y`) rodam sem aprovação recebendo texto de outros agentes. Avaliar modos mais restritos.
- [ ] A ponte sem interface ainda usa o modo headless: não aparece nas abas.
- [ ] **Equipe: as instruções da ponte são fixadas na partida.** Um agente aberto antes desta versão não sabe do `--team`/`--boss` até reabrir a aba. O aviso de equipe só chega com a próxima mensagem (de propósito, para não gastar turno).

## Terminal

- [ ] **Claude Code duplica a conversa no histórico a cada mudança de largura** (Windows). Ao mudar de largura, ele limpa só a tela e reimprime a conversa, sem `ESC[3J` (que ele só manda fora do Windows). A cópia anterior fica no histórico, na largura antiga. Medido numa sonda (portable-pty + alacritty_terminal + o claude.exe real): uma cópia a mais por largura; o mesmo com o `OpenConsole.exe` do Windows Terminal. Limpar o nosso histórico antes não serve: ao alargar, ele redesenha só a tela e a conversa sumiria. Hoje o PTY só muda com o mouse solto e o tamanho parado por 150 ms (um arrasto = uma cópia). Ideia para depois: marcar onde começa a reimpressão e descartar o trecho antigo do histórico.
- [ ] **Terminal com linhas em branco / texto cortado depois de mudar o tamanho da janela** (relatado 23/10, `t37`; captura em `dados/anexos/s_mvkyzdj8_mvkzthkl-screenshot.png`, em branco a metade de cima). Em qualquer CLI, não só no Claude. **Causa achada e corrigida em parte**: ao crescer, a grade puxava o histórico e o ConPTY repintava por cima (linhas sumiam, linhas em branco embaixo); ver `memory/aprendizados/conpty-repinta-por-cima-do-historico-puxado.md`. Falta: (1) confirmar com o usuário que o defeito dele (texto embaixo, vão em cima) era esse; se ainda aparecer, ele liga "Gravar o terminal (diagnóstico)" no menu da aba (botão direito) ou usa "Salvar foto da grade agora" e manda `dados/pty/`; (2) nas mudanças de largura sobra a repetição de 1 a 3 linhas no histórico (mesma família do item acima).

## Interface

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.
- [ ] **Tela dividida, depois:** arrastar uma **aba** para outra janela (hoje o agrupamento é sempre pelo tipo) e maximizar uma janela temporariamente.
- [ ] **Tela dividida: custo do redesenho.** Com várias janelas, cada uma redesenha a grade inteira a cada quadro (e qualquer saída de qualquer aba pede um quadro). Se pesar com 5–6 janelas, guardar as linhas que não mudaram (cache por aba).
- [ ] **Projetos: lógica da Deck sem teste automático** (filtro das abas visíveis, gravação por projeto, carga sob demanda); hoje só o core tem testes. Extrair para funções puras testáveis, no estilo de `split.rs`.
- [ ] **Projetos: o AGENTS.md de projetos não ativos** só é reescrito quando se abre a próxima aba deles (a lista de agentes pode ficar velha até lá).
- [ ] **Projetos: a árvore e o Git não guardam o estado por projeto** (pastas abertas, arquivo selecionado): voltam do zero a cada troca.
- [ ] Com a barra lateral recolhida, a chave da tela dividida (agora na ORGANIZAÇÃO) fica inacessível: avaliar um atalho de teclado.
- [ ] **Uso do Antigravity com o token vencido:** o app usa só o token que o CLI mantém (sem guardar client ID/secret de terceiros), então com o CLI fechado por mais de ~1 h a barra mostra "sem dados". Avaliar ler o client ID/secret do CLI instalado para renovar em memória.
- [ ] **Uso do Antigravity depende de API interna** (`v1internal:retrieveUserQuotaSummary`): pode mudar sem aviso; hoje qualquer falha vira "sem dados". Avaliar um aviso discreto quando falhar por muito tempo.
- [ ] **Uso do Codex só testado com arquivos de exemplo** (o Codex não estava instalado): conferir o formato real de `rate_limits` nos `rollout-*.jsonl`.

- [ ] **Diff: trabalho por quadro proporcional ao tamanho** (ultrareview, 2026-10-23). `ui/diff_view.rs`, `draw`: a chave do cache de layout (`content_hash`) percorre e faz o hash de todas as linhas do diff a cada quadro, e `gutter_cols`/`marked`/`headers` também varrem tudo. Com um diff de vários MB aberto (lockfile), cada quadro refaz esse trabalho. Calcular a chave uma vez, quando o diff carrega (guardar com o `GitDiff`, ou um contador de geração).

- [ ] **`config.json` com BOM UTF-8 é ignorado em silêncio** (visto na `t28`): o app cai nos padrões sem aviso. Editar o arquivo pelo PowerShell 5.1 (`Out-File`/`Set-Content -Encoding utf8`) grava o BOM. Tirar o BOM ao ler (`config.rs`, `Config::load`) e avisar no log quando o arquivo não se lê.

## Git e GitHub

- A troca da `main` pela `dev` está no bloco "Próximo", item d).
- [ ] **Instruções do Antigravity/Goose/Copilot sem criar `AGENTS.md`** (t38): hoje o app cria o arquivo na pasta do usuário porque não achei outro canal verificado. A cota do Antigravity estava esgotada na hora do teste; ver se `agy` lê `AGENTS.md`/`GEMINI.md` de uma pasta passada em `--add-dir` (o app já passa `dados/`: bastaria gravar lá) ou se um plugin do `agy` (`~/.gemini/config/plugins/<nome>/rules/AGENTS.md`, global e valendo fora do AgentDeck) compensa. O Amazon Q (`.amazonq/rules`) segue sem canal, e a execução headless só leva as instruções no Claude.
- [ ] **O `AGENTS.md` que o app grava aparece como alterado/não rastreado** no repositório do usuário. Desde a auditoria, com o bloco igual ao do commit o conteúdo não muda (conferido em 2026-10-16 com `core.autocrlf=true`: mesmo hash do HEAD). O bloco não lista mais os agentes instalados (t38), então não muda de máquina para máquina. Em repositório sem `AGENTS.md`, só Antigravity, Goose e Copilot (que não têm outro canal) fazem o app criá-lo, como não rastreado, com aviso na Conversa; os demais CLIs recebem as instruções sem tocar no repositório. Avaliar escrever as instruções fora do arquivo versionado (ex.: `.git/info/exclude`, ou só pelo canal de cada CLI).
- [ ] Com `commit.gpgsign` e pinentry gráfico, a chave travada é recusada na hora (com a explicação de como destravar numa aba) em vez de abrir o diálogo de senha. Avaliar deixar o diálogo abrir quando houver pinentry gráfico.
- [ ] **GitHub, depois:** comentar e aprovar PRs, criar issue e reexecutar um job pelo app (hoje é só leitura + "Enviar para agente"); o caminho `gh auth token` não foi exercitado (o `gh` não está instalado aqui).

## Testes automáticos

- [ ] **A formatação bloqueia a CI desde 2026-10-22** (`run 47`, commit `146a486`): `cargo fmt --all -- --check` falha no Windows e no Ubuntu, então build, testes e clippy nem rodam — o Linux está sem validação automática. Pagar a dívida de rustfmt num commit próprio (sem misturar com código) e voltar a exigir a CI verde. Detalhe em [LINUX.md](LINUX.md).
- [ ] Os testes de integração do fluxo entre abas (`a_question_is_typed_into_the_tab_and_answered_by_callback` e vizinhos, e `an_agent_becomes_the_boss_of_the_role_it_opens_and_the_new_one_is_told`) só rodam em Unix (`#[cfg(unix)]` em `crates/core/tests/bridge.rs`, agentes falsos em shell): criar agentes falsos para Windows (`.cmd`/`.exe` de teste) para a CI do Windows cobrir a ponte.
- [ ] Testes de interface (egui) para a barra direita, a aba Git e a tela dividida (hoje só capturas de tela no Linux).

## Sistemas

Windows e Linux. Para o Linux, ver [LINUX.md](LINUX.md) (dependências, o que já foi verificado e o roteiro manual).

- [ ] **WSLg: conferir a janela no X11** (relato de um usuário, 2026-10-23: janelas e interface quebradas no WSLg).
  O app agora detecta o WSLg e abre pelo X11, porque o Wayland de lá quebra a janela (`crates/app/src/wslg.rs`), mas
  ninguém abriu o app num WSLg. Faltam: janela maximizada (relato de deslocamento de 32 px), escala (o WSLg informa
  sempre 1, então a interface fica pequena em tela de alta densidade; o `winit` 0.30 não tem mais
  `WINIT_X11_SCALE_FACTOR`), arrastar/redimensionar, cursor e a cota do Antigravity pelo chaveiro de lá. Detalhe em
  [LINUX.md](LINUX.md), seção 2.8.
- [ ] **Celular** (bem mais para frente, com o app bem estável; usuário, 2026-10-23): acompanhar o que os agentes estão
  fazendo e falar com eles longe do computador.

- [ ] **Wayland e Fedora de verdade:** o app só foi aberto à mão no X11 do Ubuntu; o job do Fedora prova build e testes, não a janela. Rodar o roteiro do LINUX.md numa sessão Wayland (GNOME e KDE) e num Fedora Workstation; o que falhar vira item aqui.
- [ ] **Empacotamento:** não há `.deb`, `.rpm`, Flatpak, AppImage, `.desktop` nem ícone instalado; hoje o app roda da pasta compilada (ou de uma pasta portátil).
- [ ] **Ditado com GPU no Linux:** só o motor de CPU é baixado; para CUDA/Vulkan é preciso compilar o whisper.cpp e pôr o `whisper-cli` no `PATH`. Avaliar baixar/instalar um motor pronto ou pelo menos uma receita no README.
- [ ] **Falhas silenciosas no Linux:** sem `xdg-desktop-portal` o diálogo de arquivo não abre e sem `xdg-utils` o "Abrir pasta" não faz nada — os dois sem aviso na interface. Mensagem clara quando o serviço/comando falta.
- [ ] **Textos de Git/GitHub no Linux:** os erros citam Git Credential Manager e "faça um push para entrar"; no Linux o caminho é `gh auth login` ou `credential.helper=store`. Ajustar o texto por plataforma.

## Adiados

Só depois de o projeto estar mais completo.

- O **agente Ollama próprio** (`ollama-agent`) e a ideia de "IA local como serviço interno" (embeddings, compressor de
  contexto) saíram do rumo em 2026-10-23: o AgentDeck gira em volta dos CLIs de agente, e um modelo local entra por um
  CLI que o suporte (ver [AGENTDECK.md](AGENTDECK.md), "O que o AgentDeck é e não é").

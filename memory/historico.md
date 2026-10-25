# Histórico

O que já foi feito, do mais recente para o mais antigo. É o destino de todo item concluído de
[pendencias.md](pendencias.md): uma linha por item (regras em [GERAL.md](GERAL.md)); o detalhe técnico fica no commit e
o estado atual em [ARCHITECTURE.md](ARCHITECTURE.md).

## Concluído

- [x] **`crates/app/src/deck.rs` dividido em módulos** (2026-10-25, `t54`): o arquivo de ~2.900 linhas virou `crates/app/src/deck/` (`mod`, `frame`, `tabs`, `keyboard`, `sidebar`, `projects`, `feed`, `attachments`, `dictation`), sem mudar comportamento.
- [x] **Codex no Windows: a mensagem colada não fica mais parada no campo** (2026-10-25): o Enter espera 1 ms a mais por caractere colado (teto de 10 s), evitando virar quebra de linha antes de o Codex ler toda a colagem.
- [x] **`tasks.rs` dividido em módulos** (2026-10-25, `t53`): `crates/core/src/tasks.rs` virou `crates/core/src/tasks/` (`mod`, `text`, `store`, `lifecycle`, `merge`, `finish`, `git_ops`, `stash`), sem mudar a API.
- [x] **Cenários de demonstração separados de `app.rs`** (2026-10-25, `t55`): `debug_ui::apply` e os estados fictícios de captura foram para `crates/app/src/debug_ui.rs`.
- [x] **Sem a pergunta "Do you trust this folder?" nas abas** (2026-10-25): antes de abrir a aba, o app registra a confiança da pasta no Claude, no Codex e no Antigravity (`trust.rs`), sem travar abas em segundo plano.
- [x] **A mesa escolhe pela capacidade do modelo, depois pela cota** (2026-10-24, `t52`): nível da tarefa (`simples`, `media`, `complexa`), tabela de benchmarks + histórico de tarefas concluídas e seleção do modelo capaz com mais folga.
- [x] **Destaque: trocas seguidas de aba não acumulam redimensionamentos** (2026-10-24, `t51`): debounce de 600 ms na troca de destaque, cancelamento quando volta ao mesmo tamanho e espera da reimpressão terminar antes do próximo resize.
- [x] **Projeto readicionado reaproveita o id** (2026-10-24, `t50`): remover um projeto guarda id e pasta em `removedProjects`, e readicionar a mesma pasta recupera as tarefas em `dados/tarefas/<id>.json`.
- [x] **Escolha do agente da tarefa pela cota** (2026-10-24, `t49`): `--task new` sem `--agent` e o formulário recomendam o agente oficial com menor cota usada, e `ask-agent` avisa quando o destino passa de 80%.
- [x] **Escopo de escrita por tarefa, com fila** (2026-10-24, `t45`): tarefas declaram caminhos (`--scope`); se sobrepõem a outra em andamento ou revisão, entram em `Na fila` e começam sozinhas quando a anterior libera.
- [x] **Testes da ponte também no Windows** (2026-10-24, `t42`): os 17 testes de `tests/bridge.rs` passaram a rodar no Windows usando o binário de teste `fake_agent` em vez de scripts de shell.
- [x] **Antigravity sem `AGENTS.md` criado no repositório** (2026-10-24, sobra da `t38`): o app grava `dados/agy/<instância>/AGENTS.md` e passa a pasta em `--add-dir`; só Goose e Copilot ainda criam `AGENTS.md` no projeto.
- [x] **Worktree que não sai: o log diz quem segura** (2026-10-24): `file_lock.rs` lista no log e no aviso da tarefa os arquivos restantes e, no Windows, os processos que os mantêm abertos (Restart Manager).
- [x] **`config.json` editado no PowerShell** (2026-10-24): a leitura da configuração aceita UTF-8 com BOM e UTF-16, preservando os projetos quando o arquivo é salvo pelo PowerShell 5.1.
- [x] **A `main` passou a ser a reescrita em Rust** (2026-10-24): a branch `main` antiga em TypeScript foi substituída pela `dev` em Rust, com CI verde em Windows, Ubuntu e Fedora.
- [x] **Cota por modelo para os agentes** (2026-10-24): `/ask` recusa com 409 modelo esgotado; `--models` ordena os agentes por folga e mostra a cota por grupo, e `--team`/`--list` avisam ao lado do modelo.
- [x] **Mesclagem com stash e ditado: três achados da revisão da `t35` e da `t34`** (2026-10-23): aviso de onde ficou o stash no conflito, devolução do stash pelo hash SHA e aviso claro num build sem áudio.
- [x] **Memória, fase 2: `ask-agent --outline <arquivo>…`** (2026-10-23, `t40`): devolve o esqueleto do arquivo (assinaturas e linha, sem corpos, em Rust e Markdown) para o agente ler só o trecho necessário.
- [x] **Memória, fase 2: situação atual em qualquer aba e aviso de arquivo em uso** (2026-10-23, `t41`): `ask-agent --learn` sem argumentos inclui as tarefas em andamento, e com arquivos avisa se outra tarefa mexe neles.
- [x] **Editor: três achados da revisão da `t39`** (2026-10-23): busca fechada não marca linhas no arquivo grande, falha ao salvar na saída explica o motivo, e recarregar/descartar adota fim de linha e BOM do disco.
- [x] **Remover um projeto avisa da edição não salva** (2026-10-23, sobra da `t39`): a confirmação de remover projeto conta os documentos com alterações não salvas e avisa antes de descartá-las.
- [x] **Editor de arquivos no modo Documentos, com realce de sintaxe** (2026-10-23, `t39`/`t36`): edição de arquivos até 256 KB com `Ctrl+S` (preservando CRLF/LF e BOM), realce em 9 linguagens, busca (`Ctrl+F`), seleção no diff e aviso de mudança externa.
- [x] **Ditado por voz opcional** (2026-10-23, `t34`): o app pergunta antes de baixar o Whisper (~1,6 GB) na primeira abertura ou no uso do microfone, e permite baixar depois ou remover em ☰ Configurações.
- [x] **Cota do Antigravity no Linux e WSLg pelo X11** (2026-10-23, `t33`): leitura do token OAuth do `agy` no Secret Service (D-Bus) e em arquivo no Linux, e janela forçada a abrir por X11 sob o WSLg.
- [x] **Mesclar: resolver a recusa ali mesmo na janela** (2026-10-23, `t35`): a recusa de mesclagem oferece botões para guardar alterações locais em stash e mesclar, ver o que está sujo ou devolver ao agente para commitar/resolver conflito.
- [x] **Instruções do AgentDeck em toda aba, sem subagente escondido** (2026-10-23, `t38`): cada CLI recebe as instruções pelo seu próprio canal (sem depender de `AGENTS.md` prévio), proibindo subagentes internos do CLI por padrão.
- [x] **Nomes antigos do CommandDeck removidos** (2026-10-23, `t32`): saíram `COMMANDDECK_*`, `x-commanddeck-token`, marcador `commanddeck:` e `.commanddeck.lock`; ficou só a migração inicial do `config.json`.
- [x] **Agente Ollama próprio removido** (2026-10-23, `t32`): saíram o binário `ollama-agent`, o módulo `core::ollama` e o agente `ollama` dos menus, da ponte e da configuração.
- [x] **Terminal: linhas sumiam e ficavam em branco ao mudar o tamanho da janela (Windows)** (2026-10-23, `t37`): `terminal/resize.rs` desfaz o puxão de histórico do `alacritty_terminal` para casar com a repintura do ConPTY, mais gravação de diagnóstico.
- [x] **Achados do ultrareview** (2026-10-23): confirmação de entrega em `POST /inbox/delivered` após imprimir, caixa de entrada preservada ao renomear aba e barra invertida mantida como nome de arquivo no Unix.
- [x] **Achados do `/code-review`** (2026-10-23): janela minimizada não sobrescreve estado maximizado, recusa abre detalhe da tarefa de qualquer tela, renovação da cota redesenha topbar e `agy --version` só roda ao ler cota.
- [x] **Log sem o ruído do `ureq`** (2026-10-23, achado da `t31`): nível de log de `ureq`/`ureq_proto` ajustado para `WARN`, evitando gravar cabeçalhos e IPs no `agentdeck.log`.
- [x] **Recusa de ação pelo cartão da lista agora aparece** (2026-10-23): uma ação recusada ("Mesclar"/"Validar") clicada direto no cartão abre o detalhe da tarefa para mostrar a faixa vermelha de erro.
- [x] **Binários do Whisper podados** (2026-10-23, achado da `t31`): removidos do git 25 executáveis de exemplo/teste e DLLs não usadas de `whisper/bin-cpu/Release/`, ficando só o `whisper-cli.exe` e suas DLLs.
- [x] **Janela maximizada volta ao modo janela** (2026-10-22, `t30`): preenchimento de `ViewportInfo` antes de cada quadro para restaurar a janela maximizada, trocar o ícone do botão e preservar o tamanho normal em `Config`.
- [x] **Validação antes da `main`** (2026-10-22, `t31`): conferidos README, CONTRIBUTING, licenças de terceiros do Whisper, `.gitignore` de `dados/` e `/.agentdeck/`, e abertura inicial limpa sem configuração.
- [x] **Verificação do suporte a Linux (Ubuntu e Fedora)** (2026-10-22, `t29`): auditoria em `LINUX.md`, correção de separador no `read_dir`, fallback para `~/.local/share/AgentDeck`, fontes no Fedora e job `fedora` na CI sem GTK3.
- [x] **Modo Destaque na ORGANIZAÇÃO e realce do agente selecionado** (2026-10-22, `t28`): terceiro modo (`Abas | Dividida | Destaque`) com a janela ativa em 2/3 à esquerda e as demais empilhadas à direita, mais halo colorido e esmaecimento.
- [x] **Esforço por tarefa** (2026-10-22, `t25`): seletor de esforço (`Padrão | Baixo | Médio | Alto`) no formulário e `--effort` no CLI, traduzido como `--effort` no Claude e sufixo do modelo no Antigravity.
- [x] **Resposta perdida: log em disco e `ask-agent --inbox`** (2026-10-22, `t27`): log rotativo em `dados/agentdeck.log`, caixa de entrada por instância (`--inbox [--all]`) e aviso na Conversa quando a entrega no terminal falha.
- [x] **Limpezas: dica do Ctrl+L, padding da statusline e modelos da demonstração** (2026-10-22, `t26`): removidos restos do antigo compositor, preservado o `padding` da statusline do usuário no Claude e atualizados os modelos de demo.
- [x] **Mesa: mesclar com o `AGENTS.md` do app e falha de mesclagem sem beco** (2026-10-22, `t24`): a mesclagem ignora e restaura o bloco do app no `AGENTS.md`, e falhas externas deixam a tarefa em revisão (ou permitem devolver).
- [x] **Visual C, parte 3: barra direita, menus e modais** (2026-10-21, `t23`): abas-pílula na barra direita, cartões ativos de 3 linhas com barra de progresso na mesa, filtro de histórico, Conversa translúcida e modais opacos.
- [x] **Visual C, parte 2: barra lateral e centro** (2026-10-21, `t22`): chave `Agentes | Documentos` no topo da barra lateral, agentes compactos com tag do modelo, explorador com letra do Git e faixa de abas em pílulas.
- [x] **Visual C, parte 1: a base** (2026-10-21, `t21`): paleta semântica `theme::pal()`, destaque na cor do agente ativo, fontes Inter e IBM Plex Mono embutidas, ilhas de vidro, barra de título nova e ícone do app.
- [x] **Renomear para AgentDeck** (2026-10-21, `t20`): crates, executável `agentdeck.exe`, variáveis `AGENTDECK_*`, cabeçalho `x-agentdeck-token`, marcadores do `AGENTS.md` e migração automática da configuração do CommandDeck.
- [x] **Documentos no centro: chave Agentes | Documentos** (2026-10-20, `t19`): visão central para ler diffs, arquivos e markdown (`Ctrl+Shift+D`) com selo de atividade nos agentes e terminais rodando escondidos sem redimensionar.
- [x] **Mesa sem becos sem saída depois da mesclagem** (2026-10-20, `t18`): gravação de `merge_commit`, botão "Ver o que foi mesclado", `--task done` aceito em validação quando há commits novos e isolamento de `OPENCODE_CONFIG_CONTENT`.
- [x] **Memória: aprendizados e arquivos sem commit na devolução e na reprovação** (2026-10-20, `t16`): feedbacks entregam aprendizados dos arquivos recém-citados, e a Situação atual inclui arquivos sem commit das worktrees.
- [x] **Memória: `--learn` sem argumentos e nas instruções da ponte** (2026-10-20, `t15`): `ask-agent --learn` sem arquivos mostra os aprendizados globais e o índice, e as instruções mandam consultá-lo antes de editar.
- [x] **Inventário de agentes e modelos** (2026-10-20, `t14`): `ask-agent --models [<agente>]` lista os agentes instalados e seus modelos (gratuitos marcados e primeiro; OpenCode agrupado por provedor).
- [x] **Memória em duas camadas, fase 1** (2026-10-19, `t13`): aprendizados em `memory/aprendizados/*.md` com globs `touches` (`--learn` e início da tarefa) mais a "Situação atual" (outras tarefas, commits da base e histórico de feedbacks).
- [x] **Abas escondidas: indicador e acesso** (2026-10-19, `t12`): quando as abas não cabem na janela, a faixa ganha degradê, setas `‹`/`›` para rolar até a aba oculta e menu `▾` com todas as abas.
- [x] **Conversa só leitura, microfone e anexo no cabeçalho da janela e anexos na "Nova tarefa"** (2026-10-19, `t11`): removido o compositor da Conversa; microfone e anexo foram para cada janela de agente, e "Nova tarefa" aceita anexos.
- [x] **Mesa de tarefas: etapa "Aguardando validação"** (2026-10-18, `t10`): mesclar mantém aba, worktree e branch vivas em `Aguardando validação` até o usuário `Validar` (limpa tudo) ou `Reprovar` (devolve ao mesmo agente).
- [x] **Mesa de tarefas: o agente não começa sozinho** (2026-10-18, `t6`/`t10`): ordem explícita de início no fim do pedido e entrega do pedido inicial via arquivo na linha de comando ao abrir a aba (`claude`, `agy -i`, `opencode --prompt`).
- [x] **Ditado preso a cada aba de CLI** (2026-10-18, `t5`): o microfone (`Ctrl+Shift+M`) grava, transcreve com o Whisper local e cola o texto na aba onde a gravação começou, sem enviar Enter.
- [x] **OpenCode: escolher o provedor antes do modelo** (2026-10-18, `t3`): os menus de modelo agrupam o OpenCode por provedor (`opencode`, `opencode-go`…), destacando e listando primeiro os modelos `-free`.
- [x] **Regras da tarefa: o agente não pede merge** (2026-10-17, `t2`): a instrução inicial da tarefa proíbe o agente de pedir ou rodar `--task merge`, encerrando o trabalho dele em `--task done`.
- [x] **Mapa de agentes removido** (2026-10-17, `t1`): removida a aba Mapa da barra direita e o módulo `agent_map`, já cobertos pela árvore de agentes na barra lateral.
- [x] **Mesa de tarefas, fase B: a interface** (2026-10-17, `feat/mesa-ui`): aba Tarefas na barra direita com lista por estado, formulário "+ Nova", detalhe com pedido/relatório/diff e ações de iniciar, revisar, mesclar, devolver e descartar.
- [x] **Mesa de tarefas, fase A: core, ponte e CLI** (2026-10-17, `feat/mesa-tarefas`): ciclo de vida de tarefas com worktree e branch próprias em `.agentdeck/worktrees/`, armazenamento em `dados/tarefas/` e comandos `ask-agent --task`.
- [x] **Modelo de LLM por instância** (2026-10-17, `feat/modelo-por-agente`): escolha de modelo por aba (`ask-agent --model`, menu "+ Novo agente" e `roleModels`), conflito 409 se já aberta com outro modelo e exibição na UI e na ponte.
- [x] **Revisão do `core/github.rs` por modelos gratuitos do OpenCode** (2026-10-16): credencial do remoto protegida em mensagens de erro, cotas da API separadas por login e correção do teste 401 no Windows.
- [x] **Integração com o GitHub** (2026-10-16, `claude/github`): aba GitHub com PRs e checks, issues, Actions e fim do log de jobs falhos, autenticação via Git Credential Manager/`gh` e botão "Enviar para agente…".
- [x] **Git completo e bugs de git da auditoria** (2026-10-16, `claude/git`): stash, stage/discard por trecho (hunk), histórico paginado, merge/rebase pelo menu de branches, aba de conflitos e avisos claros para GPG e SSH.
- [x] **Captura de tela sem download do Whisper** (2026-10-16): `COMMANDDECK_SMOKE` não inicia mais o download automático de 1,6 GB do Whisper em pastas temporárias.
- [x] **Auditoria: workspace, download, Whisper e uso** (2026-10-16, `claude/infra`): watcher no git dir real, timeout de leitura no download, extração atômica do Whisper e melhorias na leitura de cota de Claude, Codex e Antigravity.
- [x] **Auditoria: interface** (2026-10-16, `fix/interface`): proporções da tela dividida preservadas em janela estreita, modais bloqueando fundo, microfone aberto fora da thread da UI e rotação do `crash.log`.
- [x] **Auditoria: ponte e equipe** (2026-10-16, `fix/ponte`): perguntas rastreadas por número mesmo ao renomear aba, limpeza ao remover projeto, aviso de mudança de equipe anexado na digitação e orçamento de 64 MB na ponte.
- [x] **Auditoria: terminal e processos** (2026-10-16, `fix/terminal`): fila de escrita `pty-write` por aba sem travar UI, escape `cmd_safe_arg` para shims `.cmd`, OSC 52 restrito à aba ativa e Job Object no Windows.
- [x] **Terminal congelava ao redimensionar as janelas** (2026-10-16): `Emulator::expire_sync` cobra o prazo de 150 ms da atualização sincronizada (`ESC[?2026h`), evitando tela congelada quando o fim se perde no redesenho.
- [x] **Conferido em uso** (2026-10-15 e 16): validados pelo usuário e ao vivo pela ponte os projetos, a tela dividida, a barra de título com cotas, as perguntas/respostas (síncronas e 202), os cargos em paralelo e os acentos.
- [x] **Auditoria de 2026-10-15 (regressão + correções)**: escrita atômica de config, estado do Git isolado por geração de projeto, fila de abas sem travar em Enter vazio, filtro de ESC na colagem e endurecimento de caminhos.
- [x] **Equipe de agentes reorganizável e conhecida pelos agentes** (2026-10-15): hierarquia chefe/subordinados arrastável na barra lateral (`team.rs`), `ask-agent --team` e `--boss`, e aviso automático quando a posição muda.
- [x] **Agentes de todos os projetos, em árvore, com logo** (2026-10-15): árvore de agentes por projeto na barra lateral com logo do modelo, indicador de turno ativo/concluído e troca de projeto ao clicar num agente.
- [x] **Novo agente dentro do projeto** (2026-10-15): o botão "+" saiu da barra de título e virou a linha "+ Novo agente" dentro do projeto ativo na barra lateral.
- [x] **Projetos como unidade principal** (2026-10-14): cada projeto é uma pasta com suas próprias abas, Conversa e Git; trocar de projeto mantém os terminais dos outros rodando em segundo plano.
- [x] **Barra de título própria, uso dos agentes e barra lateral nova** (2026-10-14): janela sem decoração do sistema, indicadores de uso (sessão e semana) na barra de título e seletor de projetos na barra lateral.
- [x] **Tela dividida por tipo de agente** (2026-10-14): grade automática com uma janela por tipo de CLI, divisores arrastáveis, troca de posição pela alça `⋮⋮` e atalhos entre janelas (`Ctrl+Alt+setas`).
- [x] **Barra direita mais estreita** (2026-10-14): largura padrão reduzida de 380 para 330 px (mínimo 280 px).
- [x] **Gemini removido** (2026-10-14): substituído pelo Antigravity (`agy`) em todo o catálogo de agentes, prompts e documentação.
- [x] **Aba Git e ícones no cabeçalho da barra direita** (2026-10-13): aba Git local (status, diff, stage, commit, branches, pull/push) com releitura automática pelo watcher de `.git/`.
- [x] **Barra lateral direita fixa com a Conversa** (2026-10-13): a Conversa integrada passou de baixo dos terminais para uma barra lateral direita fixa de largura ajustável.
- [x] **Segurança da ponte e das abas** (2026-10-13): token próprio por processo de agente, resposta restrita ao perguntado (`--reply`), limpeza de ESC na colagem, validação de `Host` e limites de corpo e conexões.
- [x] **Profundidade e perguntas órfãs entre abas** (2026-10-13): limite de profundidade de delegação aplicado também entre abas, e descarte das perguntas pendentes ao fechar a aba de quem perguntou.
- [x] **Entrega previsível nas abas** (2026-10-13): `delivery.rs` respeita digitação do usuário, hooks de início/fim de turno do Claude e do Codex e entrega atrasada (HTTP 202) quando a resposta passa de 90 s.
- [x] **Codex e OpenCode no catálogo de agentes** (2026-10-12): suporte aos CLIs Codex e OpenCode em abas interativas e no modo headless.
- [x] **Ambiente isolado** (2026-10-12): variáveis internas do app não vazam para os agentes filhos e a pasta de dados é protegida por trava `.lock`.
- [x] **Testes independentes da máquina** (2026-10-12): `system_dirs = false` nos testes para isolar os CLIs instalados e `rustfmt.toml` + `cargo fmt --check` na CI.
- [x] **Download do Whisper robusto** (2026-10-12): retomada por `Range`, novas tentativas com espera crescente, verificação de SHA-256, barra de progresso e suporte a Linux (`download.rs`).
- [x] **Mouse no terminal** (2026-10-12): envio de clique, arrasto e rolagem aos programas TUI nos modos 1000/1002/1003/1006, com `Shift` reservado para seleção de texto local.
- [x] **IME no terminal** (2026-10-12): suporte a composição de acentos e CJK ancorada no cursor do terminal, exibindo a pré-edição sublinhada.
- [x] **Instâncias com identidade e conversa entre abas reais** (2026-10-12): abas endereçáveis como `modelo[@cargo[@nome]]`, digitação automática no PTY pela ponte e detecção de ciclos de espera.
- [x] **Colar no Windows derrubava o app** (2026-10-11): leitura da área de transferência restrita à thread da interface, eliminando corrupção de heap no Windows.
- [x] **Validação inicial no Windows** (2026-10-11): testados à mão abas, ConPTY, microfone/Whisper, som, arrastar arquivos, colar imagem e restauração de sessão.

## Auditoria de 2026-10-15

Revisão completa da base por quatro instâncias do Claude e pelo Antigravity, cobrindo persistência atômica da
configuração, isolamento do Git na troca de projeto, fila e segurança da ponte, colagem segura nos terminais e
endurecimento de caminhos no workspace. Todos os achados foram corrigidos (exceto o fallback automático do motor GPU
para CPU do Whisper quando sai com código 0 sem saída, descartado).

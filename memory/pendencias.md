# Pendências

Lista do **trabalho de desenvolvimento** que falta (melhorias e limitações conhecidas), em ordem sugerida de
prioridade. Ao concluir um item, mova-o para "Concluído" com uma linha do que foi feito (regras dos documentos em
[GERAL.md](GERAL.md)).

Os **testes manuais** (validar no Windows, com os agentes reais) ficam em [testes.md](testes.md). Um teste que
falhar vira item aqui.

## 0. Direção: IA local como serviço interno (adiado)

> **Adiado** até o projeto estar mais completo.

Decisão: com Claude e Antigravity disponíveis, o Ollama deixa de ser "agente par" e vira **infraestrutura** do app.

- [ ] **Embeddings locais para RAG.** Modelo de embeddings no Ollama (`nomic-embed-text` ou `bge-m3`, < 1 GB, cabe na RX 7600 8 GB).
  - Indexar o workspace em pedaços (*chunks*) e manter o índice atualizado pelo watcher existente (`workspace.rs`).
  - Expor a busca de duas formas: `ask-agent`/CLI `search` (sob demanda) e um `ContextProvider` com orçamento fixo de tokens (ex.: 1.500).
  - Índice local (ex.: em `dados/`), sem enviar nada para fora.
- [ ] **Compressor de contexto.** Modelo pequeno (`qwen3:8b`, 100% GPU) para resumir:
  - respostas longas no `ask-agent` antes de entregar ao agente que perguntou (`bridge.rs`), mantendo o original na Conversa integrada.
- [ ] **Medir tokens** (`prompt_eval_count` do Ollama, tamanho dos prompts enviados a Claude/Antigravity) para comparar antes/depois.
- [ ] **Configuração** em `Config`: modelo de embeddings, modelo compressor, limiares (ex.: comprimir acima de N caracteres), ligar/desligar.
- [ ] Aba Ollama-agente passa a ser opcional (os itens da seção 1 ficam como baixa prioridade).

## 1. Agente local (Ollama) — adiado: contexto e comportamento

> **Adiado** até o projeto estar mais completo.

Problema observado: o modelo local esquece que é um agente ("age como chatbot") e para no meio da colaboração com outros agentes (ex.: Antigravity) em vez de continuar a conversa sozinho.

- [ ] **Configurar `num_ctx`**. O Ollama usa por padrão uma janela de contexto pequena (2–4k tokens). O prompt de sistema, as 6 ferramentas e o `AGENTS.md` ocupam boa parte dela, e o começo, onde está o "você é um agente", acaba cortado. Enviar `options.num_ctx` nas chamadas a `/api/chat` (ex.: 16k–32k), configurável em `Config`.
  - Arquivo: `crates/core/src/ollama.rs` e `crates/core/src/config.rs`.
- [ ] **Prompt de sistema mais curto para modelos pequenos (4B–9B).** O atual é longo e cheio de regras em caixa alta, que modelos pequenos seguem mal. Ter uma versão enxuta.
- [ ] **Instrução de colaboração mais perto do fim do contexto.** O lembrete de "continue conversando com o agente parceiro" hoje vai no fim de uma saída grande do `ask-agent`. Avaliar reinjetar a instrução como mensagem curta de sistema/usuário logo após o resultado.
- [ ] **Verificação antes de encerrar.** Hoje, quando o modelo responde em texto sem chamar ferramenta, o loop considera a tarefa concluída. Adicionar um passo de checagem ("a tarefa terminou ou falta chamar `ask-agent`/ferramentas?") antes de devolver a resposta.
- [ ] Revisar o limite de 25 passos (`MAX_STEPS`) para conversas longas entre agentes.

## 2. Injeção de contexto como um ponto único (para qualquer IA) — adiado

> **Adiado** (gestão de contexto) até o projeto estar mais completo.

Hoje cada agente recebe instruções por um caminho diferente:

| Agente | Como recebe as instruções hoje |
| :-- | :-- |
| Claude | `--append-system-prompt` (`agents.rs` / `headless.rs`) |
| Antigravity | bloco marcado no `AGENTS.md` do workspace |
| Ollama | prompt fixo em `ollama.rs` (`system_prompt`) |

Proposta:

- [ ] **Perfis de agente em arquivo**, editáveis sem recompilar:
  - `prompts/_comum.md`: regras de colaboração válidas para todos (usar `ask-agent`, não resumir a Conversa integrada, não parar para perguntar ao usuário etc.);
  - `prompts/<agente>.md`: ajustes por agente (`ollama.md`, `claude.md`, `antigravity.md`...).
- [ ] **`ContextProvider` para todos os agentes.** O trait (`crates/core/src/context.rs`) hoje só é consultado pelo Ollama. Passar a consultá-lo também no executor headless e no spawn das abas, entregando o contexto pelo canal que cada CLI suporta (flag, `AGENTS.md` ou mensagem de sistema).
- [ ] **RAG local/remoto** como um `ContextProvider` registrado via `Core::register_context_provider`.

## 3. Terminal

Nada pendente.

## 4. Interface

- [ ] **Botão "quem conversa com quem"** (próximo passo do Mapa): contagem de mensagens por par de agentes na sessão, a partir do anel de recentes do `Deck` ampliado (hoje só guarda 60 s).
- [ ] Janela estreita (perto do mínimo de 820 px): com muitas abas, as de trás encolhem até virar só o ponto colorido.

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.
- [ ] **Destacar o agente selecionado** (seção ORGANIZAÇÃO da barra lateral): realçar a janela/aba do agente escolhido e o que é dele (pedido do usuário; a seção já existe com a chave da tela dividida).
- [ ] Com a barra lateral recolhida, a chave da tela dividida (agora na ORGANIZAÇÃO) fica inacessível: avaliar um atalho de teclado.
- [ ] **Uso do Antigravity com o token vencido:** o app usa só o token que o CLI mantém (sem guardar client ID/secret de terceiros), então com o CLI fechado por mais de ~1 h a barra mostra "sem dados". Avaliar ler o client ID/secret do CLI instalado para renovar em memória.
- [ ] **Uso do Antigravity depende de API interna** (`v1internal:retrieveUserQuotaSummary`): pode mudar sem aviso; hoje qualquer falha vira "sem dados". Avaliar um aviso discreto quando falhar por muito tempo.
- [ ] **Uso do Codex só testado com arquivos de exemplo** (o Codex não estava instalado): conferir o formato real de `rate_limits` nos `rollout-*.jsonl`.
- [ ] **Statusline do usuário perde o `padding`**: a nossa (`ask-agent --usage`) fixa 0 e repassa a dele.
- [ ] **Acentos trocados por `?` nas respostas do Antigravity** pela ponte (as do Claude chegam certas). Causa provável: o PowerShell 5.1 do Antigravity converte o texto ao passar para o `ask-agent` (`$OutputEncoding`/`Get-Content` sem `-Encoding UTF8`). Orientar nas instruções da ponte ou aceitar `ask-agent --reply <n> --file <caminho>` lido em UTF-8.
- [ ] **Janela sem decoração:** validar no Windows real (arrastar, snap, Win+setas, redimensionar pelas bordas, maximizada sem cobrir a barra de tarefas); no Linux, conferir X11/Wayland ([testes.md](testes.md), seção 13).

## 5. Testes automáticos

- [ ] Os testes de integração do fluxo entre abas (`a_question_is_typed_into_the_tab_and_answered_by_callback` e vizinhos) só rodam em Unix (agentes falsos em shell): criar agentes falsos para Windows (`.cmd`/`.exe` de teste) para a CI do Windows cobrir a ponte.
- [ ] Testes de interface (egui) para a barra direita, o Mapa, a aba Git e a tela dividida (hoje só capturas de tela no Linux).
- [ ] Tela dividida: com várias janelas, cada uma redesenha a grade inteira a cada quadro (e qualquer saída de qualquer aba pede um quadro). Se pesar com 5–6 janelas, guardar as linhas que não mudaram (cache por aba).
- [ ] Tela dividida, depois: arrastar uma **aba** para outra janela (hoje o agrupamento é sempre pelo tipo) e maximizar uma janela temporariamente.

## 6. Novas funcionalidades (comparação com o Orca)

- [ ] **Integração com o GitHub (PRs, issues e Actions).**
  - Listar e abrir PRs e issues do repositório do workspace, ver o estado das Actions (checks) de cada PR e o log de um job que falhou.
  - Ações diretas: mandar um PR, uma issue ou um job vermelho para um agente (ex.: "claude@review, revise o PR #12"; "claude@dev, corrija este job").
  - Caminho mais simples: usar o `gh` já autenticado na máquina (`gh pr list --json`, `gh run view --log-failed`), sem guardar token no app; alternativa: API REST com token no cofre do sistema.
  - Novo módulo no core (ex.: `github.rs`) chamado fora da thread da interface; painel próprio na barra lateral.
  - Depende de o workspace ser um repositório git (hoje não há nada de git no código).
- [ ] **Editor de arquivos.** Abrir e editar arquivos do workspace dentro do app, em vez de mandar para o programa padrão do sistema (`Core::open_path`).
  - egui não tem editor de código pronto: avaliar `egui_extras::syntax_highlighting`/`syntect` para realce, ou um componente de editor de terceiros.
  - Mínimo útil: abas de arquivo ao lado das abas de terminal, realce de sintaxe, salvar (Ctrl+S), aviso quando um agente alterar o arquivo aberto (o watcher de `workspace.rs` já avisa) e pré-visualização de Markdown.
  - Clique na árvore continua inserindo `@arquivo`; o duplo clique passaria a abrir no editor (com opção de manter o programa do sistema).

## Instâncias (abas) — pontos em aberto

- [ ] Pré-autorizar o `ask-agent` nos CLIs que pedirem aprovação na aba (hoje só o Claude abre com `--allowedTools`; quais pedem é verificado em [testes.md](testes.md), seção 8).
- [ ] Se o agente da aba não responder por `ask-agent` (modelo fraco, ou o usuário assumiu o terminal), a pergunta expira em 10 minutos. Quem perguntou não fica mais bloqueado (recebe o aviso por escrito no terminal), mas só o `ollama-agent` responde sozinho.
- [ ] Hooks de início/fim de turno só no Claude e no Codex (este só avisa o fim, e só quando não é shim `.cmd`). Antigravity e os demais seguem a heurística da saída (1,5 s parado / 30 s na fila).
- [ ] Falta `ask-agent --cancel <n>` para desistir de uma pergunta (hoje ela expira em 10 min).
- [ ] A ponte não percebe quando o cliente (`ask-agent`) cai no meio da espera (limitação do `tiny_http`); a pergunta fica pendente até ser respondida ou expirar.
- [ ] No headless, aider (`--yes-always`) e interpreter (`-y`) rodam sem aprovação recebendo texto de outros agentes. Avaliar modos mais restritos.
- [ ] A ponte sem interface ainda usa o modo headless: não aparece nas abas.
- [ ] Não há como o usuário dar cargo a uma aba aberta à mão (hoje só duplicatas ganham nome, tipo `claude@main@ana`).

## 7. Git

- [ ] **O `AGENTS.md` que o app grava aparece como alterado/não rastreado** no repositório do usuário (o bloco é reescrito ao iniciar). Avaliar escrever as instruções fora do arquivo versionado (ex.: `.git/info/exclude`, ou só pelo canal de cada CLI).
- [ ] Ssh com senha na chave: hoje o push/pull falha com "falha de autenticação" (o `BatchMode` evita travar); orientar a usar o ssh-agent.
- [ ] Stash, preparar por trecho (hunk), editor de conflitos, histórico (log), merge/rebase pela interface.
- [ ] Integração com o GitHub (PRs, issues, Actions) — ver a seção 6.

## Concluído

- [x] **Barra de título própria, uso dos agentes e barra lateral nova.**
  - Janela sem a barra do sistema: ☰ (configurações), "CommandDeck", recolher barra, "+", tela dividida e botões de janela menores; arrastar, duplo clique e bordas pelo egui (`ui/titlebar.rs`).
  - Uso por agente aberto: logo + círculo que enche com a sessão + anel semanal (âmbar a 80 %, vermelho fechado a 100 %). Claude pela statusline (`ask-agent --usage` → `POST /usage`), Codex pelos `rollout-*.jsonl`, Antigravity pela API de cota, Ollama local (`core/src/usage.rs`).
  - Seletor de workspace no topo da barra lateral (lista de projetos, "Abrir pasta…"); explorador sem a linha raiz, com atualizar e recolher tudo.
  - O diff do Git deixou de alargar a barra direita: rola dentro dela, como o Mapa.

- [x] **Tela dividida por tipo de agente.**
  - Uma janela por tipo (todos os Ollama juntos), numa grade automática (2 lado a lado, 2+1, 2×2, 3+2, 3×2); os tipos que não cabem viram abas da última janela.
  - Abas no cabeçalho de cada janela (saíram da topbar), "+" por janela, contorno na janela focada, Ctrl+Tab dentro da janela e Ctrl+Alt+setas entre janelas.
  - Divisores arrastáveis (duplo clique iguala), troca de lugar pela alça ⋮⋮ e botão liga/desliga; ordem, proporções e liga/desliga salvos em `Config`.
  - Cálculo puro e testado em `crates/app/src/split.rs`.
- [x] **Barra direita mais estreita.** Padrão de 380 para 330 px e mínimo de 300 para 280 px (quem tinha o padrão antigo salvo passa ao novo); o detalhe do download termina em reticências.
- [x] **Gemini removido.** Substituído pelo Antigravity em todo o projeto (catálogo de agentes, headless, prompts, ditado, testes e documentos). Só os filtros da pasta `.gemini` ficaram: é a configuração do próprio Antigravity.

- [x] **Aba Git e ícones no cabeçalho da barra direita.**
  - Conversa, Mapa e Git viraram ícones (balões, diagrama, branch) com dica ao passar o mouse; o número de mensagens novas fica sobre o ícone da Conversa.
  - Git local do workspace (`core/src/git.rs`): status, diff, preparar/tirar/descartar, commit (e corrigir o último), branches, buscar/puxar/enviar e `git init`, sempre sem shell e sem prompts que travam. Testado com o git real em `tests/git.rs` (inclusive conflito e push/pull num remoto local).
  - Chip da branch na topbar; releitura automática por `CoreEvent::GitChanged` (watcher do `.git/`).

- [x] **Barra lateral direita fixa com Conversa e Mapa.**
  - A Conversa integrada, o compositor e o menu "Enviar para" saíram de baixo dos terminais para uma barra à direita que não recolhe e tem largura ajustável (salva em `Config.right_panel_width`; `bottomHeight`/`feedCollapsed` antigos são ignorados).
  - Compositor redesenhado para a barra estreita; menus ficam dentro da tela e rolam quando não cabem (corrigido também o `constrain` do menu "+", que fechava o menu no clique).
  - **Mapa** em texto de terminal com quem conversa com quem agora (`Core::conversation_map` + `agent_map::render_map`), alargando a barra sobre o terminal sem redimensioná-lo.
  - Uma pergunta com resposta atrasada (202) deixou de causar falsos "ciclo de espera".

- [x] **Segurança da ponte e das abas.**
  - A colagem nas abas remove ESC e caracteres de controle: uma mensagem não fecha mais o *bracketed paste* para virar teclas em outra aba.
  - Cada processo de agente tem token próprio; a ponte sabe quem pergunta pelo token, não pelo `from`, e só a instância perguntada responde (`ask-agent --reply <n>`).
  - Ids validados (sem aspas, espaços nem controle).
  - No Windows, shims `.cmd` não recebem texto livre na linha de comando.
  - Limites: 16 MB por requisição, 128 requisições simultâneas, `Host` obrigatório, token comparado em tempo constante.
- [x] **Profundidade e perguntas órfãs entre abas.** O limite de profundidade vale também no modo abas (medido pela cadeia de perguntas pendentes), e fechar a aba de quem perguntou descarta as perguntas dela.
- [x] **Entrega previsível nas abas** (`crates/core/src/delivery.rs`, com testes).
  - O prazo conta desde a entrada na fila.
  - Nada é digitado logo após uma tecla do usuário (nem com texto começado).
  - Hooks do Claude/Codex marcam início e fim de turno.
  - O Enter sai em ordem com as teclas do usuário.
  - A resposta lenta é entregue depois na aba de quem perguntou, em vez de prender o `ask-agent` (que estourava o timeout de 2 min do Bash do Claude).
- [x] **Codex e OpenCode** no catálogo de agentes (abas e headless).
- [x] **Ambiente isolado.** Nenhuma `COMMANDDECK_*` herdada vaza para os agentes, e uma segunda janela na mesma pasta de dados é recusada com uma mensagem clara.
- [x] **Testes independentes da máquina** (`StartOptions::system_dirs = false`), teste do spawn do Claude corrigido, `rustfmt.toml` + `cargo fmt --check` na CI.

- [x] **Download do Whisper robusto** (motor e modelo): retomada por `Range`, retry com espera crescente, detecção de conexão parada, redirecionamentos, verificação de SHA-256 e barra de progresso no cabeçalho da Conversa integrada; suporte a Linux. Módulo `crates/core/src/download.rs`.
- [x] **Mouse no terminal** (`crates/app/src/terminal/view.rs`, `input.rs`): clique, soltar, arrasto e movimento são enviados ao programa quando ele ativa os modos 1000/1002/1003, em SGR (1006) ou no formato legado; Shift devolve o mouse ao terminal para selecionar texto. Coberto por testes headless com eventos reais do egui.
- [x] **IME no terminal**: com o terminal focado o IME do sistema é habilitado ancorado no cursor; a confirmação vai ao processo como texto digitado e a pré-edição aparece sublinhada sobre o cursor (antes o evento era descartado). Falta só testar com um IME real (teclado CJK, `Win+.`).
- [x] **Instâncias com identidade e conversa entre abas reais.** Cada aba é `modelo[@cargo[@nome]]`; `ask-agent` digita a mensagem no terminal da aba (abrindo-a se preciso, inclusive um segundo Claude) e a resposta volta por `ask-agent` de volta, sem raspar a tela; ciclos de espera são recusados e fechar a aba libera quem esperava. Verificado ao vivo com duas abas do Claude, Claude ↔ Antigravity, dois Ollama (4B e 9B) e um orquestrador delegando a `claude@dev` e `claude@design`. Ver `ARCHITECTURE.md`.
- [x] **Colar no Windows derrubava o app** (corrupção de heap): o clipboard agora só é lido na thread da interface. Reproduzido e corrigido.
- [x] **Validação no Windows** (feita à mão): abas, ConPTY, microfone/Whisper, som, arrastar arquivos, colar imagem e restauração de abas.

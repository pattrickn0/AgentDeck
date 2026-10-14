# Pendências

Lista de melhorias e limitações conhecidas, em ordem sugerida de prioridade. Ao concluir um item, mova-o para
"Concluído" com uma linha do que foi feito (regras dos documentos em [GERAL.md](GERAL.md)).

## 0. Direção: IA local como serviço interno (adiado)

> **Adiado** até o projeto estar mais completo.

Decisão: com Claude e Antigravity disponíveis, o Ollama deixa de ser "agente par" e vira **infraestrutura** do app.

- [ ] **Embeddings locais para RAG.** Modelo de embeddings no Ollama (`nomic-embed-text` ou `bge-m3`, < 1 GB, cabe na RX 7600 8 GB).
  - Indexar o workspace em pedaços (*chunks*) e manter o índice atualizado pelo watcher existente (`workspace.rs`).
  - Expor a busca de duas formas: `ask-agent`/CLI `search` (sob demanda) e um `ContextProvider` com orçamento fixo de tokens (ex.: 1.500).
  - Índice local (ex.: em `dados/`), sem enviar nada para fora.
- [ ] **Compressor de contexto.** Modelo pequeno (`qwen3:8b`, 100% GPU) para resumir:
  - o histórico das **mesas**: hoje cada rodada reenvia a transcrição inteira (`table.rs`); passar a enviar um resumo + as últimas falas;
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
- [ ] **Validar ao vivo no Windows a barra direita e o Mapa**: arrastar a borda, alternar Conversa/Mapa com duas abas conversando de verdade (estados na fila / digitada / 202), e se os símbolos `─│├└►●○×` alinham na fonte mono do Windows. Feito só por testes e capturas no Linux (Xvfb).
- [ ] Atualizar `docs/screenshots/main.png` com a barra direita.
- [ ] Janela estreita (perto do mínimo de 820 px): com muitas abas, as de trás encolhem até virar só o ponto colorido.

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.

## 5. Validação

- [ ] **Validar no Windows, ao vivo, o que mudou na ponte** (feito só por testes no Linux e `cargo check`/clippy para `x86_64-pc-windows-gnu`): hooks do Claude (`--settings`) avisando início/fim de turno, `notify` do Codex, agentes instalados como shim `.cmd` (headless com prompt por arquivo, abas com `cmd_safe_arg`), resposta atrasada digitada na aba de quem perguntou e a mensagem de "outra janela usando a pasta de dados".
- [ ] **Testar o IME com um IME real** (teclado CJK, seletor de emoji `Win+.`): a lógica está ligada, mas só foi verificada por compilação e testes do cursor; não foi exercitada com composição de verdade.

## 6. Novas funcionalidades (comparação com o Orca)

- [ ] **Painéis divididos (split panes).** Ver duas ou mais abas lado a lado (horizontal/vertical), cada uma com o próprio terminal.
  - Hoje há um foco único: `Deck::active` recebe o teclado, `fit_active` só redimensiona a aba ativa e `draw_terminals` (`crates/app/src/ui/mod.rs`) desenha uma aba só.
  - Proposta: uma árvore de layout (painel = aba, nó = divisão com proporção) com painel focado, `fit` por painel e arrastar a divisória. `terminal::view::show` já recebe o retângulo, então o desenho por painel é direto.
  - Persistir o layout em `Config` junto com `open_tabs`. Atalhos para dividir, fechar e trocar de painel.
  - Esforço estimado: médio. Fica mais simples se `deck.rs` (1300 linhas) for dividido antes (abas / entrega / compositor / layout).
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

- [ ] Só o Claude abre com o comando `ask-agent` pré-autorizado (`--allowedTools`). No Antigravity funcionou sem aprovação no modo atual (`accept-edits`), mas outros agentes (codex, opencode, gemini, aider, goose, copilot, q, interpreter) não foram testados e podem pedir aprovação na própria aba.
- [ ] Se o agente da aba não responder por `ask-agent` (modelo fraco, ou o usuário assumiu o terminal), a pergunta expira em 10 minutos. Quem perguntou não fica mais bloqueado (recebe o aviso por escrito no terminal), mas só o `ollama-agent` responde sozinho.
- [ ] Hooks de início/fim de turno só no Claude e no Codex (este só avisa o fim, e só quando não é shim `.cmd`). Antigravity e os demais seguem a heurística da saída (1,5 s parado / 30 s na fila).
- [ ] Falta `ask-agent --cancel <n>` para desistir de uma pergunta (hoje ela expira em 10 min).
- [ ] A ponte não percebe quando o cliente (`ask-agent`) cai no meio da espera (limitação do `tiny_http`); a pergunta fica pendente até ser respondida ou expirar.
- [ ] No headless, gemini (`--approval-mode yolo`), aider (`--yes-always`) e interpreter (`-y`) rodam sem aprovação recebendo texto de outros agentes. Avaliar modos mais restritos.
- [ ] As mesas (`table.rs`) e a ponte sem interface ainda usam o modo headless: não aparecem nas abas.
- [ ] Os testes de integração do fluxo (`a_question_is_typed_into_the_tab_and_answered_by_callback` e os vizinhos) só rodam em Unix (agentes falsos em shell); no Windows foram verificados ao vivo.
- [ ] Não há como o usuário dar cargo a uma aba aberta à mão (hoje só duplicatas ganham nome, tipo `claude@main@ana`).

## Concluído

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

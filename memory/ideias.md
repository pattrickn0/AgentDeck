# Ideias e limitações conhecidas

O que **não** está entre os próximos passos. Não é compromisso: uma ideia só vira trabalho quando entra em
[pendencias.md](pendencias.md), que tem no máximo 10 itens (revisão com o usuário em 2026-10-24). Ao entrar lá, sai daqui.
Uma limitação que passar a incomodar no uso também sobe para lá.

## Ideias

### Mesa de tarefas e modelos

- **Mensagens ao agente não citam a validação:** `start_message` não conta que, depois de mesclada, a tarefa ainda pode
  voltar reprovada; ele só descobre quando `reject_message` chega. Conferir no uso se faz falta.
- **Rodar os testes do projeto ao receber o relatório**; **editar uma tarefa em rascunho**.
- **"Como validar" no pedido** (do OpenSpec, 2026-10-24): uma seção opcional com cenários QUANDO/ENTÃO; o relatório diz
  qual teste cobre cada um, e "aguardando validação" os mostra como caixas de marcar (o formato do `testes.md`).
  Opcional: obrigatório viraria burocracia nas tarefas pequenas.
- **Skills do projeto para os 4 CLIs oficiais** (2026-10-24): cada CLI procura skills numa pasta (`.claude/skills`,
  a do Antigravity a confirmar, `.agents/skills` no Codex, `.opencode/skills`); o AgentDeck entregaria uma pasta só a todos.
- **Seletor de modelos só com os que o plano permite** (`t14`): a lista vem de `agy models`/`opencode models` (o que o CLI
  conhece, não o que o plano permite). Em 2026-10-24 o usuário disse que, na prática, já funciona assim; reabrir se um
  modelo sem acesso aparecer. Se for feito, o mesmo filtro vale para `Core::models_inventory`.
- **Modelo por instância, depois:** trocar o modelo de uma aba aberta; lista de modelos do Codex no "+ Novo agente";
  campo livre para modelo fora da lista; editar `roleModels` pela interface.

### Modo Documentos, fase 2

- Diff lado a lado; gravar as abas de documento (somem ao fechar o app); documento ao lado de um agente (ver
  [GERAL.md](GERAL.md), "Documentos").
- Editor (`t39`, de fora de propósito): recuo automático, substituir (`Ctrl+H`), ir para a linha (`Ctrl+G`), menu de
  clique direito no diff e no arquivo grande, rolagem horizontal, CRLF/LF misturados e fora do UTF-8 (hoje só leitura),
  busca no markdown formatado e no pedido/relatório, busca no diff por ocorrência, realce dentro de bloco de código do
  markdown, mais linguagens (ver `memory/aprendizados/editor-de-documentos.md` antes de trocar por `syntect`).
- Explorador: marcar o que mudou (git), filtrar/buscar, abrir os arquivos de uma tarefa pelo detalhe dela.
- Diff: o hash de layout (`content_hash`, `ui/diff_view.rs`) varre o diff inteiro a cada quadro; calcular uma vez ao
  carregar se um diff de vários MB pesar.

### Interface

- **Inglês na interface** (estava em `pendencias.md`; saiu em 2026-10-24 para os itens de Windows e Linux caberem).
- **Temas** (pedido do usuário, 18/10; deixado de lado em 2026-10-24): trocar as constantes de `theme.rs` por uma paleta
  em tempo de execução, escolhida nas configurações, incluindo o fundo e as 16 cores do terminal (`terminal/colors.rs`).
- Emojis coloridos (o egui só tem monocromáticos); animação de hover no estado vazio.
- Tela dividida: arrastar uma aba para outra janela, maximizar uma janela, atalho de teclado com a barra lateral recolhida,
  cache por aba se o redesenho pesar com 5–6 janelas.
- Projetos: a árvore e o Git guardarem o estado por projeto; lógica da `Deck` em funções puras testáveis (como `split.rs`);
  testes de interface (egui) para a barra direita, a aba Git e a tela dividida.
- Uso do Antigravity: renovar o token em memória quando o CLI está fechado (hoje "sem dados" depois de ~1 h); aviso
  discreto quando a API interna falha por muito tempo.

### Memória

- `--outline` em JS/TS, Python e PowerShell/shell; medir no uso se os agentes rodam `--learn`/`--outline`.
- `ContextProvider` geral (adiado em 2026-10-23; reabrir só se a medição mostrar que os agentes não pedem), e RAG por ele.
- Perfis de agente em arquivo (`prompts/_comum.md` e `prompts/<agente>.md`), editáveis sem recompilar.
- Situação atual com decisões recentes; aviso de `touches` que aponta para arquivo renomeado/removido e um jeito de
  aposentar aprendizado; aprendizado novo de uma branch visível antes da mescla.

### Ponte e GitHub

- `ask-agent --cancel <n>`; reentregar sozinha a resposta que ficou no `--inbox`; pré-autorizar o `ask-agent` nos CLIs que
  pedem aprovação na aba (hoje só o Claude abre com `--allowedTools`).
- No headless, aider (`--yes-always`) e interpreter (`-y`) rodam sem aprovação: avaliar modos mais restritos.
- GitHub pelo app: comentar e aprovar PR, criar issue, reexecutar job; exercitar o caminho `gh auth token`.
- `commit.gpgsign` com pinentry gráfico: deixar o diálogo de senha abrir em vez de recusar.

### Sistemas

- Linux: abrir de verdade no Wayland (GNOME, KDE) e num Fedora Workstation; WSLg pelo X11 (janela maximizada, escala,
  cota pelo chaveiro; [LINUX.md](LINUX.md), 2.8); pacotes (`.deb`, `.rpm`, Flatpak, AppImage, `.desktop`); ditado com GPU;
  aviso quando falta `xdg-desktop-portal`/`xdg-utils`; textos de Git/GitHub próprios do Linux.
- **Celular** (bem mais para frente, com o app estável): acompanhar e falar com os agentes longe do computador.

## Limitações conhecidas

Para quem for mexer no código; não são tarefas.

- **Claude Code duplica a conversa no histórico a cada mudança de largura** (Windows): ele limpa só a tela e reimprime,
  sem `ESC[3J`. O PTY só muda com o tamanho parado por 150 ms (um arrasto = uma cópia). Sobra também a repetição de 1 a
  3 linhas no histórico nas mudanças de largura. Ver `memory/aprendizados/claude-code-sem-esc3j-no-windows.md`.
- **A ponte não percebe o cliente (`ask-agent`) cair** no meio da espera (`tiny_http`); a pergunta fica pendente até ser
  respondida ou expirar em 10 min. A resposta não entregue fica em `ask-agent --inbox`.
- **Agente da aba que não responde por `ask-agent`** (modelo fraco, ou o usuário assumiu o terminal): a pergunta expira em
  10 minutos, com aviso por escrito a quem perguntou.
- **Hooks de turno só no Claude e no Codex**; os demais seguem a heurística da saída (1,5 s parado / 30 s na fila).
- **As instruções da ponte são fixadas na partida**: um agente aberto antes de uma versão nova não sabe dos comandos novos
  até reabrir a aba; o aviso de equipe só chega com a próxima mensagem.
- **A ponte sem interface usa o modo headless** (não aparece nas abas).
- **O `AGENTS.md` de projetos não ativos** só é reescrito quando se abre a próxima aba deles.
- **A cota do Antigravity vem de uma API interna** (`v1internal:retrieveUserQuotaSummary`) que pode mudar sem aviso; o
  **uso do Codex** só foi testado com arquivos de exemplo.
- **Select, spinner e tooltips desenhados à mão**; o desfoque atrás do overlay de arrastar é aproximado.
- **Fora do rumo** (2026-10-23): o agente Ollama próprio e a "IA local como serviço interno"; um modelo local entra por um
  CLI que o suporte (ver [AGENTDECK.md](AGENTDECK.md), "O que o AgentDeck é e não é").

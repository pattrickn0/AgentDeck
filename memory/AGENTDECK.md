# AgentDeck: visão e rumo

O **AgentDeck** (antes CommandDeck) é o ambiente de desenvolvimento dos agentes de IA. Desde 2026-10-17 o
produto é conduzido pelo Claude (o orquestrador); o usuário acompanha como cliente e colaborador e é quem aprova o
que vai para a `main` (a `main` é a versão estável; o trabalho do dia a dia é na `dev`).

## Para quem é

1. **Para os agentes**, primeiro. Um agente aqui deve conseguir receber uma tarefa, trabalhar isolado sem pisar nos
   outros, saber o que o projeto já decidiu, pedir ajuda a outro agente e entregar algo revisável. O explorador, o
   Git, o GitHub e a memória existem para isso.
2. **Para o humano**, que precisa enxergar o que está acontecendo sem ler terminais inteiros: o que cada agente está
   fazendo, o que ficou pronto, o que espera revisão e o que deu errado.

## Princípios

- **A unidade de trabalho é a tarefa, não a mensagem.** Uma tarefa tem um pedido, um agente com um modelo, uma
  branch isolada, um relatório, um diff e testes. Conversa entre agentes é meio, não fim.
- **Tudo visível.** Agente que trabalha aparece numa aba; nada de trabalho invisível que o humano não consegue acompanhar.
- **Isolamento por padrão.** Cada tarefa numa worktree própria: agentes em paralelo não se atrapalham, e descartar
  uma tentativa não custa nada.
- **O modelo certo para cada trabalho.** Opus para orquestrar e decidir, Sonnet para desenvolver, modelos gratuitos
  (OpenCode) para revisão e perguntas, sempre com os achados conferidos. A cota das assinaturas é o recurso escasso.
- **A memória mora em arquivos.** O que o projeto decidiu e aprendeu fica em markdown versionado (esta pasta), e o
  app entrega o pedaço certo a cada agente na hora certa.
- **Nada de mágica que trava.** Sem prompts escondidos, sem diálogo que congela, sem processo órfão; erro com explicação.

## O que o AgentDeck é e não é (usuário, 2026-10-23)

- **100% Rust**, sem outra tecnologia (nada de Electron, Tauri ou web). Sistemas: **Windows e Linux**; macOS não está no rumo.
- **Gira em volta dos CLIs de agente** (Claude Code, Antigravity, Codex, OpenCode e os que existirem), de nuvem ou não: o
  que estiver instalado é usado. **Não é um app de modelo local**: o agente Ollama próprio (`ollama-agent`) sai; um modelo
  local continua possível através de um CLI que o suporte (ex.: o OpenCode).
- **Agentes visíveis**: quando um agente precisa de outro, abre uma instância no AgentDeck (aba) pelo `ask-agent` ou pela
  mesa de tarefas, **não um subagente interno do CLI**, a não ser que o usuário peça explicitamente.
- **Três memórias**: a **geral** (como usar o AgentDeck, igual para todo mundo, entregue pelo app a todo CLI em qualquer
  projeto); a **do projeto** (`memory/aprendizados/` do repositório do usuário, versionada nele); e a **local** (a do
  usuário e dos CLIs, que nunca vai para o git).
- Licença **MIT**. Hoje só o usuário desenvolve; contribuições de outras pessoas, talvez depois de publicado.
- **Depois da `main`**, nesta ordem aproximada: mais revisão e estabilidade, o **instalador**, inglês na interface,
  temas. **Bem mais para frente**, com o app bem estável: acompanhar e falar com os agentes **pelo celular**.
- Não fazer por enquanto: comentários linha a linha no diff para devolver ao agente (o "devolver com texto" basta).

## Rumo (ordem decidida pelo orquestrador)

1. ~~Modelo de LLM por instância~~ (feito em 2026-10-17).
2. ~~**Mesa de tarefas**~~ (fases A e B feitas em 2026-10-17; a etapa "aguardando validação" veio na `t10`; o que sobrou está em
   [pendencias.md](pendencias.md), bloco "Próximo") no lugar da Conversa integrada: criar, iniciar (worktree + branch + aba do agente com modelo),
   acompanhar, receber o relatório, revisar o diff, mesclar, validar (ou reprovar) ou descartar com um botão. É o fluxo que o
   orquestrador fazia à mão.
3. **Memória em duas camadas, nossa.** **Fase 1 feita** (tarefas `t13`, `t15` e `t16`): `memory/aprendizados/*.md` com `touches`, entregues
   no início de cada tarefa e por `ask-agent --learn`; seção "Situação atual" (outras tarefas, os arquivos delas, inclusive os ainda sem commit, e os commits da base); histórico de
   feedbacks e aprendizados dos arquivos citados nas devoluções e reprovações; `ask-agent --learn` com e sem arquivos. Como funciona: [ARCHITECTURE.md](ARCHITECTURE.md) ("Memória em duas camadas"). **Fase 2** (resumo
   de API por arquivo, injeção nas abas que não são de tarefa, `ContextProvider` geral): [pendencias.md](pendencias.md), "Memória (fase 2)".
   O plano original, para referência: Longo prazo: "aprendizados" em markdown versionado no repositório, com
   frontmatter que diz que arquivos tocam (`touches`), entregues ao agente quando a tarefa dele mexe nesses arquivos;
   e um resumo da API de um arquivo (estilo `skeleton`) para o agente ler menos. Curto prazo: o pedido da tarefa, as
   decisões e a atividade recente entregues pelo app (o "ponto único de injeção de contexto", em "Memória (fase 2)" de
   [pendencias.md](pendencias.md)).
   - **Trail-Code avaliado em 2026-10-17 e não adotado como dependência.** O `skeleton` é bom (−87% de tokens), mas
     o `ask` é léxico e não ganhou do `grep`, o `callers` não resolve chamadas de método entre crates em Rust, o `init`
     escreve hooks no `~/.claude` global (afeta todos os repositórios) mesmo com `--no-global`, e toda saída dele
     manda o agente anunciar ao usuário os tokens economizados (texto de terceiro dirigindo o agente). Ficamos com as
     ideias: aprendizados versionados com `touches` e o resumo de API.
4. **Diff legível para o usuário validar** (combinado em 2026-10-21): hoje o diff mora espremido na barra direita. O centro
   passa a ter "abas de documento" ao lado das abas de terminal, e o diff de uma tarefa (e depois o editor de arquivos e um
   explorador mais útil) abre nelas, em largura de tela cheia. É o que falta para o humano validar sem sair do app.
   Detalhes e subitens: [pendencias.md](pendencias.md), bloco "Próximo", item b).
5. **Renomear para AgentDeck e identidade visual** (combinado em 2026-10-22). **Feitos**: o nome (tarefa `t20`, com migração
   dos nomes antigos: ver [MANUTENCAO.md](MANUTENCAO.md), seção 6) e o visual (`t21`–`t23`). Os **temas** ficam para depois
   da `main` ([pendencias.md](pendencias.md), bloco "Próximo").
6. **Substituir a `main` pela `dev`** (aprovado pelo usuário em 2026-10-22). A `main` é o piloto em TypeScript; a `dev` é a
   reescrita em Rust, que o usuário usa todo dia. A `main` é trocada inteira pela `dev` (sem mesclagem), depois da `t29`
   (Linux), de uma tarefa de validação e da correção da janela maximizada.

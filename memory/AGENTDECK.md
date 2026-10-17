# AgentDeck: visão e rumo

O CommandDeck está virando o **AgentDeck**: o ambiente de desenvolvimento dos agentes de IA. Desde 2026-10-17 o
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

## Rumo (ordem decidida pelo orquestrador)

1. ~~Modelo de LLM por instância~~ (feito em 2026-10-17).
2. **Mesa de tarefas** no lugar da Conversa integrada: criar, iniciar (worktree + branch + aba do agente com modelo),
   acompanhar, receber o relatório, revisar o diff e mesclar ou descartar com um botão. É o fluxo que hoje o
   orquestrador faz à mão.
3. **Memória em duas camadas.** Longo prazo: avaliar o Trail-Code (MIT, arquivos, mapa do código + "aprendizados"
   versionados; hooks no Claude Code e MCP para os outros). Curto prazo: o pedido da tarefa, as decisões e a
   atividade recente entregues ao agente pelo app (o "ponto único de injeção de contexto" da seção 2 de
   [pendencias.md](pendencias.md)).
4. **Renomear para AgentDeck** (executáveis, variáveis `COMMANDDECK_*`, pasta de dados, marcadores do `AGENTS.md`),
   com migração, depois da mesa de tarefas.

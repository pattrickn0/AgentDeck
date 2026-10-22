<!-- agentdeck:start -->
Você está rodando dentro do AgentDeck, um ambiente que conecta vários agentes de IA. Seu nome aqui está na variável de ambiente AGENTDECK_AGENT; nunca pergunte a si mesmo.
Para conversar com outro agente, execute no shell:
  ask-agent <agente> "sua mensagem"
Agentes disponíveis: claude,antigravity,opencode,ollama (deepseek-r1:14b),ollama (qwen3.5:4b),ollama (qwen3.5:9b). A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms). Se o outro agente demorar, o ask-agent termina antes, avisando que a resposta será digitada depois neste terminal como uma nova mensagem: nesse caso, não repita a pergunta.
Para saber que agentes e modelos este computador tem, rode ask-agent --models (ou ask-agent --models <agente>) antes de escolher --agent/--model.
O histórico de cada par de agentes é mantido entre chamadas. O usuário tem acesso à "Conversa integrada" em tempo real no AgentDeck, portanto você NÃO precisa relatar nem resumir a conversa dos agentes para ele.
Ao colaborar com outro agente (ex: Antigravity), mantenha o diálogo diretamente com ele (iterando, tirando dúvidas e alinhando) até concluir a tarefa, em vez de parar no meio para fazer perguntas ao usuário.
Cada aba é uma instância com nome <modelo>[@<cargo>[@<nome>]] (ex.: claude, claude@dev, claude@dev@ana). Para delegar a uma instância com cargo, chame-a pelo nome: ask-agent claude@dev "tarefa" (a aba é aberta se não existir); use --new para abrir outra instância do mesmo cargo; ask-agent --model <modelo> claude@dev "tarefa" escolhe o LLM da instância ao abri-la (ex.: sonnet). Trabalho grande vira uma tarefa, com worktree, branch e agente próprios: ask-agent --task new "título" --start --file <pedido.md> (o pedido num arquivo preserva os acentos; pela entrada padrão o shell do Windows os troca por "?").
Quando outro agente lhe mandar uma mensagem, responda executando o comando indicado nela (ask-agent --reply <número> "sua resposta"; em textos longos ou com acentos, escreva-os num arquivo e use --file <arquivo>): é só assim que a resposta chega a ele.
Os agentes formam uma equipe: cada um pode ter um chefe e subordinados, e o usuário pode reorganizá-la a qualquer momento. Antes de delegar ou de reportar, consulte a sua posição com ask-agent --team; delegue de preferência à sua equipe e, ao concluir algo que o seu chefe pediu, reporte a ele com ask-agent --boss "mensagem".
<!-- agentdeck:end -->

## Memória do projeto

A documentação e a memória do projeto ficam em `memory/`, versionadas no git. Leia antes de mexer no que não conhece:

- `memory/GERAL.md`: ponto de partida, com a visão geral e o mapa dos outros documentos e das regras de cada um.
- `memory/AGENTDECK.md`: visão e rumo do produto (o AgentDeck) e quem decide.
- `memory/ARCHITECTURE.md`: como o código está organizado (crates, módulos, fluxos).
- `memory/MANUTENCAO.md`: como compilar, testar e manter.
- `memory/pendencias.md`: o trabalho que falta, em ordem de prioridade.
- `memory/testes.md`: roteiro de testes manuais no Windows.

A memória funciona em duas camadas:

- **Aprendizados** (longo prazo): `memory/aprendizados/`, um arquivo por armadilha, decisão ou convenção que não é óbvia no código. O formato e quando criar estão em `memory/aprendizados/README.md`. Para ver os que valem para os arquivos em que você vai mexer, rode `ask-agent --learn <arquivo> [<arquivo>...]`.
- **Situação atual** (curto prazo): no início de cada tarefa, o app entrega as outras tarefas em andamento, os arquivos que elas mexem e os últimos commits da base. Fora de tarefa, consulte `ask-agent --task list` e `git log`.

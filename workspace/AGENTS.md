<!-- agentdeck:start -->
Você está rodando dentro do AgentDeck, um ambiente que conecta vários agentes de IA. Seu nome aqui está na variável de ambiente AGENTDECK_AGENT; nunca pergunte a si mesmo.
Para conversar com outro agente, execute no shell:
  ask-agent <agente> "sua mensagem"
Agentes disponíveis: claude,antigravity. A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms). Se o outro agente demorar, o ask-agent termina antes, avisando que a resposta será digitada depois neste terminal como uma nova mensagem: nesse caso, não repita a pergunta.
O histórico de cada par de agentes é mantido entre chamadas. O usuário tem acesso à "Conversa integrada" em tempo real no AgentDeck, portanto você NÃO precisa relatar nem resumir a conversa dos agentes para ele.
Ao colaborar com outro agente (ex: Antigravity), mantenha o diálogo diretamente com ele (iterando, tirando dúvidas e alinhando) até concluir a tarefa, em vez de parar no meio para fazer perguntas ao usuário.
Cada aba é uma instância com nome <modelo>[@<cargo>[@<nome>]] (ex.: claude, claude@dev, claude@dev@ana). Para delegar a uma instância com cargo, chame-a pelo nome: ask-agent claude@dev "tarefa" (a aba é aberta se não existir); use --new para abrir outra instância do mesmo cargo.
Quando outro agente lhe mandar uma mensagem, responda executando o comando indicado nela (ask-agent --reply <número> "sua resposta"): é só assim que a resposta chega a ele.
<!-- agentdeck:end -->

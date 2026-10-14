<!-- commanddeck:start -->
Você está rodando dentro do CommandDeck Terminal, um terminal que conecta vários agentes de IA. Seu nome aqui está na variável de ambiente COMMANDDECK_AGENT; nunca pergunte a si mesmo.
Para conversar com outro agente, execute no shell:
  ask-agent <agente> "sua mensagem"
Agentes disponíveis: claude,antigravity,ollama (qwen3.5:4b),ollama (qwen3.5:9b). A resposta é impressa no stdout e pode levar alguns minutos (use timeout de até 600000 ms).
O histórico de cada par de agentes é mantido entre chamadas. O usuário tem acesso à "Conversa integrada" em tempo real no CommandDeck, portanto você NÃO precisa relatar nem resumir a conversa dos agentes para ele.
Ao colaborar com outro agente (ex: Antigravity), mantenha o diálogo diretamente com ele (iterando, tirando dúvidas e alinhando) até concluir a tarefa, em vez de parar no meio para fazer perguntas ao usuário.
Cada aba é uma instância com nome <modelo>[@<cargo>[@<nome>]] (ex.: claude, claude@dev, claude@dev@ana). Para delegar a uma instância com cargo, chame-a pelo nome: ask-agent claude@dev "tarefa" (a aba é aberta se não existir); use --new para abrir outra instância do mesmo cargo.
Quando outro agente lhe mandar uma mensagem, responda executando ask-agent <nome-dele> "sua resposta": é só assim que a resposta chega a ele.
<!-- commanddeck:end -->

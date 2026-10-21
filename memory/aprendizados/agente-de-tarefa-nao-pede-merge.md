---
titulo: O agente de uma tarefa não pede merge
resumo: O trabalho do agente termina no --task done; mesclar é de quem revisa.
touches: []
---
O agente de uma tarefa só consulta tarefas e entrega o relatório da própria (`ask-agent --task done`). Não pede a
ninguém, agente ou usuário, que mescle nem roda `--task merge`; a ponte recusa (403) qualquer outra ação de tarefa
vinda dele (`task_permission`, `bridge.rs`).

**Por quê:** no primeiro uso real da mesa o `opencode@t2` pediu ao Claude que mesclasse a própria tarefa; a decisão
de mesclar é de quem revisa o diff, não de quem o escreveu.
**Como aplicar:** termine com o relatório e espere. Se faltar algo, pergunte ao chefe com `ask-agent --boss`.

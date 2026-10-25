---
titulo: Resposta de ask-agent --reply que some - onde olhar
resumo: O app grava dados/agentdeck.log; a resposta que não chegou fica em ask-agent --inbox e a Conversa integrada avisa.
touches: [crates/core/src/bridge.rs, crates/core/src/inbox.rs, crates/app/src/deck/**, crates/app/src/logfile.rs]
---
Uma resposta atrasada (depois do 202) só chega a quem perguntou se a interface a digita na aba dele; ela se perde sem
erro se a aba não está aberta ou viva, e a resposta do 200 também se perde se o cliente já foi embora (o `tiny_http` não
percebe). Antes da `t27` o app só escrevia o log no stderr, que não existe num app de janela no Windows: a causa da
resposta #1 do Antigravity nunca foi descoberta.

**Por quê:** "o 200 foi escrito" não prova que o agente leu; "foi para a fila da aba" também não prova que foi digitada.
Por isso a caixa de entrada (`core/src/inbox.rs`) só marca "entregue" no fim: o 200 sem erro, ou a digitação no terminal.
**Como aplicar:** ao investigar uma resposta perdida, leia `dados/agentdeck.log` (procure `resposta atrasada #N`) e peça
ao agente que perguntou `ask-agent --inbox --all`. Quem mexer na entrega nas abas deve manter uma destas duas saídas
para toda resposta: digitada (`Core::answer_typed`) ou avisada (`Core::answer_lost`); nunca descartar em silêncio.

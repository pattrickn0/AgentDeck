---
titulo: O historico.md mescla por união
resumo: .gitattributes marca memory/historico.md com merge=union; não tire, e não edite entradas antigas.
touches: [memory/historico.md, .gitattributes]
---
O `.gitattributes` marca `memory/historico.md` com `merge=union`: quando duas branches acrescentam linhas no mesmo
lugar, o git guarda as dos dois lados em vez de parar com conflito.

**Por quê:** toda tarefa põe a sua entrada no topo do histórico, então duas tarefas em paralelo conflitavam ali em
toda mesclagem (t33, t34, t35 e t39 em 23/10; t37 e t38 iam pelo mesmo caminho). O usuário via a recusa "conflito ao
mesclar" a cada tarefa.
**Como aplicar:** o arquivo só recebe entradas novas; não reescreva nem apague as antigas, porque a união não
detecta conflito de verdade e manteria as duas versões. A entrada de uma tarefa pode ficar abaixo das que entraram
depois que ela começou: é esperado. Não use `union` em arquivos que também removem linhas (`pendencias.md`, código).

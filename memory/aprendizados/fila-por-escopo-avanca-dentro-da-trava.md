---
titulo: A fila por escopo avança dentro da trava das operações e Validating não a segura
resumo: A tarefa que sai de Running/Review libera a fila por advance_queue_locked (sem retomar a trava); aguardando validação não segura ninguém.
touches: [crates/core/src/tasks.rs, crates/core/src/scope.rs]
---
Uma tarefa com escopo (`Task::scope`, lista de caminhos; `scope.rs`) que se sobrepõe ao de outra `Running`/`Review` vai a
`Queued` em `task_start`. Quem a tira de lá é `Core::advance_queue_locked`, chamado no fim de `merge_task` (sucesso),
`task_discard` e de um `task_start` que falhou.

**Por quê:** `TaskStore::op` é um `parking_lot::Mutex` (não reentrante) tomado por toda operação do ciclo de vida. Chamar
`task_start` de dentro de `merge_task` travaria para sempre; por isso existem `start_locked` e `advance_queue_locked`, que
assumem a trava já tomada. `Validating` fica fora de quem segura a fila de propósito: o código já está na base, a seguinte parte
dele, e esperar o teste do usuário travaria o paralelismo (a reprovação só deixa um aviso no histórico se voltar sobreposta).
`memory/**` e `Cargo.lock` (`scope::UNRESERVED`) não entram na sobreposição nem no aviso de "fora do escopo".

**Como aplicar:** uma operação nova que tire uma tarefa de `Running`/`Review` (ou que a faça falhar) deve chamar
`advance_queue_locked` no fim, ainda com a trava; nunca `task_start`/`tasks_advance_queue` de dentro de outra operação. A base da
tarefa que sai da fila é a de quando ela começa (a worktree nasce do tip da base naquele momento), não a da criação. A interface
não precisa mexer no `Deck::tick`: o início sozinho emite o mesmo `CoreEvent::Deliver` do botão Iniciar.

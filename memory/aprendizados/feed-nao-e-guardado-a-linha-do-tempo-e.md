---
titulo: O feed (Conversa integrada) não é guardado; a linha do tempo da tarefa é
resumo: Deck::feed vive só na memória (500 entradas); o que sobrevive é Task::messages, gravado em Core::feed.
touches: [crates/core/src/lib.rs, crates/core/src/tasks/**, crates/core/src/bridge.rs, crates/app/src/deck/**, crates/app/src/ui/tasks.rs]
---
A Conversa integrada (`Deck::feed`) é uma lista na memória da interface, com teto de `FEED_MAX`; fechar o app a apaga. A
linha do tempo de cada tarefa não pode depender dela: `Core::feed` (único caminho do `ask-agent`: `bridge::Caller::feed`,
`answer_lost`…) chama `record_task_message`, que grava em `Task::messages` no JSON das tarefas.

**Por quê:** guardar o feed inteiro misturaria todos os projetos e abas e cresceria sem fim; por tarefa o teto é claro
(`MAX_MESSAGES` × `MAX_MESSAGE_CHARS`) e os dados morrem com a tarefa. Só entram `Ask`, `Answer` e `Error` em que a
instância da tarefa fala ou ouve; `Thinking` é transitório, `System` não tem remetente e `User` nasce na própria interface
(`Deck::add_feed`), nunca passa pelo core.
**Como aplicar:** uma entrada nova da ponte tem de sair por `Core::feed` (com `.in_project(...)`: sem projeto ela não é
registrada) para aparecer na linha do tempo. A comparação é com o id sem projeto (`claude@t3`, nunca `claude@t30`). Não
chame `Core::feed` de dentro de `with_tasks`: o registro trava as tarefas de novo.

---
titulo: As tarefas ficam presas ao id do projeto, não à pasta
resumo: Remover e adicionar o projeto de novo gerava outro id e deixava as tarefas órfãs em dados/tarefas/<id antigo>.json; readicionar a mesma pasta agora reaproveita o id.
touches: [crates/core/src/tasks.rs, crates/core/src/config.rs, crates/core/src/lib.rs]
---
A mesa grava as tarefas em `dados/tarefas/<id do projeto>.json` (e os pedidos em `<id>-tNN-brief.md`). Remover o
projeto da lista não apaga esse arquivo, mas adicionar a mesma pasta de novo criava um id novo (`p` + hexadecimal) e a
mesa aparecia vazia. As worktrees (`.agentdeck/worktrees/`) mudam junto com a pasta, mas o campo `worktree` de cada
tarefa e o registro do git (`.git/worktrees/*/gitdir`) continuam no caminho velho.

**Por quê:** em 2026-10-24 a pasta `commanddeck-dev` virou `agentdeck`, o projeto foi removido e readicionado, e as 47
tarefas sumiram da mesa (o id foi de `pfdaeded2` para `p76d8a279`).

**Como aplicar:** desde a `t50` (2026-10-24), remover um projeto guarda o par id + pasta em `Config.removed_projects`
(`removedProjects` no `config.json`, camelCase, últimos 50; `Config::remove_project`) e `Config::add_project`
(`Core::create_project`) reaproveita o id quando a pasta volta, desde que nenhum projeto ativo já use esse id. A
comparação de pastas é própria (`same_folder`): `\` e `/` valem o mesmo, sem barra no fim e, só no Windows, sem
diferenciar maiúsculas. Por isso **não** compare pastas de projeto com `util::same_path` quando o que importa é "a
pasta que a pessoa digitou".

A recuperação manual (o parágrafo abaixo) fica só para projetos removidos antes dessa mudança, que não têm o par
guardado. Com o app fechado (ele guarda a lista em memória e regrava ao mexer numa tarefa): copie `<id antigo>.json`
para `<id novo>.json` (o id está em `projectList` do `%APPDATA%\AgentDeck\config.json`), troque o caminho velho no
campo `worktree`, renomeie os `<id antigo>-tNN-*.md` e rode `git worktree repair <worktrees>`; ou acrescente
`{"id": "<id antigo>", "path": "<a pasta>"}` a `removedProjects` e readicione a pasta pelo app, que é o caminho da
`t50`.

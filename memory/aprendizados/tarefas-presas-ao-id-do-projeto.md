---
titulo: As tarefas ficam presas ao id do projeto, não à pasta
resumo: Remover e adicionar o projeto de novo gera outro id; as tarefas antigas ficam órfãs em dados/tarefas/<id antigo>.json.
touches: [crates/core/src/tasks.rs, crates/core/src/config.rs]
---
A mesa grava as tarefas em `dados/tarefas/<id do projeto>.json` (e os pedidos em `<id>-tNN-brief.md`). Remover o
projeto da lista não apaga esse arquivo, mas adicionar a mesma pasta de novo cria um id novo (`p` + hexadecimal) e a
mesa aparece vazia. As worktrees (`.agentdeck/worktrees/`) mudam junto com a pasta, mas o campo `worktree` de cada
tarefa e o registro do git (`.git/worktrees/*/gitdir`) continuam no caminho velho.

**Por quê:** em 2026-10-24 a pasta `commanddeck-dev` virou `agentdeck`, o projeto foi removido e readicionado, e as 47
tarefas sumiram da mesa (o id foi de `pfdaeded2` para `p76d8a279`).
**Como aplicar:** para recuperar, com o app fechado (ele guarda a lista em memória e regrava ao mexer numa tarefa):
copie `<id antigo>.json` para `<id novo>.json` (o id está em `projectList` do `%APPDATA%\AgentDeck\config.json`),
troque o caminho velho no campo `worktree`, renomeie os `<id antigo>-tNN-*.md` e rode `git worktree repair <worktrees>`.
Ao mexer na lista de projetos, um projeto readicionado na mesma pasta deveria reaproveitar o id.

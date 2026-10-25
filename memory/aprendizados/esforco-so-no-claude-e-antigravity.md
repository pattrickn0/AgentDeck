---
titulo: --effort só vale para quem aceita esforço na aba
resumo: `--task new --effort` com codex ou opencode é recusado ("esforço não aplicável") e a tarefa nem é criada.
touches: [crates/core/src/models.rs, crates/core/src/tasks.rs]
---
O Codex não tem esforço, e o OpenCode só aceita esforço no modo headless (a aba não recebe). Com `--effort`, o
`ask-agent --task new` falha antes de criar a tarefa.

**Por quê:** em 2026-10-24, a criação da t49 (codex) e da t50 (opencode) falhou por isso e precisou ser repetida.
**Como aplicar:** passe `--effort` só a tarefas do Claude (e de quem a mensagem de erro não recusar); para codex e
opencode, crie sem `--effort`.

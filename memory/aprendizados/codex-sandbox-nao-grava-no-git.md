---
titulo: O sandbox do Codex não grava em .git, nem com --add-dir
resumo: Na worktree de uma tarefa, `git add`/`git commit` do Codex dá "Access is denied"; só aprovando o comando fora do sandbox.
touches: [crates/core/src/agents.rs, crates/core/src/tasks/**]
---
A worktree de uma tarefa guarda o índice e o HEAD em `<repo>/.git/worktrees/<id>`, fora da pasta da tarefa. No modo
`workspace-write` (o padrão do Codex 0.164 no Windows, com `sandbox = "elevated"`), o Codex protege qualquer caminho
`.git`: nem `--add-dir <repo>\.git`, nem `--add-dir <repo>\.git\worktrees\<id>`, nem
`sandbox_workspace_write.writable_roots` liberam a gravação (testado com `codex exec` e `codex sandbox`).

**Por quê:** em 2026-10-24 a t49 (Codex, gpt-6-luna) entregou o relatório sem commit; o `git add` saiu com status 1 e a
mesclagem foi recusada por trabalho sem commit. O commit só saiu quando o comando foi aprovado fora do sandbox.
**Como aplicar:** ao dar tarefa ao Codex, diga no pedido que o commit pede aprovação fora do sandbox. Melhor: o app fazer
o commit do que sobrou na worktree ao receber o `--task done`, ou abrir o Codex das tarefas com outra política de
sandbox (decisão do usuário).

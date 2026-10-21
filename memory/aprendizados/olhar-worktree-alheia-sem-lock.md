---
titulo: Olhar a worktree de outra tarefa sem tomar o lock do git
resumo: Para ler o status da pasta de outro agente use worktree_dirty_unlocked (--no-optional-locks), não worktree_dirty.
touches: [crates/core/src/tasks.rs, crates/core/src/situation.rs]
---
`git status` comum atualiza o índice e cria `index.lock` na worktree. Se for a pasta de outro agente e ele rodar
`git add`/`commit` no mesmo instante, o comando dele falha com erro de `index.lock`. A "Situação atual" olha as
worktrees alheias (`task_situation`), então usa `worktree_dirty_unlocked` (`git --no-optional-locks status`).
`worktree_dirty` continua nos pontos que mexem na própria tarefa (`task_done`, `task_merge`, `task_validate`).

**Por quê:** a leitura é só informativa e não pode atrapalhar o trabalho de quem está commitando.
**Como aplicar:** ao ler o estado de uma worktree que não é da tarefa em curso, use a variante sem lock. O
`git::base_command` já define `GIT_OPTIONAL_LOCKS=0`; o argumento explícito deixa a intenção visível e não depende do ambiente.

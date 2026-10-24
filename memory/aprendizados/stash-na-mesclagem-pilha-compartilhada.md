---
titulo: O stash da mesclagem é compartilhado com as worktrees e não pode apagar nada
resumo: "Guardar e mesclar" devolve pelo hash (stash apply <hash>, nunca pop stash@{n}), confere antes com git merge-tree e, na dúvida, deixa no stash.
touches: [crates/core/src/tasks.rs]
---
`task_merge_assisted(.., stash_main = true)` guarda as alterações sem commit do repositório principal num `git stash`,
mescla e devolve. A pilha do stash é **uma só para o repositório e todas as worktrees**, e o `pop` mal feito deixa a
pasta com marcas de conflito.

**Por quê:** (1) outra tarefa ou sessão pode empilhar um stash no meio do caminho, então `stash@{0}` não é
necessariamente o nosso, e a posição muda até entre a conferência e a devolução: o código guarda o hash (`stash_push`),
devolve com `git stash apply <hash>` e só então apaga a entrada, achando a posição de novo pelo hash (`stash_ref`) logo
antes do `drop`. Com `pop stash@{n}`, um `push` de outro no meio faria o app devolver e apagar o stash de outra pessoa. (2) a devolução com conflito deixa marcas na pasta e entradas não mescladas no índice; por isso
`stash_pop` confere antes, sem tocar na pasta, com `git merge-tree --write-tree --merge-base=<stash>^1 HEAD <stash>`
(git 2.38+) e só então devolve. Com conflito, ou git sem `merge-tree --write-tree`, deixa a entrada intacta e o
aviso diz como recuperar. (3) O `base_command` liga `GIT_LITERAL_PATHSPECS`: pathspec mágico (`:(exclude)AGENTS.md`)
não funciona, então os arquivos vão por `--pathspec-from-file=- --pathspec-file-nul` (saída de `diff --name-only -z`).
(4) O stash não leva arquivos não rastreados; o que a branch também cria como arquivo novo seria recusado pelo git no
meio da mesclagem, por isso `untracked_clash` recusa antes de guardar qualquer coisa.

**Como aplicar:** ao mexer na mesclagem, todo desfecho depois do `stash_push` precisa passar por `give_back` (sucesso,
conflito desfeito, falha, recusa do `AGENTS.md`). Só o clique do usuário liga o stash; `task_merge` (linha de comando e
agentes) nunca guarda nada. Nos testes, o git do Windows pode devolver o arquivo com CRLF (`core.autocrlf`): compare
normalizando as quebras de linha.

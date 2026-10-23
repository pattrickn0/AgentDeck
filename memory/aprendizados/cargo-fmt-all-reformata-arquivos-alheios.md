---
titulo: cargo fmt --all reformata arquivos que a tarefa não tocou
resumo: O repositório não está 100% no rustfmt; formate só os seus arquivos (rustfmt --edition 2021 --check <arquivo>).
touches: [crates/**]
---
Rodar `cargo fmt --all` nesta base mexe em vários arquivos alheios (na t28 foram 11 arquivos de `app`, `cli` e `core` que a tarefa
não tocava), o que polui o diff da revisão e pode conflitar com tarefas paralelas. O `rustfmt.toml` do projeto (`max_width = 140`,
`use_small_heuristics = "Max"`) vale e deve ser seguido nos arquivos que você mexe.

**Por quê:** não existe checagem de formato no fluxo, então a base foi acumulando pequenas diferenças.
**Como aplicar:** confira só os seus arquivos com `rustfmt --edition 2021 --check crates/app/src/split.rs ...` (sem `--all`) e,
se rodou o `fmt` inteiro sem querer, desfaça os arquivos alheios com `git restore <arquivo>` antes de commitar.

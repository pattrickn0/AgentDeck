---
titulo: `impl` dividido em módulos filhos: método privado e nome sombreado
resumo: Ao quebrar um `impl` em módulos filhos, `fn` privado de uma seção não é visto pela outra (`pub(super)`) e um submódulo com o nome de um `use crate::x::{self, …}` esconde o import.
touches: [crates/app/src/deck/**, crates/core/src/tasks/**, crates/app/src/tasks.rs]
---
O `impl Deck` de `crates/app/src/deck.rs` virou um `impl` por arquivo de `crates/app/src/deck/` (t54). Quem repetir a
divisão em outro `impl` grande (a `t53` faz o mesmo com `crates/core/src/tasks.rs`) esbarra em duas coisas que o
`cargo check` só mostra depois de mover tudo.

**Por quê:** a privacidade em Rust é por módulo, não por `impl`. Um `fn load_project(&mut self)` declarado em
`deck/tabs.rs` é privado de `deck::tabs` e some para `deck::sidebar`, mesmo sendo método do mesmo `Deck`; só os
campos de `Deck` continuam visíveis (estão no módulo pai). E `mod dictation;` no pai disputa o nome com
`use crate::dictation::{self, …}`: o `use` deixa de compilar (E0255) e, pior, `dictation::toggle(…)` passa a resolver
para o módulo novo (E0425), porque a sombra vale para todo filho que faz `use super::*`.

**Como aplicar:** depois de mover, rode `cargo check` e troque `    fn nome(` por `    pub(super) fn nome(` só nos
métodos que alguma outra seção chama (`pub(super)` = visível no pai e nos irmãos, a mesma visibilidade que o método
tinha dentro do módulo único; não use `pub(crate)`, que amplia). Para o nome sombreado, tire o `self` do `use` e
qualifique as chamadas (`crate::dictation::toggle`). `use super::*;` no topo de cada arquivo traz os tipos, constantes
e imports do pai sem aviso de import não usado, e os testes (`#[cfg(test)]`) podem ficar no arquivo que testam.

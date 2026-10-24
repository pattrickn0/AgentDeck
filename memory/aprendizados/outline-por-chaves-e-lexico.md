---
titulo: O --outline acha os itens por chaves e pelo léxico do realce, não por um analisador
resumo: outline.rs usa as faixas do highlight.rs (o léxico mora no core) para ignorar aspas e comentários; erro de léxico vira item perdido.
touches: [crates/core/src/outline.rs, crates/core/src/highlight.rs, crates/app/src/highlight.rs]
---
`ask-agent --outline` (`outline.rs`) não entende a linguagem: tira do código o texto entre aspas e os comentários, usando
as faixas de `highlight::highlight_lines`, e acha cada item pela profundidade das chaves e pela palavra que abre a linha
(`fn`, `struct`, `impl`...). Por isso o léxico (`Tok`, `Span`, `Lang`, `Carry`, `highlight_line(s)`) mora em `crates/core/src/highlight.rs`; busca, `pieces` e
`Cache` ficam em `crates/app/src/highlight.rs`, que reexporta o léxico (os `crate::highlight::...` do app não mudaram).

**Por quê:** foi a forma mais barata de ter o resumo sem trazer `syn`/`tree-sitter`. O custo é que um erro do léxico quebra o
resumo do arquivo inteiro: um `"` de várias linhas que o `highlight` não carregava para a linha seguinte fez o `/**` de
dentro do texto abrir um comentário de bloco e o `learnings.rs` perdeu tudo depois de uma constante (corrigido: o `"` do Rust
agora continua na linha seguinte).
**Como aplicar:** se um resumo sair com itens faltando, procure primeiro o texto ou comentário que o `highlight` leu errado
(`highlight_lines` na linha anterior ao primeiro item que sumiu) antes de mexer no `outline.rs`. Os itens são só os de
topo e os membros de `impl`/`trait`/`mod`/`class`; corpo de função nunca aparece, e um `impl X { fn a() {} }` numa linha só
não lista o membro. Resposta maior que `FILE_BUDGET`/`OUTLINE_BUDGET` vira "… e mais N itens". As instruções de toda
aba (`bridge_instructions`) têm um teto de caracteres no teste `the_instructions_carry_the_rules_and_stay_short`: a frase do
`--outline` o subiu de 2580 para 2740.

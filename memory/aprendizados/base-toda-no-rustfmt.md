---
titulo: A base toda está no rustfmt e o CI recusa desvio
resumo: Rode cargo fmt --all antes de cada commit; o passo "Formatação" do CI (cargo fmt --all -- --check) falha com qualquer arquivo fora.
touches: [crates/**]
---
Desde 2026-10-25 todo o workspace passa no `cargo fmt --all -- --check`, com o `rustfmt.toml` do projeto (`max_width = 140`,
`use_small_heuristics = "Max"`). O CI roda esse passo antes do build em Windows e Ubuntu, então um arquivo fora do formato deixa o
CI vermelho.

**Por quê:** o aprendizado antigo mandava formatar só os próprios arquivos, porque a base acumulava desvios; com isso as tarefas
pararam de rodar o `fmt` e três arquivos da t52 entraram fora do formato, deixando o CI vermelho sem ninguém ver.
**Como aplicar:** `cargo fmt --all` antes do commit (agora só mexe no que você mudou). Se aparecer diferença num arquivo que você não
tocou, ela veio de outro commit sem formatar: inclua no seu commit, que é o que deixa o CI verde.

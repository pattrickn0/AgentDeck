---
titulo: O egui_commonmark acompanha a versão do egui
resumo: A versão menor do egui_commonmark segue a do egui (0.25 para o egui 0.36); atualize os dois juntos e restaure o espaçamento no markdown.
touches: [Cargo.toml, crates/app/Cargo.toml, crates/app/src/ui/docs.rs]
---
O markdown do modo Documentos é desenhado pelo `egui_commonmark` (dependência do workspace, sem recursos extras). Ele depende de
uma versão exata do `egui`: a 0.25 é a do egui 0.36 (a 0.26 já pede o 0.37, a 0.24 o 0.35). Quando o egui mudar de versão, a
conta é conferir no crates.io qual `egui_commonmark` pede o egui novo e atualizar os dois no `Cargo.toml` da raiz; se não houver,
o desenho próprio com `pulldown-cmark` (títulos, listas com `- [ ]`, tabelas, citações, código) é o plano B.

**Por quê:** o app zera `style.spacing.item_spacing` e `button_padding` (`theme::install_style`) porque quase tudo é pintado à mão; o
`egui_commonmark` depende desse espaçamento para separar parágrafos e itens, e sem ele o texto sai colado. Por isso
`ui/docs.rs::markdown_style` restaura o espaçamento, as fontes e as cores só dentro da coluna do documento.
**Como aplicar:** ao atualizar o egui, atualize junto e confira o resultado com `COMMANDDECK_DEBUG_UI=docs-demo-report`; mexa no
estilo do markdown só em `markdown_style`, nunca em `install_style`.

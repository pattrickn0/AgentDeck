---
titulo: No egui, a cor do place e a margin do TextEdit podem ser ignoradas
resumo: painter.galley só usa a cor dada para texto sem cor; TextEdit com .frame(...) ignora .margin(...).
touches: [crates/app/src/ui/**]
---
Duas armadilhas que passaram pela repaginação visual sem ninguém ver:

- `place(painter, r, align, g, cor)` (e `painter.galley`) só aplica `cor` a texto sem cor (`PLACEHOLDER`). Um galley
  feito com `galley(..., p.text_strong)` sai sempre branco, qualquer que seja a cor passada ao `place`. Foi assim que
  as abas da barra direita ficaram todas brancas e o número do selo ativo ficou branco sobre a cor de destaque.
- No egui 0.36, `TextEdit::frame(Frame)` substitui a moldura inteira: `.margin(...)` só vale para a moldura
  padrão. Com `.frame(egui::Frame::NONE).margin(m)` o texto encosta na borda.

**Por quê:** nenhum dos dois dá erro nem aviso; só aparece na captura de tela, e de perto.

**Como aplicar:** monte o galley já com a cor final (ou use `text_in`, que faz isso); quando a cor depende do hover,
meça a largura com `text_w` antes e pinte depois. Em campo de texto com moldura própria, ponha a margem na moldura:
`.frame(egui::Frame::NONE.inner_margin(m))`.

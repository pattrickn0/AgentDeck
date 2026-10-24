---
titulo: O editor do modo Documentos: decisões e armadilhas
resumo: Realce por tokenizador próprio (sem syntect); o TextEdit pede o galley todo quadro (cache por impressão digital); edição suja nunca é trocada por leitura.
touches: [crates/app/src/docs.rs, crates/core/src/highlight.rs, crates/app/src/ui/code.rs, crates/app/src/ui/select.rs, crates/app/src/ui/diff_view.rs, crates/app/src/ui/docs.rs]
---
O arquivo de texto pequeno do modo Documentos é um `DocState::Source` pintado por um `egui::TextEdit` (`ui/code.rs`); o diff e o
arquivo grande são pintados à mão (`ui/diff_view.rs`, `ui/code.rs::text_body`) com a seleção de `ui/select.rs`.

- **Realce sem `syntect`.** `egui_extras::syntax_highlighting` (o `syntect`) traria o motor de regex, as definições e os temas:
  alguns MB no executável e minutos a mais na compilação das três plataformas da CI, para nove linguagens. `highlight.rs` é uma tabela
  por linguagem (palavras-chave, comentários, aspas) e uma função por linha, com o estado que passa de uma linha à seguinte em `Carry`.
  Se faltar linguagem, acrescente uma entrada em `spec()` (ou `Lang::from_path`); só pense em `syntect` se precisar de gramática de verdade.
- **O `TextEdit` pede o galley a cada quadro.** O `layouter` roda todo quadro (e de novo a cada tecla), então montar o realce ali sem cache
  refaria o arquivo inteiro 60 vezes por segundo. `docs::Shaped` guarda o galley por impressão digital do texto, largura de quebra, escala da
  tela e busca (`Find::stamp`), e `highlight::Cache` guarda as faixas por texto e linguagem: digitar refaz, repintar não. Ao mexer na chave,
  confira o teste `the_shaped_text_is_rebuilt_only_when_…`. O limite do editor (`EDIT_MAX` 256 KB, linha de 4 000 bytes) existe por isso.
- **O `TextEdit` multilinha do egui 0.36 não rola na horizontal** (o texto passa da largura e é cortado): o editor quebra a linha na
  largura, e a coluna dos números pinta o número só na primeira linha de cada parágrafo (`ends_with_newline` do galley). O arquivo grande,
  que não passa pelo `TextEdit`, rola nos dois sentidos.
- **Edição suja nunca é trocada por uma leitura.** `DocTab::apply` manda a leitura de um `Source` para `Source::reload`, que só guarda o texto
  do disco em `external` (a faixa pergunta). Quem acrescentar outro caminho que escreva em `DocState` deve respeitar isso; é também por isso
  que `docs_reload` de um `Source` não passa por "Carregando…". O watcher relê depois de cada gravação nossa: `Source::is_disk` evita o falso
  aviso.
- **Fim de linha e BOM voltam como eram.** A tela trabalha com `\n` e sem BOM; `Eol`/`bom` do arquivo original são aplicados em
  `workspace::write_text`. CRLF+LF misturados ou texto não UTF-8 travam a edição (`Source::locked`): gravar de volta mudaria linhas que o usuário
  não tocou.
- **Seleção própria só onde o egui não alcança.** `select::drive` cria a área de interação antes de qualquer botão desenhado por cima (os botões
  de trecho do diff da aba Git), senão a área toma o clique deles. A posição é (linha, byte do texto que se vê); o texto copiado desfaz a
  troca de tabulação por espaços (`diff_view::unexpanded`).
- **Teste de interface com o egui**: um `Context` novo não tem fontes até o primeiro `run_ui`, e o `TexturesDelta` precisa de `.clear()` antes de
  ser descartado (veja `draw_frame` em `tasks_flow_tests.rs`, que também injeta teclas, texto e mouse).

---
titulo: Janela escondida ou minimizada não pinta, e a minimizada tem tamanho 0×0
resumo: O trabalho do Deck sem quadro roda por Deck::background; nenhuma pintura nem fit_tab com a janela escondida.
touches: [crates/app/src/app.rs, crates/app/src/deck/**]
---
Com a janela minimizada (no Windows) ou escondida, o winit não entrega `RedrawRequested` (o sistema não manda `WM_PAINT`) e a
janela minimizada informa `inner_size` 0×0. Duas consequências que já mordem: (1) tudo o que ficava dentro do quadro
(`Deck::before_frame` → `tick`: entregas de `ask-agent`, respostas "digitadas depois", Enter agendado, fim de turno,
temporizadores) parava; (2) se um quadro chega mesmo assim, desenhar com 0×0 faz o egui entrar em pânico em debug
(`Negative child size` em `placer.rs`, reproduzido ao minimizar um build de debug da base) e, no `panes::draw`, o `fit_tab`
pediria um terminal sem área, que estraga o CLI (ver `claude-code-sem-esc3j-no-windows.md`).

**Por quê:** o trabalho que não precisa de pintura foi separado em `Deck::work`; `before_frame` o chama com o `Context`, e
`Deck::background` sem ele. `App::redraw` com `is_hidden` (minimizada, não visível ou sem área) só faz o `background` e volta;
`Wake` e os prazos viram `Background::wake`/`next` (`app.rs`), com `ControlFlow::WaitUntil` até o próximo prazo e `Wait` sem prazo.
**Como aplicar:** o que o `tick` precisar do egui (`Context`: copiar para a área de transferência, ler o mouse) fica atrás de
`Option<&Context>` e é adiado, não feito às cegas. Algo novo que esconda a janela (a bandeja) deve chamar `Background::wake` ao
esconder, porque o winit não avisa de `set_visible(false)`. O teste do `background` sem quadro é
`a_hidden_window_still_types_the_queued_messages_and_the_late_answers_and_never_resizes_the_terminals` (`tasks_flow_tests.rs`).

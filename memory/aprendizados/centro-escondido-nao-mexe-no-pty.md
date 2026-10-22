---
titulo: Com os terminais escondidos (modo Documentos), nada os toca
resumo: Sem fit_tab, sem teclado nem IME, e o fim de turno conta como não visto; a troca de lado só é percebida um quadro depois.
touches: [crates/app/src/deck.rs, crates/app/src/docs.rs, crates/app/src/ui/mod.rs, crates/app/src/ui/panes.rs, crates/app/src/ui/docs.rs]
---
Com o centro em Documentos, `ui::draw` não chama `panes::draw`; os terminais continuam rodando, mas ninguém os desenha, redimensiona
ou alimenta. Três regras seguem disso: (1) **nenhum `fit_tab`** enquanto o centro mostra documentos, senão o PTY muda de tamanho
escondido e o Claude Code duplica a conversa no Windows (ver `claude-code-sem-esc3j-no-windows.md`); ao voltar, o `fit_tab` de sempre
ajusta o tamanho, com a espera de 150 ms; (2) o terminal não recebe **teclado nem IME**: `handle_key` e `handle_ime` olham
`docs.view` direto; (3) o fim de um turno marca `Tab.done` até na aba ativa (ninguém a está vendo), e ativar uma aba sem voltar aos
agentes não a limpa: quem alimenta o selo da chave é `Deck::agents_badge`.

**Por quê:** `Deck::center_changed` (que desliga `term_focus`, limpa o IME e marca a aba ativa como vista ao voltar) só roda no
`before_frame` seguinte à troca, e as teclas chegam entre dois quadros: o teclado do terminal não pode depender de `term_focus`
sozinho. O estado de `Docs::view` é o que manda; quem abre uma aba ou clica num agente deve chamar `Deck::show_agents`.
**Como aplicar:** um modo novo que esconda os terminais repete as três regras; o teste
`reading_documents_keeps_the_terminals_running_unresized_and_marks_finished_turns_as_unseen` (`tasks_flow_tests.rs`) cobre o PTY, o selo e a volta.

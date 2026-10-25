---
titulo: O X esconde na bandeja e a segunda abertura ativa a instância viva
resumo: Encerrar é o menu da bandeja; a segunda instância lê a ponte em dados/.agentdeck.bridge e faz POST /activate com o token mestre.
touches: [crates/app/src/app.rs, crates/app/src/tray.rs, crates/core/src/activation.rs, crates/core/src/bridge.rs]
---
O × da janela (`CloseRequested`, Alt+F4 e o × desenhado na barra de título) **não encerra**: esconde na bandeja
(`App::close_window`). Quem encerra é o "Encerrar" do menu do ícone (`Deck::request_quit`). Qualquer código novo que
mexa em "fechar" precisa decidir qual dos dois quer; o `quit_requested` do `Deck` agora vale com a janela escondida
(`App::quit_if_requested` roda também depois do `Deck::background`).

**Por quê:** o usuário quer o comportamento do Discord, com turnos longos terminando sem a janela. A janela escondida
não recebe quadro, então três coisas só funcionam porque foram feitas à mão: `Background::wake` ao esconder (o winit não
avisa), `show_window` ao voltar (um `request_redraw` e `viewport_stale`), e a pergunta de "Encerrar" só aparece depois de
mostrar a janela. Sem bandeja (Linux sem `StatusNotifierWatcher`) o × **minimiza**: esconder deixaria o app invisível e
sem como voltar (`tray::close_action`).

A segunda abertura não falha mais com "outra janela já está usando a pasta": `main` chama `activation::activate_running`,
que só age se a trava `dados/.agentdeck.lock` está pega por outro processo, lê `dados/.agentdeck.bridge` (porta e token
mestre, gravado depois de a ponte subir) e faz `POST /activate`. O arquivo sozinho não prova nada: quem pega a trava
apaga o de um travamento, e uma instância ainda subindo (trava pega, arquivo ausente) é esperada por até 6 s.
**Como aplicar:** `/activate` só aceita o token mestre (um agente não pode trazer a janela do usuário para a frente); no
Windows a segunda instância precisa do `AllowSetForegroundWindow(ASFW_ANY)` antes do POST, senão a janela só pisca. Para
testar a bandeja sem clicar: `PostMessage(hwnd, WM_CLOSE)` esconde a janela e abrir o `.exe` de novo a traz de volta.

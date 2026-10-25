---
titulo: Claude Code duplica a conversa ao mudar a largura (sem ESC[3J no Windows)
resumo: Cada largura nova gera uma cópia no histórico; por isso o PTY só muda com o tamanho parado.
touches: [crates/app/src/deck.rs, crates/app/src/terminal/**]
---
Ao mudar de largura, o Claude Code limpa só a tela e reimprime a conversa. Fora do Windows ele manda `ESC[3J`
(apaga o histórico); no Windows não manda, então a cópia anterior fica no histórico, na largura antiga. Foi medido
numa sonda (portable-pty + alacritty_terminal + o `claude.exe` real): uma cópia a mais por largura, o mesmo com o
`OpenConsole.exe` do Windows Terminal.

**Por quê:** limpar o nosso histórico antes do redimensionamento não serve: ao alargar, o Claude Code redesenha só
a tela e a conversa sumiria.
**Como aplicar:** o PTY só muda de tamanho com o mouse solto e o tamanho parado por 150 ms (`RESIZE_DEBOUNCE`,
`resize_due` em `deck.rs`): um arrasto vira uma cópia só. Uma troca de aba no Destaque muda a largura das janelas
sem arrasto: ali o prazo é de 600 ms (`HIGHLIGHT_RESIZE_DEBOUNCE`, marcado em `Deck::activate` por
`highlight_switch_until`). Voltar ao tamanho que o PTY já tem dentro do prazo cancela o pendente (`Deck::fit_tab`).
Depois de um redimensionamento aplicado, o próximo da mesma aba espera a saída dela ficar parada por 300 ms
(`RESIZE_SETTLE`), com teto de 3 s (`RESIZE_SETTLE_MAX`) para um agente que não para de escrever: sem isso, trocas
seguidas de aba empilhavam reimpressões e a aba do Claude ficou ~15 s desenhando em 38 colunas (t51, 2026-10-24).
Não encurte esses prazos nem redimensione o PTY a cada quadro. O problema de fundo continua aberto em `ideias.md`
("Limitações conhecidas").

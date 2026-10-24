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
`resize_due` em `deck.rs`): um arrasto vira uma cópia só. Não encurte esse prazo nem redimensione o PTY a cada
quadro. O problema de fundo continua aberto em `ideias.md` ("Limitações conhecidas").

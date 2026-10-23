---
titulo: Terminal com a metade de cima em branco: grave o PTY antes de mexer (t37)
resumo: Não reproduz com o claude.exe real nem no Term; ligue AGENTDECK_DEBUG_PTY=1, pegue dados/pty/*.grid.txt e .bin e decida por eles.
touches: [crates/app/src/terminal/**, crates/app/src/deck.rs, tools/pty-probe/**]
---
O usuário viu (23/10) uma aba do Claude com a metade de cima da tela em branco e o texto só embaixo, cortado no meio, logo
depois de uma resposta longa. A `t37` não conseguiu reproduzir, então **a causa continua aberta** (`pendencias.md`, Terminal).
O que foi descartado, para o próximo não repetir:

- **O desenho (`view.rs`) está inocente no que a CPU decide.** Ele não guarda nada entre quadros: pinta toda a `display_iter`
  da grade. O teste `the_frame_paints_every_row_the_grid_has_text_in_and_no_other` repete uma sessão real do Claude e confere
  que as linhas pintadas são exatamente as da grade (também rolada no histórico). Atlas de fontes/GPU não se testa assim.
- **O emulador e o CLI não deixam vão** em ~25 execuções da sonda (`tools/pty-probe`: portable-pty/ConPTY + `alacritty_terminal`
  + o `claude.exe` 2.1.309 real, tela de 12 a 58 linhas e 50 a 120 colunas, redimensionando durante e depois da resposta, até
  quatro respostas seguidas, modo clássico e tela alternativa): nenhuma corrida de 6+ linhas vazias na tela nem no histórico.
- Fatos medidos que ajudam a raciocinar: o Claude Code 2.1.x usa **por padrão a tela alternativa** (`ESC[?1049h`, histórico 0);
  o modo clássico (histórico) vem de `{"tui":"default"}` (`/tui default`, ou `--settings`). A barra de rolagem da captura do
  usuário tinha histórico, então a aba dele estava no modo clássico. A saída que o ConPTY entrega é só pintura absoluta
  (`ESC[l;cH`, `ESC[K`, `ESC[nX`, `ESC[nC`) e `\n` no fim: sem `ESC[J`, `ESC[nS/T`, `ESC[r`; por isso uma grade diferente da do
  ConPTY (linhas ou deslocamento do viewport) pintaria no lugar errado e deixaria vão, sem erro nenhum.

**Por quê:** o defeito só aparece na sessão longa do usuário; chutar uma correção (limpar cache que não existe, reabrir o PTY)
esconderia o sinal. Uma gravação decide: se `.grid.txt` (a grade, 1,5 s depois da última saída) tem o texto e o quadro mostrava
em branco, é o desenho; se a grade também está em branco, reproduza o `.bin` com `.events.txt` num `Term` novo
(`record::replay::run`, nos testes): em branco de novo = o que o CLI/ConPTY mandou; cheio = o estado do emulador divergiu
(redimensionamento fora de ordem, `Session::resize` mexe no `Term` antes do PTY).
**Como aplicar:** com o defeito na tela, rode o app com `AGENTDECK_DEBUG_PTY=1` (desligado por padrão), abra a aba, e quando
acontecer anote a hora e mande os três arquivos de `dados/pty/` daquela aba (`<aba>-<data>-<hora>.bin`, `.events.txt`,
`.grid.txt`) junto com a captura. O `.bin` contém tudo o que o CLI mostrou: não publique sem olhar. Para a conferência por
sonda, `tools/pty-probe/README.md`.

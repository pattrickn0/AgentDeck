---
titulo: No Windows, a grade não pode puxar o histórico ao crescer: o ConPTY repinta por cima e as linhas somem (t37)
resumo: Ao ganhar linhas (ou juntar linhas no reflow) o alacritty puxa histórico para a tela; o ConPTY repinta de ESC[H e apaga o puxado. terminal/resize.rs desfaz isso.
touches: [crates/app/src/terminal/**, crates/app/src/deck/**, tools/pty-probe/**]
---
Defeito (usuário, 23/10): "às vezes, nos terminais" a tela fica com linhas em branco e o texto cortado, em qualquer CLI. Reproduzido **sem
o Claude**, com `powershell` + `tools/pty-probe`: 200 linhas numeradas, janela 58→30→58 linhas, e as linhas 144 a 171 sumiam do histórico
(28 = as linhas ganhas) com 28 linhas em branco embaixo; numa sessão de 400 linhas quebradas, 120→60→120 colunas perdia 29 linhas; com 24
mudanças aleatórias de tamanho, 155. É a causa 2/3 do pedido: o par ConPTY + `alacritty_terminal` desencontrado.

**Causa:** ao ganhar linhas, ou ao juntar linhas quebradas (reflow de uma largura maior), o `alacritty_terminal` mantém o cursor na mesma
linha da tela e **puxa linhas do histórico para o topo**. O conhost não: o topo da janela dele fica no mesmo conteúdo, as linhas novas ficam
em branco embaixo, e logo depois do redimensionamento ele repinta a janela toda a partir de `ESC[H` (visto nos bytes: `ESC[8;linhas;colunast`,
`ESC[H`, as linhas, `ESC[K`). A repintura cai por cima das linhas puxadas, que somem do histórico e da tela. Encolher já era igual nos dois.
Os CLIs em tela alternativa (OpenCode) não têm histórico e se repintam sozinhos: não sofrem.

**Correção:** `terminal/resize.rs` (usada por `Emulator::resize`, só no Windows). Marca a primeira célula da tela com um caractere sem largura,
deixa o `alacritty_terminal` redimensionar e, se a marca desceu (linhas puxadas), devolve essas linhas ao histórico (`scroll_up`, cursor
junto); se a marca ficou no meio de uma linha (a continuação foi juntada à anterior), a linha inteira vai para o histórico. Resultado como
o do ConPTY: o que estava no topo continua no topo e sobram linhas em branco embaixo. Medido com o `powershell`: 0 linhas perdidas nas
24 mudanças aleatórias (eram 155). Sobra, em mudanças de largura, a repetição de 1 a 3 linhas no histórico (o mesmo problema de cópias do
`claude-code-sem-esc3j-no-windows.md`).
**Testes:** `record.rs` reproduz duas gravações reais do conhost (`testdata/conhost-powershell-*.bin` + `.events.txt`), com a correção ligada
(nada some) e desligada (some).

**Como investigar o próximo caso:** a sonda `tools/pty-probe` (README) roda qualquer CLI, aplica as trocas de tamanho (`RESIZE`) e confere a
numeração (`L00001 ...`) e vãos; `PROBE_PULL=1` desliga a correção para comparar; `PROBE_WATCH=L00372,...` mostra onde cada linha está. No app,
o item "Gravar o terminal (diagnóstico)" do menu da aba (botão direito; ou `AGENTDECK_DEBUG_PTY=1`) grava em `dados/pty/` os bytes (`.bin`),
as trocas de tamanho (`.events.txt`) e a grade 1,5 s depois da última saída (`.grid.txt`); "Salvar foto da grade agora" guarda a grade na hora
e abre a pasta. Grade com o texto e quadro em branco = desenho; grade em branco, e o `.bin` reproduzido num `Term` novo também = o CLI/ConPTY.
O `.bin` tem tudo o que o CLI mostrou: não publique sem olhar.

**Por quê:** o ConPTY não conversa com o emulador sobre o que cada um fez no resize; qualquer diferença de modelo vira conteúdo no lugar errado
sem erro nenhum. **Como aplicar:** mexer em resize/reflow é mexer aqui; meça com a sonda (shell e CLI) antes e depois. Ainda não se achou o caso
"texto embaixo e vão em cima" da captura do usuário; se a correção não bastar, grave a sessão dele.

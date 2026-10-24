# pty-probe

Sonda usada nas investigações do terminal (`claude-code-sem-esc3j-no-windows`, `t37`). Roda um programa de verdade num PTY (ConPTY no
Windows), alimenta o mesmo emulador do app (`alacritty_terminal` 0.26 e a mesma mudança de tamanho, `crates/app/src/terminal/resize.rs`,
incluída por `#[path]`) e mede a grade, sem janela.

```
cd tools/pty-probe && cargo build --release
PROBE_PROGRAM=C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe PROBE_ARGS=-NoLogo PROBE_BOOT=3 PROBE_CWD=C:/Users/<você> \
RESIZE="12:30:120,15:58:120,18:58:60,21:58:120" \
./target/release/pty-probe 58 120 <saida.bin> '1..400 | % { ("L{0:D5} " -f $_) + ("abcdefghij" * 8) }'
```

- `pty-probe <linhas> <colunas> <saida> "<linha 1>" ["<linha 2>" ...]`: cada linha é digitada e enviada com Enter, a primeira depois de
  `PROBE_BOOT` segundos (padrão 8) de silêncio, as outras quando a saída se aquieta.
- `PROBE_PROGRAM` (obrigatório), `PROBE_ARGS` (separados por espaço), `PROBE_CWD`; para o Claude Code: `PROBE_ARGS="--model haiku"`, uma pasta
  já confiável, e `PROBE_SETTINGS=<arquivo.json>` (vai em `--settings`; com `{"tui":"default"}` ele usa o modo clássico, com histórico; sem isso, a
  tela alternativa).
- `RESIZE="seg:linhas:colunas,..."`: trocas de tamanho, em segundos depois do primeiro envio (grade e PTY mudam juntos, como `Session::resize`).
  Cada troca imprime, 0,9 s depois, o padrão da tela (`#` linha com texto, `.` vazia), o histórico e o relatório `SEQ`.
- Relatório `SEQ`: lê as linhas `L00001 ...` na ordem histórico + tela e diz as que **pularam** (`puladas`), as que **repetem ou voltam**
  (`repetidas/voltas`) e quantas linhas vazias ficaram entre duas consecutivas. Também avisa `VAO` (5+ linhas vazias seguidas com texto depois).
- `PROBE_PULL=1`: desliga a correção do app (o `alacritty_terminal` puro), para comparar. `PROBE_WATCH=L00372,PS C`: mostra em que linha
  cada texto está depois de cada troca.
- Grava `<saida>` (bytes crus) e `<saida>.events.txt` (trocas de tamanho), o formato da gravação do app (`terminal/record.rs`), que os testes
  reproduzem (`record::replay`).
- O ambiente do programa é o do app: `TERM=xterm-256color`, `COLORTERM=truecolor`, sem `CLAUDE_CODE_*`/`CLAUDECODE`. Com o Claude Code a
  sonda gasta tokens (uma resposta por linha enviada).

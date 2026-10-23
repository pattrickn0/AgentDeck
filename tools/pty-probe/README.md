# pty-probe

Sonda usada na investigação da `t37` ("terminal: metade da tela fica em branco") e na do `claude-code-sem-esc3j-no-windows`.
Roda um CLI de verdade num PTY, alimenta o mesmo emulador do app (`alacritty_terminal` 0.26) e mede a grade, sem janela.

```
cd tools/pty-probe && cargo build --release
PROBE_PROGRAM=C:/Users/<você>/AppData/Roaming/npm/node_modules/@anthropic-ai/claude-code/bin/claude.exe \
PROBE_CWD=<pasta já confiável no Claude> \
RESIZE="3:30:100,6:58:120" \
./target/release/pty-probe <linhas> <colunas> <saida.bin> "<prompt 1>" ["<prompt 2>" ...]
```

- `RESIZE="seg:linhas:colunas,..."`: trocas de tamanho, em segundos depois do primeiro prompt (a grade e o PTY mudam juntos,
  como `Session::resize`).
- `PROBE_SETTINGS=<arquivo.json>`: vai em `--settings`. Com `{"tui":"default"}` o Claude Code 2.1.x volta ao modo clássico
  (texto no histórico); sem isso ele usa a tela alternativa (sem histórico).
- Imprime `VAO ...` (padrão de linhas vazias `.` e com texto `#`) quando aparece um vão, e no fim o padrão final, o tamanho do
  histórico, as corridas de 6+ linhas vazias na tela e no histórico, e a grade.
- O ambiente do CLI é o do app: `TERM=xterm-256color`, `COLORTERM=truecolor`, sem `CLAUDE_CODE_*`/`CLAUDECODE`.
- A sonda custa tokens (uma resposta por prompt; use um modelo pequeno, a sonda passa `--model haiku`).

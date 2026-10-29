# pty-probe

Probe used in terminal investigations (`claude-code-sem-esc3j-no-windows`, `t37`). Runs a real program in a PTY (ConPTY on
Windows), feeds the same emulator as the app (`alacritty_terminal` 0.26 and the same resize logic, `crates/app/src/terminal/resize.rs`,
included via `#[path]`), and measures the grid, without a window.

```
cd tools/pty-probe && cargo build --release
PROBE_PROGRAM=C:/Windows/System32/WindowsPowerShell/v1.0/powershell.exe PROBE_ARGS=-NoLogo PROBE_BOOT=3 PROBE_CWD=C:/Users/<you> \
RESIZE="12:30:120,15:58:120,18:58:60,21:58:120" \
./target/release/pty-probe 58 120 <output.bin> '1..400 | % { ("L{0:D5} " -f $_) + ("abcdefghij" * 8) }'
```

- `pty-probe <lines> <columns> <output> "<line 1>" ["<line 2>" ...]`: each line is typed and sent with Enter, the first after
  `PROBE_BOOT` seconds (default 8) of silence, the others when output settles down.
- `PROBE_PROGRAM` (required), `PROBE_ARGS` (space-separated), `PROBE_CWD`; for Claude Code: `PROBE_ARGS="--model haiku"`, an already
  trusted folder, and `PROBE_SETTINGS=<file.json>` (passed to `--settings`; with `{"tui":"default"}` it uses classic mode, with history; without it, the
  alternate screen).
- `RESIZE="sec:lines:columns,..."`: resize events, in seconds after the first input (grid and PTY resize together, like `Session::resize`).
  Each resize prints, 0.9 s later, the screen pattern (`#` line with text, `.` empty), the history, and the `SEQ` report.
- `SEQ` report: reads `L00001 ...` lines in history + screen order and reports those that **skipped** (`puladas`), those that **repeat or go back**
  (`repetidas/voltas`), and how many empty lines remained between two consecutive lines. Also warns on `VAO` (5+ consecutive empty lines followed by text).
- `PROBE_PULL=1`: disables the app fix (pure `alacritty_terminal`), for comparison. `PROBE_WATCH=L00372,PS C`: shows which line
  each text is on after each resize.
- Records `<output>` (raw bytes) and `<output>.events.txt` (resize events), the app's recording format (`terminal/record.rs`), which tests
  replay (`record::replay`).
- The program environment matches the app: `TERM=xterm-256color`, `COLORTERM=truecolor`, without `CLAUDE_CODE_*`/`CLAUDECODE`. With Claude Code, the
  probe consumes tokens (one response per sent line).

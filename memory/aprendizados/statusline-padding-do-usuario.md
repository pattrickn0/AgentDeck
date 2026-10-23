---
titulo: A statusline do app precisa do padding da statusline do usuário
resumo: O Claude Code aplica o padding do statusLine; o nosso --usage só repassa a linha, então write_claude_hooks copia o padding do usuário.
touches: [crates/core/src/agents.rs, crates/cli/src/bin/ask-agent.rs]
---
O app grava `dados/hooks/claude-settings.json` (passado em `--settings`) com a própria statusline
(`ask-agent --usage`), que substitui a do usuário. O `--usage` roda o comando do usuário e imprime a saída
dele, mas o `padding` é aplicado pelo Claude Code, não pela linha: com `"padding": 0` fixo, a statusline do
usuário que tinha margem encosta na borda. `write_claude_hooks` lê o `padding` da statusline do usuário na
mesma precedência do Claude Code (`.claude/settings.local.json` do projeto, `.claude/settings.json` do
projeto, `$CLAUDE_CONFIG_DIR/settings.json` ou `~/.claude/settings.json`), ignorando a nossa própria, e grava
esse valor no nosso `statusLine`.

**Por quê:** o `padding` não aparece na saída do comando, então repassar a linha do usuário não basta.
**Como aplicar:** ao mexer em `write_claude_hooks`, mantenha a leitura do padding. O arquivo de hooks é um só
por pasta de dados, então projetos com paddings diferentes alternam qual valor fica gravado (o padding do
usuário, igual para todos, é o caso comum).

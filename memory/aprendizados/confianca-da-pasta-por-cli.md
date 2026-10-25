---
titulo: Cada CLI guarda a confiança da pasta num lugar e com uma regra diferente
resumo: Claude herda da pasta-mãe (~/.claude.json), Codex da raiz do repositório e aceita -c, Antigravity só pelo caminho exato (settings.json).
touches: [crates/core/src/trust.rs, crates/core/src/agents.rs, crates/core/src/tasks.rs]
---
Conferido em 2026-10-25 (Claude Code 2.1.310, Codex 0.164.0, `agy` 1.4.7), abrindo cada CLI num PTY numa pasta nova:

- **Claude:** `projects["C:/pasta"].hasTrustDialogAccepted` em `~/.claude.json` (barras normais). Uma pasta-mãe aceita vale
  para as filhas, então a worktree de uma tarefa herda a do projeto. Não há opção de linha de comando.
- **Codex:** `[projects.'c:\pasta'] trust_level = "trusted"` no `config.toml`, pela **raiz do repositório** (a da worktree
  é a do repositório principal). `-c projects={'<pasta>'={trust_level='trusted'}}` resolve sem gravar nada; a caixa não
  importa. **Chave repetida no TOML derruba o Codex** ("invalid type: string ... expected a map"): a aba do projeto passa
  uma chave só.
- **Antigravity:** `trustedWorkspaces` em `~/.gemini/antigravity-cli/settings.json`, comparado pelo **texto exato** do
  caminho: nem a pasta-mãe (havia `C:\` e `C:\Users\pattr` na lista e ainda perguntava), nem minúsculas, nem `/`. O
  `--add-dir` não interfere (a hipótese antiga da pendência estava errada).

**Por quê:** a aba aberta em segundo plano (`ask-agent`, tarefa) ficava parada na pergunta sem ninguém ver.
**Como aplicar:** o app grava antes de abrir (`trust::prepare`); a pasta pessoal e a raiz de disco nunca. Ao testar,
use uma pasta nova e tire depois as entradas de teste do `~/.claude.json` e do `settings.json` do `agy`. Enviar Enter
pelo PTY não aceita a pergunta do `agy` (o Enter cru não chega), então teste pela lista, não pelo teclado. CLI novo: ache
onde ele guarda a confiança e acrescente a `trust::prepare`.

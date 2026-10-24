---
titulo: Cada CLI recebe as instruções do AgentDeck por um canal próprio
resumo: Claude, Codex, OpenCode, Aider e Interpreter por opção; Antigravity, Goose e Copilot só pelo AGENTS.md (criado só com o bloco do app).
touches: [crates/core/src/agents.rs, crates/core/src/models.rs, crates/core/src/workspace.rs, crates/core/src/headless.rs, AGENTS.md]
---
As instruções do AgentDeck (regra de não usar subagentes internos, `ask-agent`, memória geral) não podem depender do
`AGENTS.md` do usuário: um projeto novo não tem um, e o agente sem as instruções abre o subagente do próprio CLI.

- **Claude:** `--append-system-prompt`. **Codex:** `-c developer_instructions=<TOML>`; num shim `.cmd` as aspas morrem
  (`cmd_safe_arg`), então o texto vai cru, que o Codex aceita como string quando não é TOML. Não instalado na máquina de
  desenvolvimento: nunca visto funcionando.
- **OpenCode:** `{"instructions": ["<arquivo>"]}` em `OPENCODE_CONFIG_CONTENT` (soma-se às do usuário). Conferido: com
  `opencode run --standalone` a regra do arquivo foi obedecida. Só o servidor próprio da aba lê a variável, então o TUI
  agora **sempre** abre com `--standalone`, com ou sem modelo.
- **Aider:** `--read <arquivo>`. **Interpreter:** `--custom_instructions` (não verificados).
- **Antigravity, Goose, Copilot:** só leem o `AGENTS.md` (o `agy` também lê `GEMINI.md` e `~/.gemini/AGENTS.md`, e aceita
  plugins globais; nenhuma opção de sistema). O app cria um `AGENTS.md` só com o bloco quando a pasta não tem um, avisa na
  Conversa e nunca toca num que já exista (`Core::ensure_agents_file`).
- O texto genérico (arquivo e bloco) não lista os agentes instalados: senão o arquivo mudaria de máquina para máquina e
  sujaria o `git status`.

**Por quê:** relato de usuário novo (23/10): os agentes não usavam o `ask-agent` e abriam subagentes do CLI.
**Como aplicar:** CLI novo = achar a opção de instruções dele (`--help`, documentação) e acrescentar a `instruction_args`;
só sem opção entra em `reads_only_agents_md`. O que a cota impede de testar vai para `memory/testes.md`.

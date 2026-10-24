---
titulo: Agente falso dos testes é um binário do crate, não um script de shell
resumo: Um `[[bin]]` de teste com `CARGO_BIN_EXE_<nome>` roda no Windows e no Unix; o comportamento vem de um JSON ao lado, porque o headless limpa o ambiente.
touches: [crates/core/tests/bridge.rs, crates/core/tests/support/fake_agent.rs, crates/core/Cargo.toml]
---
O `tests/bridge.rs` precisa de um "claude" falso que imprima texto, leia a entrada padrão e saia com um código. Um
`.cmd` esbarraria no codepage do `cmd.exe` (acentos nos prompts) e nas aspas/`%` dos argumentos; por isso o falso é o
binário de teste `fake_agent` (`[[bin]]` em `crates/core/Cargo.toml`, fonte em `tests/support/fake_agent.rs`).

**Por quê:** `CARGO_BIN_EXE_<nome>` (definido só para bins do próprio pacote) dá o caminho absoluto do binário já
compilado, e o teste o copia para a pasta de busca com o nome do agente (`claude.exe` no Windows, `claude` no Unix).
O comportamento não pode vir de variável de ambiente: o headless chama `env_clear()` e depois só passa `agent_env`, e
os argumentos são montados pelo app, não pelo teste. Por isso ele fica num `<nome>.fake.json` ao lado do executável,
que o próprio binário acha por `current_exe()`.

**Como aplicar:** para outro agente falso nos testes, copie `FAKE_AGENT` para o PATH e escreva o JSON com os campos
descritos em `tests/support/fake_agent.rs` (`stdout`, `echo_args`, `echo_stdin`, `fail_on_arg`, `log`…). Um `[[bin]]`
de teste também é compilado por `cargo build --workspace`, então o nome deve deixar claro que não é parte do app.

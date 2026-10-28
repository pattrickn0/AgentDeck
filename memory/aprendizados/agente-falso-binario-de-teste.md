---
title: The fake agent in tests is a crate binary, not a shell script
summary: A test `[[bin]]` with `CARGO_BIN_EXE_<name>` runs on Windows and Unix; behavior comes from a JSON alongside it, because headless clears the environment.
touches: [crates/core/tests/bridge.rs, crates/core/tests/support/fake_agent.rs, crates/core/Cargo.toml]
---
`tests/bridge.rs` needs a fake "claude" that prints text, reads standard input, and exits with a code. A
`.cmd` would run into `cmd.exe`'s codepage (accents in prompts) and quotes/`%` in arguments; therefore, the fake is the
test binary `fake_agent` (`[[bin]]` in `crates/core/Cargo.toml`, source in `tests/support/fake_agent.rs`).

**Why:** `CARGO_BIN_EXE_<name>` (defined only for binaries of the package itself) provides the absolute path to the already
compiled binary, and the test copies it to the search directory with the agent's name (`claude.exe` on Windows, `claude` on Unix).
The behavior cannot come from an environment variable: headless calls `env_clear()` and then only passes `agent_env`, and
arguments are constructed by the app, not by the test. Therefore, it lives in a `<name>.fake.json` alongside the executable,
which the binary itself finds via `current_exe()`.

**How to apply:** for another fake agent in tests, copy `FAKE_AGENT` to PATH and write the JSON with the fields
described in `tests/support/fake_agent.rs` (`stdout`, `echo_args`, `echo_stdin`, `fail_on_arg`, `log`...). A test `[[bin]]`
is also compiled by `cargo build --workspace`, so the name should make it clear that it is not part of the app.

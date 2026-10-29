---
title: The Antigravity token on Linux is in the Secret Service, not in oauth_creds.json
summary: `agy` stores the token in the keyring (`service=gemini`, `username=antigravity`); reading only the file leaves quota as "no data".
touches: [crates/core/src/usage/antigravity.rs, crates/core/Cargo.toml]
---
On Linux, `agy` stores the OAuth token in the **Secret Service** (the GNOME/KDE keyring), with the attributes
`service=gemini` and `username=antigravity` and the label "Password for 'antigravity' on 'gemini'". The secret is the
same JSON as in the Windows Credential Manager (`{"token":{"access_token",...,"expiry"},"auth_method"}`),
which `parse_creds_json` already understands. `~/.gemini/oauth_creds.json` is a different thing (the Gemini CLI login) and
might not even exist; this is why the Antigravity quota showed "no data" on Linux.

**Why:** the CLI uses `zalando/go-keyring`, which on Linux talks D-Bus to the Secret Service (on Windows,
`CredRead`). It is not a file the app can read directly.

**How to apply:** `secret-service` 5 (pure Rust, via `zbus`; adds no C libraries to CI) with the feature
`rt-async-io-crypto-rust` — without a crypto feature it does not even compile — and session `EncryptionType::Plain`, as
`go-keyring` opens it. Read in its own thread with a deadline (3 s) and only `found.unlocked`: unlocking an item would open
a password prompt in the background. Without D-Bus (SSH, container), lookup fails and the app falls back to files. There is a
third location: with `GEMINI_FORCE_FILE_STORAGE=true` (SSH, container, WSL), `agy` writes the same JSON to
`~/.gemini/antigravity-cli/antigravity-oauth-token`. To check manually:
`secret-tool lookup service gemini username antigravity`.

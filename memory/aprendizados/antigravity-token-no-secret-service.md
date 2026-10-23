---
titulo: O token do Antigravity no Linux está no Secret Service, não no oauth_creds.json
resumo: O `agy` grava o token no chaveiro (`service=gemini`, `username=antigravity`); ler só o arquivo deixa a cota "sem dados".
touches: [crates/core/src/usage/antigravity.rs, crates/core/Cargo.toml]
---
No Linux o `agy` guarda o token OAuth no **Secret Service** (o chaveiro do GNOME/KDE), com os atributos
`service=gemini` e `username=antigravity` e o rótulo "Password for 'antigravity' on 'gemini'". O segredo é o
mesmo JSON do Gerenciador de Credenciais do Windows (`{"token":{"access_token",…,"expiry"},"auth_method"}`),
que o `parse_creds_json` já entende. O `~/.gemini/oauth_creds.json` é outra coisa (o login do Gemini CLI) e
pode nem existir; por isso a cota do Antigravity ficava "sem dados" no Linux.

**Por quê:** o CLI usa o `zalando/go-keyring`, que no Linux fala D-Bus com o Secret Service (no Windows,
`CredRead`). Não é um arquivo que o app possa ler direto.

**Como aplicar:** `secret-service` 5 (Rust puro, por `zbus`; não acrescenta biblioteca C à CI) com a feature
`rt-async-io-crypto-rust` — sem uma feature de cripto ele nem compila — e sessão `EncryptionType::Plain`, como
o `go-keyring` abre. Ler numa thread própria com prazo (3 s) e só `found.unlocked`: destravar um item abriria
um pedido de senha em segundo plano. Sem D-Bus (SSH, contêiner) a busca falha e o app cai nos arquivos. Há um
terceiro lugar: com `GEMINI_FORCE_FILE_STORAGE=true` (SSH, contêiner, WSL) o `agy` grava o mesmo JSON em
`~/.gemini/antigravity-cli/antigravity-oauth-token`. Para conferir à mão:
`secret-tool lookup service gemini username antigravity`.

# 🤝 Guia de Contribuição

## Ambiente

- Windows 10/11 64-bit (alvo principal). Linux funciona para desenvolvimento.
- [Rust stable](https://rustup.rs) — `rustup update` de vez em quando.
- Linux: `sudo apt install libasound2-dev libgtk-3-dev libxkbcommon-dev`.

```powershell
git clone https://github.com/pattrickn0/CommandDeck-Terminal.git
cd CommandDeck-Terminal
cargo run -p commanddeck          # roda o app (modo debug)
```

## Antes de abrir um PR

```bash
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

O CI (`.github/workflows/ci.yml`) roda exatamente isso no Windows e no Linux.

## Princípios do projeto

1. **Privacidade:** áudio `.wav` é apagado logo após a transcrição; anexos e prompts longos ficam retidos só para as últimas 5 sessões.
2. **Workspace isolado:** agentes trabalham em `workspace/` por padrão. Nunca criar `AGENTS.md` na pasta do usuário.
3. **Ponte local segura:** só `127.0.0.1`, token aleatório por sessão e validação do cabeçalho `Host`.
4. **UI nunca trava:** tudo que pode bloquear (processos, rede, transcrição) roda em thread.
5. Textos para o usuário em pt-BR; comentários de código em português.

Veja [docs/MANUTENCAO.md](docs/MANUTENCAO.md) para o mapa do código e receitas comuns.

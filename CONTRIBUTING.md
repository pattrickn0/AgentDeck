# 🤝 Guia de Contribuição

## Ambiente

- Windows 10/11 64-bit (alvo principal). Linux funciona para desenvolvimento.
- [Rust stable](https://rustup.rs), 1.89 ou mais novo: `rustup update` de vez em quando.
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

O CI (`.github/workflows/ci.yml`) roda isso no Windows e no Linux (a formatação com `cargo fmt --all -- --check`;
o estilo está em `rustfmt.toml`).

## Princípios do projeto

1. **Privacidade:** áudio `.wav` é apagado logo após a transcrição; anexos e prompts longos ficam retidos só para as últimas 5 sessões.
2. **Pasta isolada:** cada agente trabalha na pasta do **projeto** em que a aba foi aberta (ou, numa tarefa, na worktree própria dela, `<repo>/.agentdeck/worktrees/<id>`), nunca nos arquivos do app. O app só cria `AGENTS.md` dentro da própria pasta do app (`workspace/`); nas pastas dos projetos só atualiza o bloco de um `AGENTS.md` que já exista, e nunca escreve na pasta pessoal do usuário.
3. **Ponte local segura:** só `127.0.0.1`, um token aleatório **por processo de agente** (a ponte sabe quem pergunta pelo token) e validação do cabeçalho `Host`. Texto vindo de outro agente nunca vai cru para um terminal (`util::terminal_text`) nem para a linha de comando de um shim `.cmd`.
4. **UI nunca trava:** tudo que pode bloquear (processos, rede, transcrição) roda em thread.
5. Textos para o usuário em pt-BR; comentários de código em português.

6. **Documentação junto com o código:** mudou comportamento, protocolo ou uma pendência? Atualize o documento
   correspondente em [memory/](memory/GERAL.md) no mesmo commit. Pendência concluída sai de `pendencias.md` e vai para
   `historico.md`.
7. **Memória do projeto:** algo que não é óbvio no código e que custou tempo (uma armadilha, uma decisão, uma convenção)
   vira um arquivo em [memory/aprendizados/](memory/aprendizados/README.md), no mesmo commit.

Veja [memory/MANUTENCAO.md](memory/MANUTENCAO.md) para o mapa do código e receitas comuns, e
[memory/GERAL.md](memory/GERAL.md) para a visão geral e o mapa dos documentos.

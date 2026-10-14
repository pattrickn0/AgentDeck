# 🗂️ Workspace Padrão dos Agentes (`workspace/`)

Este diretório serve como o **espaço de trabalho de desenvolvimento (sandbox) padrão** para os agentes de inteligência artificial (**Claude Code** e **Google Antigravity**) que operam através do **CommandDeck Terminal**.

---

## 🛡️ Princípio de Isolamento

Para evitar que os agentes executem alterações acidentais nos arquivos de infraestrutura da própria aplicação CommandDeck (`crates/`, `Cargo.toml`), todas as instâncias de terminal dos agentes são inicializadas por padrão com o diretório de trabalho apontado para esta pasta (`workspace/`).

- Qualquer arquivo, script, repositório ou código gerado pelas IAs durante seus testes ou tarefas será criado aqui.
- O workspace pode ser alterado a qualquer momento nas configurações do CommandDeck para apontar para qualquer projeto do seu disco rígido.

---

## 📄 Instruções para Agentes (`AGENTS.md`)

O arquivo [`AGENTS.md`](./AGENTS.md) presente nesta pasta é automaticamente injetado e lido pelos CLIs de IA no momento de inicialização. Ele instrui os agentes sobre:
1. Seu nome de agente ativo (via variável de ambiente `COMMANDDECK_AGENT`).
2. O comando shell `ask-agent <agente> "mensagem"` para consultar outro agente.
3. As diretrizes para troca de opiniões, revisão mútua de código e síntese colaborativa de tarefas.

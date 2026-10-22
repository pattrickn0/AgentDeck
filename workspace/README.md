# 🗂️ Workspace dos Agentes (`workspace/`)

Este diretório é o **espaço de trabalho (sandbox) de desenvolvimento original** dos agentes de inteligência artificial (**Claude Code**, **Google Antigravity** e os demais) que operam através do **AgentDeck**. Hoje o centro do app são os **projetos**: cada projeto é uma pasta escolhida pelo usuário, e as abas dele rodam lá. O `workspace/` continua existindo como a pasta do app para onde uma configuração antiga é migrada (`Config::migrate_legacy`) e onde o app cria o `AGENTS.md`.

---

## 🛡️ Princípio de Isolamento

Para evitar que os agentes executem alterações acidentais nos arquivos de infraestrutura da própria aplicação AgentDeck (`crates/`, `Cargo.toml`), as abas dos agentes rodam na pasta do **projeto** em que foram abertas (e, numa tarefa da mesa, na worktree própria dela, `<repo>/.agentdeck/worktrees/<id>`), não na pasta do app.

- Qualquer arquivo, script, repositório ou código gerado pelas IAs durante seus testes ou tarefas é criado na pasta do projeto ou da tarefa.
- Sem projeto aberto, o centro do app pede uma pasta; quem tinha só o workspace antigo ganha um projeto apontando para ele na primeira abertura.

---

## 📄 Instruções para Agentes (`AGENTS.md`)

O arquivo [`AGENTS.md`](./AGENTS.md) desta pasta é **gerado pelo app** (o bloco entre `<!-- agentdeck:start -->` e `<!-- agentdeck:end -->` é reescrito a cada início) e lido pelos CLIs de IA na inicialização. Nas pastas dos projetos o app só atualiza o bloco de um `AGENTS.md` que já exista. Ele instrui os agentes sobre:
1. Seu nome de agente ativo (via variável de ambiente `AGENTDECK_AGENT`).
2. O comando shell `ask-agent <agente> "mensagem"` para consultar outro agente.
3. As diretrizes para troca de opiniões, revisão mútua de código e síntese colaborativa de tarefas.

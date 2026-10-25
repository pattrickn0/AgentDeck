---
titulo: Acentos se perdem na entrada padrão do Windows
resumo: PowerShell 5.1 manda stdin em ASCII; texto com acento vai por --file.
touches: [crates/cli/**, crates/core/src/bridge.rs, crates/core/src/tasks/**, crates/core/src/util.rs]
---
No Windows PowerShell 5.1 o `$OutputEncoding` é US-ASCII: texto canalizado (`... | ask-agent x -`) ou passado pelo
shell perde todo caractere fora do ASCII (acentos e travessões viram `?`) antes de chegar ao `ask-agent`.
`ask-agent --file <arquivo>` lê o arquivo direto, em UTF-8 (ou UTF-16 com BOM, como o PowerShell 5.1 grava), por
`util::decode_text`.

**Por quê:** foi o primeiro bug real da mesa de tarefas (relatórios e briefs chegavam com `?`).
**Como aplicar:** relatório, feedback e pedido de tarefa, ou qualquer texto com acento, vão num arquivo
(`ask-agent --task done <id> --file <arq>`). Ao mexer no `ask-agent` ou na ponte, não troque `--file` por stdin e
mantenha `decode_text` na leitura.

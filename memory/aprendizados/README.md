# Aprendizados do projeto

Memória de longo prazo do AgentDeck: um arquivo `.md` por coisa que o projeto já aprendeu e que um próximo agente
não deveria precisar descobrir de novo (uma armadilha, uma decisão, uma convenção). Versionados no git, passam pela
mesma revisão e mescla que o código. Este `README.md` não conta como aprendizado.

## Formato

```
---
title: Accents are lost on the Windows standard input
summary: PowerShell 5.1 sends stdin as ASCII; text with accents goes through --file.
touches: [crates/core/src/bridge.rs, crates/cli/**]
---
The fact, then **Why:** and **How to apply:** in a few lines.
```

- `title` and `summary` (preferred, in English) or the old `titulo` and `resumo`: one line each, required. The summary is
  what shows in the index. New learnings are written in English.
- `touches`: globs relativos à raiz do repositório, em lista (`[a, b]`) ou em itens `- a`. `*` não cruza pasta, `**`
  cruza; um padrão sem `/` (`*.rs`) vale para o nome do arquivo em qualquer pasta. Vazio ou ausente = vale para o
  projeto todo.
- O corpo é obrigatório. Seja curto: o app entrega o texto inteiro até um orçamento de caracteres; o que passar vira
  só a linha do índice.
- Arquivo com frontmatter inválido é ignorado (com aviso no log do app); nunca derruba a tarefa.

## Como chegam ao agente

- No início de cada tarefa: o texto dos globais e dos que casam com arquivos citados no pedido, mais o índice dos
  outros.
- Na devolução e na reprovação de uma tarefa: os específicos dos arquivos que o feedback novo cita e que o agente ainda
  não tinha visto.
- Sob demanda: `ask-agent --learn [<arquivo>...]` imprime os que valem para esses arquivos; sem argumentos, mostra
  os globais por inteiro e o índice de todos os outros.

## Quando criar um

Só para o que **não é óbvio** no código e custou tempo ou causou erro. Nada de resumir o que o código já diz. Crie o
arquivo na branch da tarefa e cite-o no relatório. Nome: curto, em minúsculas, com hífens.

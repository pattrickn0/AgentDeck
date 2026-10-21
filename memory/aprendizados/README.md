# Aprendizados do projeto

Memória de longo prazo do AgentDeck: um arquivo `.md` por coisa que o projeto já aprendeu e que um próximo agente
não deveria precisar descobrir de novo (uma armadilha, uma decisão, uma convenção). Versionados no git, passam pela
mesma revisão e mescla que o código. Este `README.md` não conta como aprendizado.

## Formato

```
---
titulo: Acentos se perdem na entrada padrão do Windows
resumo: PowerShell 5.1 manda stdin em ASCII; texto com acento vai por --file.
touches: [crates/core/src/bridge.rs, crates/cli/**]
---
O fato, e depois **Por quê:** e **Como aplicar:** em poucas linhas.
```

- `titulo` e `resumo`: uma linha cada, obrigatórios. O resumo é o que aparece no índice.
- `touches`: globs relativos à raiz do repositório, em lista (`[a, b]`) ou em itens `- a`. `*` não cruza pasta, `**`
  cruza; um padrão sem `/` (`*.rs`) vale para o nome do arquivo em qualquer pasta. Vazio ou ausente = vale para o
  projeto todo.
- O corpo é obrigatório. Seja curto: o app entrega o texto inteiro até um orçamento de caracteres; o que passar vira
  só a linha do índice.
- Arquivo com frontmatter inválido é ignorado (com aviso no log do app); nunca derruba a tarefa.

## Como chegam ao agente

- No início de cada tarefa: o texto dos globais e dos que casam com arquivos citados no pedido, mais o índice dos
  outros.
- Sob demanda: `ask-agent --learn [<arquivo>...]` imprime os que valem para esses arquivos; sem argumentos, mostra
  os globais por inteiro e o índice de todos os outros.

## Quando criar um

Só para o que **não é óbvio** no código e custou tempo ou causou erro. Nada de resumir o que o código já diz. Crie o
arquivo na branch da tarefa e cite-o no relatório. Nome: curto, em minúsculas, com hífens.

---
titulo: A régua da memória (memeval): o teto de caracteres pesa mais que a co-mudança
resumo: cargo run -p agentdeck-core --example memeval mede recall/precisão/tamanho da entrega de aprendizados; em 24/10 a co-mudança não ganhou do controle dos arquivos mais mudados e o teto de 6000 caracteres era o gargalo.
touches: [crates/core/examples/memeval.rs, crates/core/src/learnings.rs, crates/core/src/situation.rs]
---
`crates/core/examples/memeval.rs` mede, nas tarefas mescladas (e nos commits diretos da `dev`), o que cada jeito de
entregar aprendizados teria acertado. Rode com o caminho **do Windows** do arquivo de tarefas (o binário não entende
`/c/Users/...` do Git Bash):

`cargo run --release -p agentdeck-core --example memeval -- . "C:\...\dados\tarefas\<projeto>.json" [--detalhe]`

O certo = aprendizados com `touches` que casam com o que a tarefa mudou, lidos **na base** da tarefa (o ponto em que a
branch saiu, não o `merge-base` da mesclagem: quem mesclou a `dev` na branch moveria a base). Os globais ficam fora da conta.

**O que a medição de 24/10 ensinou** (39 tarefas, 28 com aprendizado na base, histórico de 12 dias):
- O gargalo da entrega de hoje é o **teto de 6000 caracteres**, não a falta de palpite: a estratégia (a) acerta 46% dos
  certos com o teto e 60% sem ele (9000 caracteres já dão 56%), a ~+500 tokens por tarefa.
- A **co-mudança não ganhou do controle** "sugerir os arquivos que mais mudaram antes da base" (os centrais: `deck.rs`,
  `tasks.rs`, `app.rs`), nem para a memória nem para a previsão de arquivos (precisão dos vizinhos ~30% contra ~28%).
  Ordenar por lift (corrigir pela frequência) piorou a precisão. O decaimento pela idade (7 e 3 dias) não mudou nada.
- Os pedidos citam caminhos (36 de 39 tarefas); as mensagens dos commits quase nunca (12 de 58): a amostra de commits
  é fraca para (a) e (c).

**Por quê:** num repositório pequeno e jovem os mesmos poucos arquivos mudam em quase toda tarefa, então "mudou junto"
vira "é um arquivo central".
**Como aplicar:** antes de construir o grafo, refaça a medição quando houver mais histórico (hoje os resultados mudam
com 1 ou 2 tarefas a mais) e compare sempre com o controle `ctrl` da grade; só vale a co-mudança se ganhar dele.

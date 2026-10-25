---
titulo: Aba que precisa abrir recebe o pedido por arquivo, não digitado
resumo: Digitar num CLI que ainda sobe perde o texto; o prompt inicial é uma linha ASCII que manda ler um arquivo.
touches: [crates/core/src/tasks/**, crates/core/src/agents.rs, crates/core/src/models.rs, crates/core/src/delivery.rs]
---
Digitar o texto numa aba que ainda está abrindo (nova, ou processo encerrado) o perde: o CLI está subindo (visto no
Antigravity). Então o brief, a devolução ou a reprovação vão para `dados/tarefas/<projeto>-<id>-brief.md` (ou
`-feedback.md`) e `Delivery::boot` leva só uma linha ASCII (`tasks::boot_line`) mandando ler o arquivo.
`agents::spawn_spec_for` a põe na linha de comando, uma vez (`models::boot_prompt_flag`): `claude "<linha>"`,
`agy -i "<linha>"`, `opencode --prompt "<linha>"`.

**Por quê:** aspas, quebras de linha e acentos na linha de comando se perdem nos shims `.cmd` do Windows.
**Como aplicar:** não ponha o texto da tarefa na linha de comando nem digite na aba que ainda abre. Mensagem nova ao
agente da tarefa passa por `task_boot_prompt`; os CLIs sem `boot_prompt_flag` e a aba já viva seguem digitando.

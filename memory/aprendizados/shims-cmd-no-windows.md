---
titulo: Shims .cmd no Windows: duas passagens no PTY e prompt por arquivo no headless
resumo: Nas abas, cmd_safe_arg escapa para o cmd.exe /c e para o %* do shim; no headless, o .cmd é chamado direto e o prompt vai por arquivo.
touches: [crates/core/src/agents.rs, crates/core/src/headless.rs]
---
Um CLI instalado via npm ou pip no Windows é um shim `.cmd`. O `cmd.exe` interpreta `& | < > ^ % "` e quebras de linha,
e numa aba interativa (`portable-pty`) o texto passa por **duas** passagens do interpretador: a linha do
`cmd.exe /d /e:on /v:off /c <shim>` e a linha interna do shim que expande `%*`.

**Por quê:** apagar caracteres especiais quebrava caminhos válidos do Windows (como `C:\Users\Ana&Bia`), e passar um
prompt livre na linha de comando permitiria executar comandos pelo `cmd.exe`.

**Como aplicar:** nas abas (`spawn_spec_for`), todo argumento gerado pelo app passa por `cmd_safe_arg` (`agents.rs`),
que escapa para as duas passagens (`^^^c` fora de aspas, `%%cd:~,%` para `%`, com `/e:on /v:off`). Texto livre de outro
agente nunca entra na linha de comando de um shim: o prompt inicial de tarefa (`boot_prompt_flag`) é só uma linha ASCII
apontando para um arquivo, e no modo headless (`headless.rs`) o `.cmd` é chamado direto em `std::process::Command` (que
aplica o escape de lote da std) com o prompt gravado em `dados/prompts/` (`prompt_arg`).

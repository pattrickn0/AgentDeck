# Pendências

Lista de melhorias e limitações conhecidas, em ordem sugerida de prioridade.

## 1. Agente local (Ollama): contexto e comportamento

Problema observado: o modelo local esquece que é um agente ("age como chatbot") e para no meio da colaboração com outros agentes (ex.: Antigravity) em vez de continuar a conversa sozinho.

- [ ] **Configurar `num_ctx`**. O Ollama usa por padrão uma janela de contexto pequena (2–4k tokens). O prompt de sistema, as 6 ferramentas e o `AGENTS.md` ocupam boa parte dela, e o começo, onde está o "você é um agente", acaba cortado. Enviar `options.num_ctx` nas chamadas a `/api/chat` (ex.: 16k–32k), configurável em `Config`.
  - Arquivo: `crates/core/src/ollama.rs` e `crates/core/src/config.rs`.
- [ ] **Prompt de sistema mais curto para modelos pequenos (4B–9B).** O atual é longo e cheio de regras em caixa alta, que modelos pequenos seguem mal. Ter uma versão enxuta.
- [ ] **Instrução de colaboração mais perto do fim do contexto.** O lembrete de "continue conversando com o agente parceiro" hoje vai no fim de uma saída grande do `ask-agent`. Avaliar reinjetar a instrução como mensagem curta de sistema/usuário logo após o resultado.
- [ ] **Verificação antes de encerrar.** Hoje, quando o modelo responde em texto sem chamar ferramenta, o loop considera a tarefa concluída. Adicionar um passo de checagem ("a tarefa terminou ou falta chamar `ask-agent`/ferramentas?") antes de devolver a resposta.
- [ ] Revisar o limite de 25 passos (`MAX_STEPS`) para conversas longas entre agentes.

## 2. Injeção de contexto como um ponto único (para qualquer IA)

Hoje cada agente recebe instruções por um caminho diferente:

| Agente | Como recebe as instruções hoje |
| :-- | :-- |
| Claude | `--append-system-prompt` (`agents.rs` / `headless.rs`) |
| Antigravity | bloco marcado no `AGENTS.md` do workspace |
| Ollama | prompt fixo em `ollama.rs` (`system_prompt`) |

Proposta:

- [ ] **Perfis de agente em arquivo**, editáveis sem recompilar:
  - `prompts/_comum.md`: regras de colaboração válidas para todos (usar `ask-agent`, não resumir a Conversa integrada, não parar para perguntar ao usuário etc.);
  - `prompts/<agente>.md`: ajustes por agente (`ollama.md`, `claude.md`, `antigravity.md`...).
- [ ] **`ContextProvider` para todos os agentes.** O trait (`crates/core/src/context.rs`) hoje só é consultado pelo Ollama. Passar a consultá-lo também no executor headless e no spawn das abas, entregando o contexto pelo canal que cada CLI suporta (flag, `AGENTS.md` ou mensagem de sistema).
- [ ] **RAG local/remoto** como um `ContextProvider` registrado via `Core::register_context_provider`.

## 3. Terminal

- [ ] **Cliques e movimento do mouse em modo mouse.** Hoje só a roda do mouse chega a programas em tela cheia (`vim`, `htop`, menus interativos). Implementar o envio de clique/movimento nas sequências SGR (`\x1b[<...M`/`m`).
  - Arquivos: `crates/app/src/terminal/`.
- [ ] **IME no terminal.** Composição de IME (teclados CJK, seletor de emoji `Win+.`) funciona no compositor, mas não é encaminhada ao terminal. Acentos do português não são afetados.

## 4. Interface

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.

## 5. Validação

- [ ] Testar o app de verdade no Windows (o CI só compila e roda testes): abas, ConPTY, microfone/Whisper, som, arrastar arquivos, colar imagem e restauração de abas.

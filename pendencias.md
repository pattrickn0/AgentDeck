# Pendências

Lista de melhorias e limitações conhecidas, em ordem sugerida de prioridade.

## 0. Direção: IA local como serviço interno (adiado)

> **Adiado** até o projeto estar mais completo.

Decisão: com Claude e Antigravity disponíveis, o Ollama deixa de ser "agente par" e vira **infraestrutura** do app.

- [ ] **Embeddings locais para RAG.** Modelo de embeddings no Ollama (`nomic-embed-text` ou `bge-m3`, < 1 GB, cabe na RX 7600 8 GB).
  - Indexar o workspace em pedaços (*chunks*) e manter o índice atualizado pelo watcher existente (`workspace.rs`).
  - Expor a busca de duas formas: `ask-agent`/CLI `search` (sob demanda) e um `ContextProvider` com orçamento fixo de tokens (ex.: 1.500).
  - Índice local (ex.: em `dados/`), sem enviar nada para fora.
- [ ] **Compressor de contexto.** Modelo pequeno (`qwen3:8b`, 100% GPU) para resumir:
  - o histórico das **mesas**: hoje cada rodada reenvia a transcrição inteira (`table.rs`); passar a enviar um resumo + as últimas falas;
  - respostas longas no `ask-agent` antes de entregar ao agente que perguntou (`bridge.rs`), mantendo o original na Conversa integrada.
- [ ] **Medir tokens** (`prompt_eval_count` do Ollama, tamanho dos prompts enviados a Claude/Antigravity) para comparar antes/depois.
- [ ] **Configuração** em `Config`: modelo de embeddings, modelo compressor, limiares (ex.: comprimir acima de N caracteres), ligar/desligar.
- [ ] Aba Ollama-agente passa a ser opcional (os itens da seção 1 ficam como baixa prioridade).

## 1. Agente local (Ollama) — adiado: contexto e comportamento

> **Adiado** até o projeto estar mais completo.

Problema observado: o modelo local esquece que é um agente ("age como chatbot") e para no meio da colaboração com outros agentes (ex.: Antigravity) em vez de continuar a conversa sozinho.

- [ ] **Configurar `num_ctx`**. O Ollama usa por padrão uma janela de contexto pequena (2–4k tokens). O prompt de sistema, as 6 ferramentas e o `AGENTS.md` ocupam boa parte dela, e o começo, onde está o "você é um agente", acaba cortado. Enviar `options.num_ctx` nas chamadas a `/api/chat` (ex.: 16k–32k), configurável em `Config`.
  - Arquivo: `crates/core/src/ollama.rs` e `crates/core/src/config.rs`.
- [ ] **Prompt de sistema mais curto para modelos pequenos (4B–9B).** O atual é longo e cheio de regras em caixa alta, que modelos pequenos seguem mal. Ter uma versão enxuta.
- [ ] **Instrução de colaboração mais perto do fim do contexto.** O lembrete de "continue conversando com o agente parceiro" hoje vai no fim de uma saída grande do `ask-agent`. Avaliar reinjetar a instrução como mensagem curta de sistema/usuário logo após o resultado.
- [ ] **Verificação antes de encerrar.** Hoje, quando o modelo responde em texto sem chamar ferramenta, o loop considera a tarefa concluída. Adicionar um passo de checagem ("a tarefa terminou ou falta chamar `ask-agent`/ferramentas?") antes de devolver a resposta.
- [ ] Revisar o limite de 25 passos (`MAX_STEPS`) para conversas longas entre agentes.

## 2. Injeção de contexto como um ponto único (para qualquer IA) — adiado

> **Adiado** (gestão de contexto) até o projeto estar mais completo.

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

Nada pendente.

## 4. Interface

- [ ] Emojis monocromáticos (limitação do egui). Avaliar fonte de emoji colorida ou renderização própria.
- [ ] Select, spinner e tooltips desenhados à mão imitando o Chromium; desfoque atrás do overlay de arrastar é aproximado.
- [ ] Animação suave (hover) nos botões do estado vazio.

## 5. Validação

- [ ] **Agentes herdam o ambiente do terminal pai.** Ao abrir o app de dentro de um terminal que já roda o CommandDeck, a nova instância herda `COMMANDDECK_PORT`, `COMMANDDECK_TOKEN` e `COMMANDDECK_AGENT` (e pode compartilhar a pasta de dados). Suspeita de ter derrubado o terminal em uso numa sessão de testes. Limpar essas variáveis ao iniciar e/ou isolar `dados/` por instância.
- [ ] **Testes de integração dependem do `agy` não estar instalado.** `workspace_agents_md_and_spawn_specs` (`crates/core/tests/bridge.rs`) falha quando o Antigravity está instalado na máquina, porque espera o estado "não instalado". Tornar o teste independente do ambiente (PATH e `LOCALAPPDATA` isolados).
- [ ] **Testar o IME com um IME real** (teclado CJK, seletor de emoji `Win+.`): a lógica está ligada, mas só foi verificada por compilação e testes do cursor; não foi exercitada com composição de verdade.

## Concluído

- [x] **Download do Whisper robusto** (motor e modelo): retomada por `Range`, retry com espera crescente, detecção de conexão parada, redirecionamentos, verificação de SHA-256 e barra de progresso no cabeçalho da Conversa integrada; suporte a Linux. Módulo `crates/core/src/download.rs`.
- [x] **Mouse no terminal** (`crates/app/src/terminal/view.rs`, `input.rs`): clique, soltar, arrasto e movimento são enviados ao programa quando ele ativa os modos 1000/1002/1003, em SGR (1006) ou no formato legado; Shift devolve o mouse ao terminal para selecionar texto. Coberto por testes headless com eventos reais do egui.
- [x] **IME no terminal**: com o terminal focado o IME do sistema é habilitado ancorado no cursor; a confirmação vai ao processo como texto digitado e a pré-edição aparece sublinhada sobre o cursor (antes o evento era descartado). Falta só testar com um IME real (teclado CJK, `Win+.`).
- [x] **Abrir a aba do agente sozinho** quando outro agente o chama pelo `ask-agent` (terminal normal em segundo plano, com teto de 3 abas, opção em Configurações e sem reabrir o que o usuário fechou). A conversa segue pela Conversa integrada. Ver `ARCHITECTURE.md`.
- [x] **Colar no Windows derrubava o app** (corrupção de heap): o clipboard agora só é lido na thread da interface. Reproduzido e corrigido.
- [x] **Validação no Windows** (feita à mão): abas, ConPTY, microfone/Whisper, som, arrastar arquivos, colar imagem e restauração de abas.

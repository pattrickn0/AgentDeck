# Manual de Operação e Uso do CommandDeck Terminal

Este manual descreve em detalhes todos os recursos visuais, atalhos de teclado, fluxos de trabalho e modos de interação disponibilizados pela interface do **CommandDeck Terminal**.

---

## 🖥️ Layout da Interface

A interface do CommandDeck é dividida em quatro regiões funcionais principais:

```
┌────────────────────────────────────────────────────────────────────────┐
│ 1. TOPBAR: [Ctrl+B] [Aba Claude] [Aba Antigravity] [+] ... [📁 Workspace] [⚙️]│
├───────────────┬────────────────────────────────────────────────────────┤
│ 2. SIDEBAR    │ 3. ÁREA DE TERMINAIS (xterm.js)                        │
│    (Explorador│    Exibe a sessão interativa real do CLI selecionado.  │
│     de        │    (Claude Code, Antigravity CLI ou PowerShell)        │
│     Arquivos) ├────────────────────────────────────────────────────────┤
│               │ ── SPLITTER HORIZONTAL (Arraste para redimensionar) ── │
│   📁 src/     ├────────────────────────────────────────────────────────┤
│   📄 main.js  │ 4. ÁREA INFERIOR                                       │
│   📄 app.html │    ┌──────────────────────────────────────────────────┐│
│               │    │ [▾ Conversa integrada]                   [Limpar]││
│               │    │ Feed de mensagens entre agentes e mesas de debate││
│               │    ├──────────────────────────────────────────────────┤│
│               │    │ [Enviar para: Aba Ativa ▼] [Chips de Anexos]    ││
│               │    │ [Textarea de Prompt...  ] [×][🎤] [📎] [ Enviar ]││
│               │    └──────────────────────────────────────────────────┘│
└───────────────┴────────────────────────────────────────────────────────┘
```

---

## 🗂️ 1. Gerenciamento de Abas e Terminais

O CommandDeck hospeda instâncias completas de pseudo-terminais (`node-pty`) conectadas aos binários oficiais:

- **Abas Disponíveis:**
  - **Claude (Laranja - `#d97757`):** Sessão interativa oficial do `claude.exe`. Recebe instruções da ponte via `--append-system-prompt`.
  - **Antigravity (Azul - `#4f8cf7`):** Sessão interativa do `agy.exe`. Recebe instruções da ponte através de bloco delimitado em `AGENTS.md`.
  - **Ollama (Amarelo - `#f59e0b`):** Sessão interativa com agente autônomo local (`bin/ollama-agent.js`). Conecta-se aos modelos locais (como `qwen3.5:9b`), equipado com ferramentas completas de workspace para ler, criar, editar arquivos e rodar comandos PowerShell com feedback visual em tempo real.
  - **Terminal / PowerShell (Cinza - `#9aa0a6`):** Sessão padrão do PowerShell (`powershell.exe -NoLogo`) para tarefas de administração local.

- **Atalhos e Ações:**
  - `Ctrl + T`: Abre imediatamente uma nova aba com o tipo do agente atualmente ativo (ou clica no botão `+` para escolher o agente no menu dropdown).
  - `Ctrl + Tab` / `Ctrl + Shift + Tab`: Alterna sequencialmente entre as abas abertas.
  - **Fechar Aba:** Clique no ícone `×` à direita do título da aba. O processo correspondente é finalizado graciosamente via IPC.
  - **Reiniciar Processo Encerrado:** Caso o CLI saia ou seja finalizado, a aba exibe o aviso de processo encerrado e permite reiniciar pressionando `Enter`.

---

## 📁 2. Explorador de Arquivos (Barra Lateral)

A barra lateral exibe a árvore de diretórios do workspace selecionado em tempo real:

- **Recolher / Expandir:** Pressione `Ctrl + B` ou clique no botão chevron no canto superior esquerdo da barra de topo.
- **Redimensionar:** Clique e arraste o divisor vertical entre a barra lateral e os terminais (largura ajustável entre 150px e 480px).
- **Inserção com 1 Clique (Debounce Inteligente):**
  - Um clique simples insere a referência `@caminho/do/arquivo` no campo de texto do compositor (com aspas se contiver espaços: `@"caminho com espaço"`).
- **Abertura Externa com Duplo Clique:**
  - Dois cliques rápidos sobre um arquivo cancelam a inserção de texto e acionam o comando nativo do sistema operacional (`shell.openPath`), abrindo o arquivo no seu editor padrão sem poluir o compositor com referências duplicadas.
- **Badges de Extensão Estilizados:**
  - Marcadores visuais: `JS` (amarelo), `TS` (azul), `{ }` (JSON verde), `M↓` (Markdown ciano), `<>` (HTML laranja), `#` (CSS azul claro) e `Py` (Python).

---

## 💬 3. Conversa Integrada (Feed de Agentes)

O painel "Conversa integrada" registra em tempo real toda a interação que ocorre nos bastidores:

- **Mensagens Registradas:**
  - Consultas autônomas entre agentes disparadas via comando `ask-agent`.
  - Respostas devolvidas pelos agentes em modo headless.
  - Pensamento/raciocínio intermediário durante mesas de debate (`thinking ●`).
  - Avisos do sistema e notificações de download de modelos do Whisper.
- **Recolhimento / Expansão:** Clique em `▾ Conversa integrada` para minimizar o feed e priorizar a visualização do terminal.
- **Badge de Mensagens Não Lidas:** Marcador numérico vermelho indicando novas mensagens recebidas enquanto o painel estiver recolhido.
- **Limpar Feed:** O botão `Limpar` reseta o histórico visual exibido no painel sem afetar os processos em execução.

---

## ✍️ 4. Barra de Composição (Composer)

A barra de composição no rodapé unifica a entrada de comandos para todos os agentes:

### Modos de Destino ("Enviar para")
1. **Aba ativa:** Envia o texto digitado diretamente para o terminal visível no momento.
2. **Todos os agentes abertos:** Dispara o prompt simultaneamente para todas as abas ativas do Claude e do Antigravity.
3. **Mesa Colaborativa (`Mesa: Agente A → Agente B`):** Dispara uma mesa de debate automatizada por rodadas entre os agentes selecionados com síntese de encerramento.

### Envio de Mensagens e Atalhos
- `Enter`: Envia a mensagem imediatamente para o destino selecionado.
- `Shift + Enter`: Insere uma nova linha no campo de texto sem enviar.
- `Ctrl + L`: Move o foco diretamente para a caixa de composição.
- **Auto-Resize Inteligente:** O campo de texto expande sua altura dinamicamente conforme o volume de linhas digitadas (até 260px).

---

## 📎 5. Sistema de Anexos, Imagens e Retenção

O CommandDeck oferece suporte multimodal com cópia assíncrona:

1. **Botão de Clipe (`📎`):** Abre a caixa de diálogo para seleção de múltiplos arquivos com cópia em background.
2. **Arrastar e Soltar (Drag & Drop):** Solte arquivos ou imagens na janela sob o overlay visual azul.
3. **Colar Imagens Direto da Área de Transferência (`Ctrl + V`):** Capturas de tela copiadas são salvas automaticamente como PNG e anexadas como chip.
4. **Política de Retenção de 5 Sessões:** Prompts longos e anexos de imagem são preservados para as 5 sessões mais recentes. Resíduos de sessões mais antigas são limpos automaticamente na inicialização.

---

## 🎙️ 6. Transcrição de Voz Local com Botão de Cancelamento

O conjunto de microfone no compositor oferece captura de áudio com privacidade total:

1. **Gravar Áudio:** Clique no ícone de microfone (`🎤`). O botão pulsa em vermelho e o botão de cancelamento (`×`) surge ao lado.
2. **Cancelar Gravação:** Clique no botão de cancelamento (`×`) ao lado do microfone. A gravação é descartada imediatamente em memória, sem gerar arquivos e sem gastar processamento.
3. **Concluir Gravação:** Clique novamente no microfone. O áudio é transcrito via Whisper, inserido no prompt e o arquivo `.wav` é **excluído imediatamente do disco**.
4. **Envio Imediato Pós-Gravação:** Clicar em "Enviar" durante a gravação agenda o envio automático assim que a fala for transcrita.

---

## ⚙️ 7. Menu de Configurações e Notificações Sonoras

No canto superior direito da barra de topo (ícone de engrenagem `⚙️`):
- **Sons de Notificação:** Ativa ou desativa os alertas sonoros suaves emitidos na conclusão de tarefas longas ou término de mesas de orquestração.
- **Testar Som:** Reproduz o chime harmônico sintetizado via Web Audio API.

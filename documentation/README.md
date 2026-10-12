# 📚 Centro de Documentação Técnica do CommandDeck Terminal

Bem-vindo à base de conhecimento central do **CommandDeck Terminal** (anteriormente JWIC). Todos os manuais, contratos de governança, arquitetura técnica, subsistemas e diagnósticos operacionais estão documentados e versionados nesta pasta.

---

## 🗺️ Mapa da Documentação

A documentação está estruturada em seis pilares complementares:

```
documentation/
├── README.md                          # Índice central e guia de navegação
├── setup-and-troubleshooting.md       # Instalação, login nos CLIs e resolução de problemas
├── architecture.md                    # Arquitetura de software, Electron, PTY e segurança
├── agents-protocol.md                 # Protocolo ask-agent, bridge HTTP e mesas de colaboração
├── whisper-transcription.md           # Transcrição neural local, hardware e descarte imediato
├── user-manual.md                     # Manual operacional, atalhos, compositor e anexos
└── limitations-and-roadmap.md         # Auditoria técnica, limitações, riscos e plano futuro
```

---

## 📖 Resumo dos Documentos

### 1. [Guia de Instalação e Solução de Problemas (`setup-and-troubleshooting.md`)](./setup-and-troubleshooting.md)
- Requisitos de hardware e sistema operacional (Windows 10/11 x64).
- Instalação e autenticação prévia nos CLIs oficiais: **Claude Code** (`claude`) e **Antigravity CLI** (`agy`).
- Instalação das dependências do CommandDeck e configuração do atalho sem console.
- Configuração e compilação do subsistema Whisper (Vulkan SDK, CUDA, CPU).
- Tabela de diagnóstico de falhas frequentes (*troubleshooting*).

### 2. [Arquitetura Técnica do Sistema (`architecture.md`)](./architecture.md)
- Diagrama completo de componentes e fluxos entre processos (Main, Preload e Renderer).
- Pseudo-Terminais com `node-pty` e Windows Pseudo Console (ConPTY) com throttle de redimensionamento.
- Camada de isolamento e segurança com ContextBridge e Content Security Policy (CSP).
- Gerenciamento de workspace isolado, limpeza de arquivos temporários e persistência em `config.json`.

### 3. [Protocolo de Comunicação Agente ↔ Agente (`agents-protocol.md`)](./agents-protocol.md)
- Mecanismo e variáveis de ambiente do utilitário `ask-agent` (`JWIC_PORT`, `JWIC_TOKEN`, etc.).
- Fila assíncrona de exclusão mútua por par de agentes no endpoint `/ask`.
- Especificação completa da API HTTP REST local interna (`/ask` e `/agents`) com validação de Host.
- Gerenciamento de sessões com memória por par de agentes (`sessions.set`).
- Prevenção de recursão infinita (`MAX_DEPTH`) e auto-chamadas.
- Orquestração de **Mesas Colaborativas** multirrodadas com fechamento de síntese final.

### 4. [Subsistema de Transcrição Local Whisper (`whisper-transcription.md`)](./whisper-transcription.md)
- Engenharia de captura e resampling via Web Audio API (conversão nativa para WAV PCM 16kHz mono).
- Botão integrado de cancelamento de áudio sem processamento.
- Descarte imediato de arquivos `.wav` após transcrição e retenção das 5 últimas sessões para prompts/capturas.
- Detecção e priorização de aceleração por hardware: NVIDIA CUDA → Vulkan (AMD/Intel) → CPU AVX2.
- Parâmetros de linha de comando do motor `whisper-cli.exe` e dicionário fonético contextual.

### 5. [Manual de Operação e Uso (`user-manual.md`)](./user-manual.md)
- Visão geral da interface gráfica e regiões funcionais.
- Gerenciamento de abas, navegação por teclado (`Ctrl+T`, `Ctrl+Tab`, `Ctrl+B`).
- Explorador de arquivos com debounce anti-conflito de cliques (1 clique insere `@caminho`, 2 cliques abre no SO).
- Caixa de composição multimodal (Composer), atalhos de envio (`Enter` vs `Shift+Enter`).
- Gestão de anexos assíncrona, arrastar e soltar (Drag & Drop) e colagem direta de prints (`Ctrl+V`).

### 6. [Limitações Técnicas, Riscos e Roadmap (`limitations-and-roadmap.md`)](./limitations-and-roadmap.md)
- Auditoria técnica profunda cobrindo backend, segurança, frontend e usabilidade.
- Matriz de limitações categorizadas por gravidade e soluções recomendadas.
- Plano de ação estruturado em três fases de evolução incremental.

---

> [!NOTE]
> Esta documentação é o repositório oficial da verdade técnica do projeto. Ao introduzir alterações em componentes de código, certifique-se de atualizar os documentos correspondentes.

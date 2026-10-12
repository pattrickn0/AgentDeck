# Mapeamento de Limitações Técnicas, Riscos e Roadmap (CommandDeck Terminal)

Este documento consolida a auditoria técnica exaustiva realizada sobre a arquitetura, backend, frontend e subsistemas do **CommandDeck Terminal**. Ele cataloga as limitações operacionais, melhorias recentemente aplicadas e o roadmap para evolução da plataforma.

---

## 🎯 Sumário Executivo do Diagnóstico

O CommandDeck Terminal é uma solução de alta produtividade para unificar os CLIs líderes de mercado (Claude Code e Antigravity CLI) com terminais interativos reais, transcrição neural estritamente local e colaboração autônoma entre agentes.

Durante a última revisão de engenharia, melhorias substanciais foram implementadas:
- ✅ **Throttle no ConPTY:** Redimensionamento protegido com debounce de 60ms;
- ✅ **Fila Serial no Bridge HTTP:** Eliminação de condições de corrida em chamadas concorrentes ao `/ask`;
- ✅ **Timeout Explícito no Claude:** Limite de 10 minutos para matar processos travados;
- ✅ **Resolução Precisa do Claude CLI:** Falsos positivos eliminados com checagem de binário;
- ✅ **Descarte Imediato de Áudio WAV:** Gravações de áudio são apagadas imediatamente após a transcrição;
- ✅ **Botão de Cancelamento de Áudio Integrado:** Cancelamento instantâneo de voz sem processamento;
- ✅ **Cópia Assíncrona de Anexos:** Migração de `copyFileSync` para `copyFile` assíncrono;
- ✅ **Política de Retenção de 5 Sessões:** Prompts e capturas de sessões anteriores são limpos automaticamente;
- ✅ **Proteção de Workspace:** Não criação de arquivos `AGENTS.md` fora da pasta do projeto e workspace isolado padrão;
- ✅ **Debounce na Árvore de Arquivos:** Eliminação do conflito entre clique simples e duplo.

---

## ⚠️ Matriz de Limitações Remanescentes e Riscos

### 1. Processos Nativos e Integração com CLIs

#### 1.1. Ausência de Streaming no Modo Headless e Mesas de Debate
- **Impacto:** **ALTA (UX)**
- **Detalhes:** O método `runHeadless` executa os comandos `claude -p` e `agy -p` aguardando a finalização completa do processo (`child.on('close')`) para emitir a resposta em bloco. Em tarefas complexas com pensamento prolongado e chamadas de ferramentas, cada rodada pode levar de 1 a 3 minutos. O usuário não tem visibilidade incremental do progresso (tokens, arquivos lidos, comandos rodados) durante a execução, vendo apenas um indicador de carregamento.
- **Solução Recomendada:** Implementar streaming lendo incrementalmente o `stdout` dos processos e atualizando o painel de feed em tempo real.

---

### 2. Subsistema de Áudio e Whisper.cpp

#### 2.1. Download de Binários Executáveis sem Verificação Criptográfica (Checksum)
- **Impacto:** **CRÍTICA (Segurança)**
- **Detalhes:** Os binários do whisper.cpp (`whisper-bin-x64.zip` e `whisper-bin-win-cuda-12.4.0-x64.zip`) e o modelo neural (`ggml-large-v3-turbo.bin` de 1,6 GB) são baixados diretamente da internet e extraídos/executados sem checagem de hash SHA-256.
- **Solução Recomendada:** Fixar hashes SHA-256 conhecidos no código-fonte e validar o checksum antes de descompactar e executar qualquer arquivo executável baixado da rede.

#### 2.2. Download Monolítico sem Suporte a Range Requests
- **Impacto:** **ALTA (Resiliência)**
- **Detalhes:** O modelo neural possui 1,6 GB. O cliente HTTP atual não suporta retomada de download (`Range: bytes=...`). Caso a conexão caia aos 95%, o download é descartado e reiniciado do zero.
- **Solução Recomendada:** Adicionar suporte a requisições com cabeçalho `Range` e retries com backoff exponencial.

---

### 3. Interface, Terminal (xterm.js) e UX

#### 3.1. Desempenho do Renderizador xterm.js Baseado em DOM
- **Impacto:** **ALTA (Performance de UI)**
- **Detalhes:** O xterm.js utiliza o renderizador DOM padrão. Em fluxos contínuos de saída massiva de texto de LLMs, o recálculo de layout do Chromium gera quedas de quadros e atrasos na digitação.
- **Solução Recomendada:** Incorporar o `@xterm/addon-webgl` com fallback para `@xterm/addon-canvas`.

#### 3.2. Conflito no Atalho `Ctrl+C` com Texto Selecionado
- **Impacto:** **MÉDIA (Usabilidade)**
- **Detalhes:** Se o usuário tiver texto selecionado no terminal, o atalho `Ctrl+C` prioriza a cópia do texto para a área de transferência e não envia o caractere de interrupção SIGINT (`\x03`) ao processo.
- **Solução Recomendada:** Limpar a seleção após a cópia ou enviar SIGINT se `Ctrl+C` for pressionado consecutivamente.

---

## 🗺️ Roadmap de Evolução Técnica

### Fase 1: Integridade e Download Resiliente do Whisper (Próxima Etapa)
- [ ] Implementar verificação de hash SHA-256 para binários do Whisper e modelo neural.
- [ ] Adicionar suporte a HTTP `Range: bytes=...` com retries para downloads pesados.
- [ ] Exibir barra de progresso percentual na interface durante o download do modelo de 1,6 GB.

### Fase 2: Streaming e Renderização Acelerada
- [ ] Integrar `@xterm/addon-webgl` com fallback para Canvas no xterm.js.
- [ ] Implementar streaming de tokens/saída em tempo real para as mesas de debate e `ask-agent`.
- [ ] Adicionar suporte a divisão de visualização (Split View lado a lado: Claude | Antigravity).

### Fase 3: Operações Avançadas e Expansão de Plataforma
- [ ] Implementar menu de contexto na árvore de arquivos (Criar Arquivo, Nova Pasta, Renomear, Excluir).
- [ ] Expandir suporte oficial para ambientes Linux e macOS.

# Guia de Instalação, Configuração e Solução de Problemas (CommandDeck Terminal)

Este documento fornece as instruções passo a passo para instalação, pré-requisitos, configuração de ambiente e resolução de problemas técnicos (troubleshooting) do **CommandDeck Terminal**.

---

## 💻 Requisitos de Sistema

### Sistema Operacional
- **Windows 10 ou 11 (64-bit)** (recomendado).
- *Nota sobre Linux / macOS:* Atualmente, o CommandDeck foi projetado com dependências do ecossistema Windows (`conpty`, `powershell.exe`, `where.exe` e scripts `.cmd`).

### Pré-requisitos de Software
1. **Node.js**: Versão 18.x, 20.x ou superior.
2. **NPM**: Versão 9.x ou superior.
3. **PowerShell**: PowerShell 5.1 (nativo do Windows) ou PowerShell 7+ (Core).
4. **Git**: Para controle de versão e operações de repositório.

---

## 🤖 Instalação e Autenticação dos CLIs Oficiais

O CommandDeck não substitui nem emula as credenciais dos CLIs; ele orquestra os **binários oficiais** instalados localmente. Antes de abrir o CommandDeck, certifique-se de instalar e autenticar cada CLI no terminal do seu sistema operacional.

### 1. Claude Code CLI (`claude`)
```powershell
# Instalação global via npm:
npm install -g @anthropic-ai/claude-code

# Autenticação prévia (execute uma vez no terminal):
claude
```
Siga as instruções exibidas na tela para autenticar com sua conta Anthropic. Certifique-se de que o comando `claude` responda normalmente no PowerShell.

### 2. Antigravity CLI (`agy`)
```powershell
# Instalação oficial via PowerShell:
irm https://antigravity.google/cli/install.ps1 | iex

# Autenticação prévia:
agy login
```
Após a instalação, confirme que o executável está disponível no `PATH` executando `agy --version`.

---

## 📦 Instalação do CommandDeck Terminal

Clone o repositório ou navegue até a pasta raiz do projeto:

```powershell
cd C:\Users\<seu-usuario>\jwic

# Instalar dependências do Node / Electron:
npm install

# Iniciar o aplicativo:
npm start
```

### Criando o Atalho na Área de Trabalho
O arquivo `JWIC.lnk` (ou `CommandDeck.lnk`) presente na raiz do repositório pode ser copiado para a Área de Trabalho ou para o Menu Iniciar do Windows. Ele executa diretamente o binário `node_modules\electron\dist\electron.exe .` em segundo plano, sem manter uma janela de console do Windows aberta.

---

## 🎙️ Configuração do Subsistema Whisper (Voz Local)

O CommandDeck integra transcrição de fala para texto 100% local através do `whisper.cpp` com o modelo `ggml-large-v3-turbo` (1,6 GB).

### Downloads Automáticos em Segundo Plano
Ao iniciar a aplicação pela primeira vez, o CommandDeck verifica a pasta `whisper/` e baixa automaticamente:
1. **Modelo neural:** `whisper/models/ggml-large-v3-turbo.bin` (via Hugging Face).
2. **Engine CPU:** `whisper-bin-x64.zip` (via GitHub Releases oficial do whisper.cpp).
3. **Engine CUDA:** `whisper-bin-win-cuda-12.4.0-x64.zip` (caso uma GPU NVIDIA seja detectada).

O status dos downloads é transmitido em tempo real para a área "Conversa integrada" na interface do usuário.

### Compilação da Engine Vulkan (Para GPUs AMD e Intel)
Como o projeto `whisper.cpp` não publica binários oficiais pré-compilados com Vulkan para Windows, o CommandDeck inclui o código-fonte em `whisper/src/` para compilação local:

1. **Instale o Visual Studio Build Tools:** Instale a carga de trabalho de desenvolvimento para C++ (MSVC compiler, CMake).
2. **Instale o Vulkan SDK:** Baixe e instale a versão mais recente em [lunarg.com/vulkan-sdk](https://www.lunarg.com/vulkan-sdk/).
3. **Compile a DLL e executável:**
   ```powershell
   cd whisper\src\whisper.cpp
   cmake -B build -DWHISPER_VULKAN=ON
   cmake --build build --config Release
   ```
4. Os binários gerados (`whisper-cli.exe`, `whisper.dll`, `ggml-vulkan.dll`) devem ser movidos para a pasta `whisper\bin-vulkan\`.

---

## 🔧 Solução de Problemas Frequentes (Troubleshooting)

### 1. "Antigravity CLI (agy) não encontrado"
- **Causa:** O binário `agy.exe` não está no `PATH` ou nos diretórios padrão (`%LOCALAPPDATA%\agy\bin`, `%LOCALAPPDATA%\Programs\agy`).
- **Solução:** Reabra o terminal com privilégios adequados e rode `irm https://antigravity.google/cli/install.ps1 | iex`. Certifique-se de que a variável de ambiente `PATH` inclua o diretório de instalação do Antigravity.

### 2. "Claude Code CLI (claude) não encontrado"
- **Causa:** O pacote `@anthropic-ai/claude-code` não foi instalado globalmente ou o binário não está no `PATH`.
- **Solução:** Execute no terminal `npm install -g @anthropic-ai/claude-code`.

### 3. "You've hit your weekly limit" no Claude
- **Causa:** A conta vinculada ao Claude Code atingiu a quota semanal da Anthropic.
- **Sintoma:** Ao invocar `ask-agent claude "mensagem"`, a resposta retorna com erro 1 contendo o horário de reset da quota.
- **Solução:** Aguarde o horário indicado para renovação da quota ou continue trabalhando utilizando as abas do Antigravity CLI e terminal Shell, que operam de forma independente.

### 4. Falha ou congelamento na gravação de áudio
- **Causa:** Permissão de microfone bloqueada no Windows.
  - *Resolução:* Acesse **Configurações do Windows > Privacidade e Segurança > Microfone** e verifique se o acesso a aplicativos da área de trabalho está ativado.

### 5. `ask-agent` retorna "rode este comando de dentro de um terminal do CommandDeck"
- **Causa:** O script `ask-agent` foi executado a partir de um prompt externo do Windows (fora da janela do CommandDeck).
- **Explicação:** O utilitário depende das variáveis de ambiente injetadas em tempo de execução pelo processo pai (`JWIC_PORT`, `JWIC_TOKEN`, `JWIC_AGENT`). Ele só opera dentro das abas do CommandDeck.

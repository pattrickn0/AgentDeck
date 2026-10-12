# 🤝 Guia de Contribuição (Contributing to CommandDeck Terminal)

Agradecemos o seu interesse em contribuir para o **CommandDeck Terminal**! Este projeto é construído para desenvolvedores que desejam levar a orquestração e colaboração autônoma de agentes de IA ao próximo nível.

---

## 💻 Ambiente de Desenvolvimento

### Pré-requisitos
- **Sistema Operacional:** Windows 10 ou 11 (64-bit).
- **Node.js:** Versão 18.x, 20.x ou superior.
- **PowerShell:** 5.1 ou PowerShell Core (7+).
- **Git** instalado e configurado.
- Os CLIs oficiais autenticados:
  - Claude Code CLI (`npm install -g @anthropic-ai/claude-code && claude`)
  - Antigravity CLI (`irm https://antigravity.google/cli/install.ps1 | iex && agy login`)

### Configurando o Repositório
```powershell
# 1. Clone o repositório
git clone https://github.com/pattrickn0/CommandDeck-Terminal.git
cd CommandDeck-Terminal

# 2. Instale as dependências
npm install

# 3. Inicie o aplicativo em modo de desenvolvimento
npm start
```

---

## 📐 Diretrizes de Arquitetura e Código

Ao submeter contribuições, siga os princípios estabelecidos do projeto:

1. **Privacidade e Descarte Imediato:**
   - Quaisquer arquivos de áudio temporários gerados para o motor Whisper DEVEM ser descartados imediatamente (`fs.unlinkSync` / `promises.unlink`) logo após a extração textual.
   - Retenção estrita de no máximo 5 sessões para anexos e prompts longos.

2. **Segurança e Isolamento no Electron:**
   - Mantenha `nodeIntegration: false` e `contextIsolation: true` na janela principal.
   - Toda comunicação entre Renderer e Main deve trafegar estritamente pelos canais IPC declarados em `preload.js`.
   - O servidor HTTP bridge em `main.js` deve validar rigorosamente o cabeçalho `x-jwic-token` e rejeitar requisições externas à interface de loopback (`127.0.0.1`).

3. **Compatibilidade Terminal (ConPTY / xterm.js):**
   - Garanta que sequências ANSI e redimensionamentos (resize throttle de 60ms) permaneçam fluidos.

---

## 🔀 Fluxo de Trabalho Git

1. Crie uma branch para a sua funcionalidade ou correção:
   ```bash
   git checkout -b feat/minha-melhoria
   ```
2. Escreva commits claros seguindo o padrão [Conventional Commits](https://www.conventionalcommits.org/):
   - `feat: adiciona suporte a novos modelos whisper`
   - `fix: corrige debounce no clique do explorador de arquivos`
   - `docs: atualiza guia de instalação do Vulkan SDK`
   - `refactor: modulariza controle de abas do renderer`
3. Teste a aplicação e certifique-se de que não haja regressões de interface ou de comunicação inter-agente.
4. Abra um Pull Request detalhando as motivações e alterações realizadas.

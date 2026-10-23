# 📁 Diretório de Dados de Execução (`dados/`)

Este diretório é utilizado internamente pelo **AgentDeck** para armazenamento temporário e de tempo de execução durante as sessões de uso.

---

## 🔒 Privacidade e Política de Retenção

Por motivos de privacidade e conformidade técnica, o conteúdo desta pasta é **ignorado pelo controle de versão (`.gitignore`)**:

1. **Gravações de Áudio (`.wav`):**
   - Áudios capturados pelo microfone são gravados temporariamente para processamento pelo motor Whisper.
   - **Descarte Imediato:** Assim que a transcrição neural é concluída, o arquivo `.wav` é **imediatamente excluído** do disco. Retenção zero de voz.

2. **Anexos e Capturas Multimodais (`anexos/`):**
   - Imagens coladas (`Ctrl+V`), arquivos arrastados sobre uma janela ou anexados pelo botão do cabeçalho da janela (e pelo formulário "Nova tarefa") para contextualizar os agentes.
   - **Retenção de 5 Sessões:** O AgentDeck mantém apenas os anexos das 5 sessões mais recentes. Arquivos de sessões antigas são expurgados automaticamente na inicialização.

3. **Prompts Longos (`prompts/`):**
   - Prompts extensos enviados ao Antigravity CLI que ultrapassam o limite seguro da linha de comando do Windows são gravados em arquivos de texto temporários e passados via referência de diretório.

4. **Histórico de Sessões (`sessions.json`):**
   - Registro local dos identificadores alfanuméricos das últimas sessões ativas para controle de ciclo de vida e purga de arquivos.

5. **Tarefas (`tarefas/`):**
   - A mesa de tarefas guarda um `<projeto>.json` por projeto (as tarefas, os relatórios e o histórico) e, para cada tarefa que abre uma aba nova, o pedido (`<projeto>-<id>-brief.md`) e as devoluções (`-feedback.md`) que o agente lê por arquivo.
   - **Ficam fora do repositório do usuário** e não são apagados ao remover o projeto.

6. **Hooks do Claude (`hooks/`), `crash.log` e `.agentdeck.lock`:**
   - Gerados pelo app: a configuração dos hooks de início/fim de turno, o registro das quedas (`crash.log`, que gira acima de 256 KB), o log do app (`agentdeck.log`, que gira acima de 2 MB) e a trava que impede uma segunda janela na mesma pasta.

# 📁 Diretório de Dados de Execução (`dados/`)

Este diretório é utilizado internamente pelo **CommandDeck Terminal** para armazenamento temporário e de tempo de execução durante as sessões de uso.

---

## 🔒 Privacidade e Política de Retenção

Por motivos de privacidade e conformidade técnica, o conteúdo desta pasta é **ignorado pelo controle de versão (`.gitignore`)**:

1. **Gravações de Áudio (`.wav`):**
   - Áudios capturados pelo microfone são sintetizados temporariamente para processamento pelo motor Whisper.
   - **Descarte Imediato:** Assim que a transcrição neural é concluída, o arquivo `.wav` é **imediatamente excluído** do disco (`unlinkSync`). Retenção zero de voz.

2. **Anexos e Capturas Multimodais (`anexos/`):**
   - Imagens coladas (`Ctrl+V`), arrastadas ou anexadas para contextualizar os agentes Claude e Antigravity.
   - **Retenção de 5 Sessões:** O CommandDeck mantém apenas os anexos das 5 sessões mais recentes. Arquivos de sessões antigas são expurgados automaticamente na inicialização.

3. **Prompts Longos (`prompts/`):**
   - Prompts extensos enviados ao Antigravity CLI que ultrapassam o limite seguro da linha de comando do Windows são gravados em arquivos de texto temporários e passados via referência de diretório.

4. **Histórico de Sessões (`sessions.json`):**
   - Registro local dos identificadores alfanuméricos das últimas sessões ativas para controle de ciclo de vida e purga de arquivos.

# 🧠 Modelos Neurais Whisper (`whisper/models/`)

Este diretório armazena os pesos pré-treinados dos modelos neurais Whisper no formato binário GGML.

---

## 📦 Modelo Padrão: `ggml-large-v3-turbo.bin`

- **Tamanho:** ~1,62 GB (1.624.555.275 bytes)
- **Origem:** OpenAI Whisper large-v3-turbo / Repositório GGML Hugging Face
- **Controle de Versão:** Por ultrapassar os limites de arquivos do Git e GitHub (100 MB), os arquivos `.bin` são **ignorados no repositório (`.gitignore`)**.

---

## ⚡ Como Obter o Modelo

### 1. Download Automático (Recomendado)
Você não precisa baixar o modelo manualmente! Ao iniciar o CommandDeck Terminal pela primeira vez (`npm start`), a aplicação verifica a existência de `whisper/models/ggml-large-v3-turbo.bin`. Se não o encontrar, ela inicia o download em segundo plano automaticamente exibindo o progresso em tempo real na interface gráfica.

### 2. Download Manual (Ambientes Offline / Conexões Lentas)
Caso prefira baixar previamente:

#### Via PowerShell:
```powershell
Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin" -OutFile "whisper\models\ggml-large-v3-turbo.bin"
```

#### Via Curl:
```bash
curl -L -o whisper/models/ggml-large-v3-turbo.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

Após o download, certifique-se de que o arquivo esteja nomeado exatamente como `ggml-large-v3-turbo.bin` dentro da pasta `whisper/models/`.

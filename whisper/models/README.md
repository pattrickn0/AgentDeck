# 🧠 Whisper Neural Models (`whisper/models/`)

This directory stores the pre-trained weights for Whisper neural models in GGML binary format.

---

## 📦 Default Model: `ggml-large-v3-turbo.bin`

- **Size:** ~1.62 GB (1,624,555,275 bytes)
- **Origin:** OpenAI Whisper large-v3-turbo / Hugging Face GGML Repository
- **Version Control:** Because they exceed Git and GitHub file limits (100 MB), `.bin` files are **ignored in the repository (`.gitignore`)**.

---

## ⚡ How to Obtain the Model

### 1. Automatic Download (Recommended)
You do not need to download the model manually! When launching AgentDeck for the first time (`agentdeck.exe`), the application checks for the presence of `whisper/models/ggml-large-v3-turbo.bin`. If not found, it automatically starts a background download, displaying real-time progress in the graphical interface.

### 2. Manual Download (Offline Environments / Slow Connections)
If you prefer to download in advance:

#### Via PowerShell:
```powershell
Invoke-WebRequest -Uri "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin" -OutFile "whisper\models\ggml-large-v3-turbo.bin"
```

#### Via Curl:
```bash
curl -L -o whisper/models/ggml-large-v3-turbo.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-large-v3-turbo.bin
```

After downloading, ensure the file is named exactly `ggml-large-v3-turbo.bin` inside the `whisper/models/` folder.

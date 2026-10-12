# 🎙️ Subsistema de Transcrição de Voz Local (`whisper/`)

O diretório `whisper/` gerencia todo o motor de inferência de voz para texto (Speech-to-Text) do **CommandDeck Terminal**, baseado no projeto de alto desempenho **`whisper.cpp`** da GGML.

---

## 🌟 Pilares do Subsistema

1. **Privacidade Absoluta (Zero Cloud):** Nenhuma gravação de áudio ou metadado vocal trafega pela internet. Toda inferência neural ocorre estritamente no seu computador.
2. **Descarte Imediato de Áudio:** No milissegundo em que o texto é transcrito e inserido no campo de composição, o arquivo de áudio `.wav` temporário é **imediatamente excluído** do disco.
3. **Aceleração por Hardware Dinâmica:** Detecta o hardware do sistema e seleciona a engine mais veloz disponível:
   - 🟢 **NVIDIA CUDA:** Detecta placas GeForce/RTX via `nvidia-smi` e utiliza CUDA 12.4 (`bin-cuda/`).
   - 🔴/🔵 **Vulkan API:** Suporte nativo multiplataforma para GPUs **AMD Radeon**, **Intel Arc / Iris Xe** (`bin-vulkan/`).
   - ⚪ **CPU AVX2:** Fallback universal com multithreading OpenMP (`bin-cpu/`).

---

## 📂 Estrutura de Pastas

```
whisper/
├── README.md               # Este documento de referência
├── bin-vulkan/             # Binários compilados para aceleração Vulkan (AMD/Intel)
│   ├── whisper-cli.exe     # Executável de inferência
│   ├── whisper.dll         # Biblioteca Whisper
│   └── ggml-vulkan.dll     # Backend Vulkan da biblioteca GGML
├── bin-cpu/                # Binários oficiais de fallback para CPU (AVX2/OpenMP)
│   └── Release/
│       └── whisper-cli.exe
├── bin-cuda/               # Baixado dinamicamente caso GPU NVIDIA seja detectada
└── models/                 # Diretório de modelos neurais GGML
    ├── README.md           # Instruções e links de download do modelo
    └── .gitkeep            # ggml-large-v3-turbo.bin (~1.6 GB, ignorado no git)
```

---

## 🧠 Modelo Neural (`large-v3-turbo`)

O CommandDeck utiliza o modelo oficial **`large-v3-turbo`** (~1,62 GB) da OpenAI convertido para o formato GGML:
- **Precisão Excepcional:** Suporte nativo a dezenas de idiomas com foco especial em Português e Inglês técnico.
- **Download Automático:** Na primeira inicialização, se o modelo não existir na pasta `whisper/models/`, o CommandDeck baixa-o automaticamente direto do Hugging Face com exibição do progresso no terminal da interface.
- **Injeção de Contexto Fonético:** Cada inferência inclui um prompt contextual customizado (`--prompt "CommandDeck Terminal, Claude, Antigravity, agy, Vulkan..."`) para garantir grafia correta dos termos de desenvolvimento.

---

## 🔨 Compilação Manual do Backend Vulkan (Opcional)

Os binários em `bin-vulkan/` já vêm prontos para uso em sistemas Windows x64 com suporte a Vulkan. Caso queira recompilar ou atualizar a versão do `whisper.cpp`, fornecemos um script automatizado:

```powershell
# Pré-requisito: Vulkan SDK e CMake instalados
powershell -ExecutionPolicy Bypass -File scripts/build-vulkan.ps1
```

# 🎙️ Local Voice Transcription Subsystem (`whisper/`)

The `whisper/` directory manages the entire Speech-to-Text voice inference engine of **AgentDeck**, based on the high-performance **`whisper.cpp`** project from GGML.

---

## 🌟 Subsystem Pillars

1. **Absolute Privacy (Zero Cloud):** No audio recording or voice metadata travels over the internet. All neural inference occurs strictly on your computer.
2. **Immediate Audio Discard:** The millisecond text is transcribed and inserted into the composer field, the temporary `.wav` audio file is **immediately deleted** from disk. Zero voice retention.
3. **Dynamic Hardware Acceleration:** Detects system hardware and selects the fastest available engine:
   - 🟢 **NVIDIA CUDA:** Detects GeForce/RTX cards via `nvidia-smi` and uses CUDA 12.4 (`bin-cuda/`).
   - 🔴/🔵 **Vulkan API:** Native cross-platform support for **AMD Radeon**, **Intel Arc / Iris Xe** GPUs (`bin-vulkan/`).
   - ⚪ **CPU AVX2:** Universal fallback with OpenMP multithreading (`bin-cpu/`).

---

## 📂 Folder Structure

```
whisper/
├── README.md               # This reference document
├── bin-vulkan/             # Binaries compiled for Vulkan acceleration (AMD/Intel)
│   ├── whisper-cli.exe     # Inference executable
│   ├── whisper.dll         # Whisper library
│   └── ggml-vulkan.dll     # Vulkan backend of the GGML library
├── bin-cpu/                # Official CPU fallback binaries (AVX2/OpenMP)
│   └── Release/
│       └── whisper-cli.exe
├── bin-cuda/               # Downloaded dynamically if an NVIDIA GPU is detected
└── models/                 # GGML neural models directory
    ├── README.md           # Model download instructions and links
    └── .gitkeep            # ggml-large-v3-turbo.bin (~1.6 GB, git-ignored)
```

---

## 🧠 Neural Model (`large-v3-turbo`)

AgentDeck uses the official **`large-v3-turbo`** model (~1.62 GB) from OpenAI converted to GGML format:
- **Exceptional Accuracy:** Native support for dozens of languages with a special focus on Portuguese and technical English.
- **Optional download:** Voice dictation is optional. On first launch, AgentDeck asks if you want to use it; only with confirmation does it download the model (directly from Hugging Face, with progress in the right bar) and the CPU engine. Users who chose "Not now" can download it later (or remove what was downloaded) in Settings.
- **Phonetic Context Injection:** Each inference includes a customized contextual prompt (`--prompt "AgentDeck, Claude, Antigravity, agy, Vulkan..."`) to ensure correct spelling of development terms.

---

## 🔨 Manual Compilation of the Vulkan Backend (Optional)

The binaries in `bin-vulkan/` come ready to use on Windows x64 systems with Vulkan support. If you wish to recompile or update the `whisper.cpp` version, we provide an automated script:

```powershell
# Prerequisite: Vulkan SDK and CMake installed
powershell -ExecutionPolicy Bypass -File scripts/build-vulkan.ps1
```

---

## Third-party licenses

The binaries versioned in `bin-cpu/` and `bin-vulkan/` are not AgentDeck code (MIT):

- whisper.cpp and ggml (whisper-cli.exe, whisper.dll, ggml*.dll): MIT, © the GGML project authors (https://github.com/ggml-org/whisper.cpp).
- SDL2.dll: zlib (https://libsdl.org).
- msvcp140.dll, vcruntime140*.dll, vcomp140.dll: Microsoft Visual C++ redistributable runtime.
- The large-v3-turbo model (downloaded, never versioned): MIT, OpenAI.

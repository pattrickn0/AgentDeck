---
title: The Linux Vulkan whisper-cli is built by our own workflow, statically, on ubuntu-22.04
summary: whisper-engines.yml builds whisper.cpp with Vulkan (glibc 2.35, no bundled .so) and releases it; the zip's SHA-256 is pinned by hand in whisper.rs.
touches: [crates/core/src/whisper.rs, .github/workflows/whisper-engines.yml, memory/LINUX.md]
---
upstream whisper.cpp releases ship no Linux GPU build, so `.github/workflows/whisper-engines.yml` builds `whisper-cli`
with `-DGGML_VULKAN=1` and attaches `whisper-bin-vulkan-linux-x64.zip` (+ `.sha256`) to the release
`whisper-engines-<whisper.cpp tag>` of `pattrickn0/Holt`. The app (`engine_archive`, `ENGINES_RELEASE`,
`VULKAN_LINUX_SHA256`) downloads it into `whisper/bin-vulkan-linux/` when there is a GPU **and** a Vulkan driver manifest.

**Why (the non-obvious choices):**
- **glibc:** a binary needs at least the glibc it was linked against. The job runs on `ubuntu-22.04` (glibc 2.35), not
  `ubuntu-latest`, and the "Check portability" step fails if `objdump -T` asks for a symbol newer than `GLIBC_2.35`.
- **Static instead of bundled `.so`:** `-DBUILD_SHARED_LIBS=OFF`, `-static-libstdc++ -static-libgcc`, `-DGGML_OPENMP=OFF`,
  `-DGGML_NATIVE=OFF` (no `-march=native`, which would crash on other CPUs; AVX2 stays on). The zip is `whisper-cli` + `LICENSE`.
  The only run-time dependencies are glibc and `libvulkan.so.1`; if the loader is missing the process fails to start and
  `transcribe_pcm16k` falls back to the CPU engine with a notice. `libvulkan` is never needed to build Holt.
- **Vulkan SDK from LunarG**, pinned (`VULKAN_SDK_VERSION`): Ubuntu 22.04's packaged headers/`glslc` are too old for ggml-vulkan.
- **llvmpipe does not count as a driver** (`lvp_icd*.json`): Vulkan on the software rasteriser is slower than the CPU engine,
  and VMs/containers have it without a GPU. `has_vulkan_icd` skips it; `lspci` must also list a GPU.
- **The hash is pinned by hand:** after a new build copy the SHA-256 from the "Package" step (or the `.sha256` asset) into
  `VULKAN_LINUX_SHA256` and bump `ENGINES_RELEASE` if the whisper.cpp tag changed. Until then it is all zeros and the download
  fails its checksum (dictation stays on CPU).

**How to apply:** to change the whisper.cpp version or flags, edit the workflow, dispatch it
(`gh workflow run whisper-engines.yml -f whisper_version=vX.Y.Z`, it must be on the default branch) or push a
`whisper-engines-vX.Y.Z` tag, then update the two constants. The workflow only builds x86_64; aarch64 and CUDA on Linux
still mean a self-compiled `whisper-cli` on PATH (which counts as the CPU engine).

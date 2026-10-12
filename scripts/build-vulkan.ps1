<#
.SYNOPSIS
    Compila o whisper.cpp com suporte a aceleração Vulkan para Windows (GPUs AMD Radeon e Intel).
.DESCRIPTION
    Clona o repositório whisper.cpp (se necessário), compila via CMake + MSVC com a flag
    -DWHISPER_VULKAN=ON e copia os binários gerados para whisper/bin-vulkan/.
#>

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$whisperDir = Join-Path $projectRoot "whisper"
$srcDir = Join-Path $whisperDir "src\whisper.cpp"
$outDir = Join-Path $whisperDir "bin-vulkan"

Write-Host "=== Compilador Vulkan do whisper.cpp para CommandDeck ===" -ForegroundColor Cyan

# Verifica pré-requisitos
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    Write-Error "CMake não encontrado no PATH. Instale o CMake ou o Visual Studio Build Tools com C++."
    exit 1
}

if (-not $env:VULKAN_SDK -and -not (Test-Path "C:\VulkanSDK")) {
    Write-Warning "VULKAN_SDK não detectado nas variáveis de ambiente. Certifique-se de ter o LunarG Vulkan SDK instalado."
}

# Clona ou atualiza o whisper.cpp
if (-not (Test-Path $srcDir)) {
    Write-Host "Clonando whisper.cpp em $srcDir..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Path (Split-Path -Parent $srcDir) -Force | Out-Null
    git clone --depth 1 https://github.com/ggml-org/whisper.cpp $srcDir
}

# Configura e compila
$buildDir = Join-Path $srcDir "build"
Write-Host "Configurando CMake com suporte a Vulkan..." -ForegroundColor Yellow
cmake -S $srcDir -B $buildDir -DWHISPER_VULKAN=ON

Write-Host "Compilando whisper-cli em Release..." -ForegroundColor Yellow
cmake --build $buildDir --config Release --target whisper-cli

# Garante pasta de destino
if (-not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
}

Write-Host "Copiando binários compilados para $outDir..." -ForegroundColor Yellow
$builtFiles = Get-ChildItem -Recurse -Path $buildDir -Include "*.dll", "*.exe" | Where-Object { $_.FullName -notmatch "CMakeFiles" }
foreach ($file in $builtFiles) {
    Copy-Item -Path $file.FullName -Destination $outDir -Force
}

Write-Host "Compilação concluída com sucesso! Binários instalados em $outDir" -ForegroundColor Green

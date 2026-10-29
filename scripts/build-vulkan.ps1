<#
.SYNOPSIS
    Builds whisper.cpp with Vulkan acceleration for Windows (AMD Radeon and Intel GPUs).
.DESCRIPTION
    Clones the whisper.cpp repository (if needed), builds it with CMake + MSVC with the flag
    -DWHISPER_VULKAN=ON and copies the built binaries to whisper/bin-vulkan/.
#>

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$whisperDir = Join-Path $projectRoot "whisper"
$srcDir = Join-Path $whisperDir "src\whisper.cpp"
$outDir = Join-Path $whisperDir "bin-vulkan"

Write-Host "=== whisper.cpp Vulkan build for Holt ===" -ForegroundColor Cyan

# Checks the prerequisites
if (-not (Get-Command cmake -ErrorAction SilentlyContinue)) {
    Write-Error "CMake not found on PATH. Install CMake or the Visual Studio Build Tools with C++."
    exit 1
}

if (-not $env:VULKAN_SDK -and -not (Test-Path "C:\VulkanSDK")) {
    Write-Warning "VULKAN_SDK not found in the environment variables. Make sure the LunarG Vulkan SDK is installed."
}

# Clones or updates whisper.cpp
if (-not (Test-Path $srcDir)) {
    Write-Host "Clonando whisper.cpp em $srcDir..." -ForegroundColor Yellow
    New-Item -ItemType Directory -Path (Split-Path -Parent $srcDir) -Force | Out-Null
    git clone --depth 1 https://github.com/ggml-org/whisper.cpp $srcDir
}

# Configures and builds
$buildDir = Join-Path $srcDir "build"
Write-Host "Configuring CMake with Vulkan support..." -ForegroundColor Yellow
cmake -S $srcDir -B $buildDir -DWHISPER_VULKAN=ON

Write-Host "Compilando whisper-cli em Release..." -ForegroundColor Yellow
cmake --build $buildDir --config Release --target whisper-cli

# Makes sure the destination folder exists
if (-not (Test-Path $outDir)) {
    New-Item -ItemType Directory -Path $outDir -Force | Out-Null
}

Write-Host "Copying the built binaries to $outDir..." -ForegroundColor Yellow
$builtFiles = Get-ChildItem -Recurse -Path $buildDir -Include "*.dll", "*.exe" | Where-Object { $_.FullName -notmatch "CMakeFiles" }
foreach ($file in $builtFiles) {
    Copy-Item -Path $file.FullName -Destination $outDir -Force
}

Write-Host "Build finished! Binaries installed in $outDir" -ForegroundColor Green

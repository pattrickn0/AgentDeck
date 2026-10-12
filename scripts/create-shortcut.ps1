<#
.SYNOPSIS
    Cria atalhos do CommandDeck Terminal no diretório raiz do projeto e na Área de Trabalho.
.DESCRIPTION
    Gera atalhos do Windows (.lnk) apontando para o binário do Electron em segundo plano,
    permitindo abrir a aplicação com um clique e sem deixar prompts de comando visíveis.
#>

$ErrorActionPreference = "Stop"

$projectRoot = Split-Path -Parent $PSScriptRoot
$electronExe = Join-Path $projectRoot "node_modules\electron\dist\electron.exe"

if (-not (Test-Path $electronExe)) {
    Write-Warning "Executável do Electron não foi encontrado em: $electronExe"
    Write-Warning "Por favor, execute 'npm install' antes de criar os atalhos."
    exit 1
}

$wscript = New-Object -ComObject WScript.Shell

# 1. Atalho na raiz do repositório
$localShortcut = Join-Path $projectRoot "CommandDeck.lnk"
$shortcut = $wscript.CreateShortcut($localShortcut)
$shortcut.TargetPath = $electronExe
$shortcut.Arguments = "`"$projectRoot`""
$shortcut.WorkingDirectory = $projectRoot
$shortcut.Description = "CommandDeck Terminal - Terminal Colaborativo para Claude Code e Antigravity"
$shortcut.Save()

Write-Host "Atalho local criado com sucesso em: $localShortcut" -ForegroundColor Green

# 2. Atalho na Área de Trabalho (Desktop)
$desktop = [Environment]::GetFolderPath("Desktop")
if ($desktop -and (Test-Path $desktop)) {
    $desktopShortcut = Join-Path $desktop "CommandDeck.lnk"
    $dShortcut = $wscript.CreateShortcut($desktopShortcut)
    $dShortcut.TargetPath = $electronExe
    $dShortcut.Arguments = "`"$projectRoot`""
    $dShortcut.WorkingDirectory = $projectRoot
    $dShortcut.Description = "CommandDeck Terminal - Terminal Colaborativo para Claude Code e Antigravity"
    $dShortcut.Save()
    Write-Host "Atalho na Área de Trabalho criado em: $desktopShortcut" -ForegroundColor Green
}

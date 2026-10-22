# Gera crates/app/assets/agentdeck.ico (16, 24, 32, 48 e 256 px) a partir do mesmo desenho do logo
# do app (LOGO_CARDS em crates/app/src/icons.rs). Fundo transparente; contorno e furo em #16181c.
#
#   powershell -File scripts/gerar-icone.ps1
#
# O teste `the_committed_ico_has_the_five_sizes` falha se o .ico versionado ficar diferente do desenho.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
cargo test -p agentdeck --locked --bin agentdeck write_the_ico_file -- --ignored
if ($LASTEXITCODE -ne 0) { throw 'cargo test falhou' }
Get-Item crates/app/assets/agentdeck.ico | Format-List FullName, Length

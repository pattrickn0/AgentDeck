# Generates crates/app/assets/agentdeck.ico (16, 24, 32, 48 and 256 px) from the same drawing as the app's
# logo (LOGO_CARDS in crates/app/src/icons.rs). Transparent background; outline and hole in #16181c.
#
#   powershell -File scripts/generate-icon.ps1
#
# The `the_committed_ico_has_the_five_sizes` test fails if the committed .ico differs from the drawing.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
cargo test -p agentdeck --locked --bin agentdeck write_the_ico_file -- --ignored
if ($LASTEXITCODE -ne 0) { throw 'cargo test falhou' }
Get-Item crates/app/assets/agentdeck.ico | Format-List FullName, Length

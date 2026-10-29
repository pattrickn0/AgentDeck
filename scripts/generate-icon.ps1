# Generates crates/app/assets/agentdeck.ico (16, 24, 32, 48 and 256 px) and agentdeck.png (256 px, the Linux icon)
# from the same drawing as the app's logo (LOGO_CARDS in crates/app/src/icons.rs). Transparent background; outline and hole in #16181c.
#
#   powershell -File scripts/generate-icon.ps1
#
# The `the_committed_ico_has_the_five_sizes` and `the_committed_png_is_the_largest_ico_image` tests fail if the
# committed files differ from the drawing.
$ErrorActionPreference = 'Stop'
Set-Location (Join-Path $PSScriptRoot '..')
cargo test -p agentdeck --locked --bin agentdeck write_the_ico_file -- --ignored
if ($LASTEXITCODE -ne 0) { throw 'cargo test falhou' }
Get-Item crates/app/assets/agentdeck.ico, crates/app/assets/agentdeck.png | Format-List FullName, Length

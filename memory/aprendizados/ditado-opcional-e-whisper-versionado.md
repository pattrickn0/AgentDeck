---
titulo: O ditado é opcional e "instalado" é modelo + motor; bin-cpu e bin-vulkan vêm no git
resumo: Config::dictation (unasked/yes/no) decide se baixa; quem já tem o modelo conta como "sim"; remover apaga só o modelo, o CUDA e sobras.
touches: [crates/core/src/whisper.rs, crates/core/src/config.rs, crates/app/src/deck.rs, crates/app/src/ui/menus.rs]
---
O Whisper só baixa se `Config::dictation` for `yes` (ou se já está no disco): `whisper::startup(choice, installed)` devolve
`Ensure`, `Ask` ou `Skip` e a escolha a gravar. "Instalado" (`whisper::is_installed`) é o **modelo** em `whisper/models/`
mais algum `whisper-cli` (motor); só o motor não basta, porque `bin-cpu/` e `bin-vulkan/` do Windows vêm **versionados no
git** e já existem num checkout sem o modelo.

**Por quê:** o app baixava 1,6 GB sozinho na primeira abertura. Quem atualiza (modelo já baixado, config sem o campo) não
pode ser perguntado: vira `yes` calado. `AGENTDECK_SMOKE` nunca chama `Deck::dictation_startup`. Um instalador futuro pode
escrever `"dictation": "yes"|"no"` no `config.json` antes da primeira abertura.
**Como aplicar:** `Core::remove_dictation` (☰ Configurações) apaga o modelo, o `.part`, `bin-cuda/`, os pacotes `.zip` e, fora do
Windows, os motores baixados; **não** apaga `bin-cpu/` e `bin-vulkan/` no Windows (versionados; apagá-los sujaria o git).
Ela recusa enquanto `ensuring` está ligado e grava `no`. A interface descobre o fim do download/remoção por
`CoreEvent::DictationChanged` (não por polling da flag, que perderia a corrida com o último evento).

---
title: Dictation is optional and "installed" means model + engine; bin-cpu and bin-vulkan are tracked in git
summary: Config::dictation (unasked/yes/no) controls whether to download; having the model already counts as "yes"; removal deletes only the model, CUDA, and leftovers.
touches: [crates/core/src/whisper.rs, crates/core/src/config.rs, crates/app/src/deck/**, crates/app/src/ui/menus.rs]
---
Whisper is downloaded only if `Config::dictation` is `yes` (or if it is already on disk): `whisper::startup(choice, installed)` returns
`Ensure`, `Ask`, or `Skip`, plus the choice to save. "Installed" (`whisper::is_installed`) means the **model** in `whisper/models/`
plus a `whisper-cli` (engine); the engine alone is not sufficient because Windows `bin-cpu/` and `bin-vulkan/` are **tracked in
git** and already exist in a checkout that lacks the model.

**Why:** the app used to download 1.6 GB on its own upon first launch. Users upgrading (model already downloaded, config lacks the field)
should not be prompted: it silently becomes `yes`. `HOLT_SMOKE` never calls `Deck::dictation_startup`. A future installer can
write `"dictation": "yes"|"no"` to `config.json` before first startup.
**How to apply:** `Core::remove_dictation` (☰ Settings) removes the model, `.part`, `bin-cuda/`, `.zip` packages, and, outside
Windows, downloaded engines; it does **not** delete `bin-cpu/` and `bin-vulkan/` on Windows (versioned; deleting them would dirty git).
Removal is rejected while `ensuring` is active and persists `no`. The UI learns of download/removal completion via
`CoreEvent::DictationChanged` (not by polling the flag, which could lose the race with the final event).

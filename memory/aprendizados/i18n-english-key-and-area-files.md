---
title: i18n uses the English text as the key, one pt-BR file per area, agents always English
summary: t!("English") with a pt-BR pair in i18n/pt_br/<area>.rs; agent-facing text never goes through t!; a source scan test keeps keys and entries in sync.
touches: [crates/core/src/i18n/**, crates/app/src/**, crates/core/src/bridge.rs, crates/core/src/headless.rs]
---
The app is English by default; `config.json` `"language": "pt-BR"` (Settings ☰ > Language) switches to Portuguese
at once. `agentdeck_core::t!` (import it with `use agentdeck_core::t;`):

- `t!("Close")` is `&'static str`; with placeholders it is `String`: `t!("Model: {model}", model = m)`, or the
  shorthand `t!("Download ({size})", size)`. A missing translation falls back to English.
- The pair `("English", "Português")` goes in the area file `crates/core/src/i18n/pt_br/<area>.rs` (the list and what
  each covers is in `pt_br/mod.rs` and each file's header). Text shared by areas (Close, Cancel, Save, Back, Default,
  Rename) is in `common.rs`. The same key in two area files is allowed only with the same translation.
- **Agent-facing text never goes through `t!`** (ask-agent output, AgentDeck instructions, briefs, feedback, bridge
  responses): it stays English. The bridge request threads (`cd-bridge-req`, `cd-bridge-late`) and
  `Core::run_headless` run under `i18n::english(...)`, so a core function shared with the UI answers agents in English
  even when the user chose Portuguese. A new thread that answers agents needs the same wrapper.
- `cargo test -p agentdeck-core i18n` runs the checks: no key with two different translations, the same `{placeholders}` in both
  languages, and a scan of `crates/*/src/**/*.rs` that takes the literal after every `t!(` and fails on a key with no
  pt-BR entry or an entry no `t!` uses. Comment lines (`//`) are skipped; the key must be a plain `"..."` literal
  (the scan rejects `t!(CONST)` and raw strings).

**Why:** decided by the user on 2026-10-25 (app in English, Portuguese optional, agents English only). The English key
and one file per area let several tasks convert areas in parallel without touching the same files.
**How to apply:** tests that assert UI text must not depend on the process-wide language (other tests change it): wrap
them in `i18n::english(|| ...)` or `i18n::with_lang(Lang::PtBr, || ...)`, which are per thread. Language names in the
selector stay in their own language ("English", "Português") and are not translated.

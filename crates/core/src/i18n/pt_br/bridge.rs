//! pt-BR translations for the area: Bridge, exchange, delivery, inbox, instances, team and ask-agent (mostly agent-facing: only what the UI shows goes through t!).
//!
//! Entries are `(English, Portuguese)`; the English text is the key used in `t!("...")`. See the `i18n` module docs for
//! the conventions and the coverage test.

pub const ENTRIES: &[(&str, &str)] = &[
    ("There is already an instance with the id \"{id}\"", "Já existe uma instância com o id \"{id}\""),
    ("Instance \"{id}\" not found", "Instância \"{id}\" não encontrada"),
];

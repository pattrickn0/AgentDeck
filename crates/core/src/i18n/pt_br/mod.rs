//! The Portuguese (pt-BR) translations, one file per area of the code so that tasks converting different areas never
//! touch the same file. Each file exports `ENTRIES: &[(&str, &str)]` as `(English, Portuguese)` pairs; the English text
//! is the lookup key. To add a new area, create the file, declare it below and list it in [`AREAS`].

use std::collections::HashMap;
use std::sync::OnceLock;

pub mod app;
pub mod common;
pub mod core;
pub mod docs;
pub mod git;
pub mod github;
pub mod settings;
pub mod tasks;

/// Every area file with its name (the name only shows up in test failures).
pub const AREAS: &[(&str, &[(&str, &str)])] = &[
    ("common", common::ENTRIES),
    ("settings", settings::ENTRIES),
    ("tasks", tasks::ENTRIES),
    ("git", git::ENTRIES),
    ("github", github::ENTRIES),
    ("docs", docs::ENTRIES),
    ("app", app::ENTRIES),
    ("core", core::ENTRIES),
];

/// The Portuguese text for the English `key`, or `None` when there is no translation.
pub fn lookup(key: &str) -> Option<&'static str> {
    static MAP: OnceLock<HashMap<&'static str, &'static str>> = OnceLock::new();
    MAP.get_or_init(|| AREAS.iter().flat_map(|(_, entries)| entries.iter().copied()).collect()).get(key).copied()
}

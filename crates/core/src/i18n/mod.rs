//! Translation of the text the user reads in the app (English by default, Portuguese as an option).
//!
//! **The English text is the key** (gettext style): `t!("Open folder…")` returns the Portuguese text when the
//! language is pt-BR and the English text otherwise; a missing translation falls back to English.
//!
//! - `t!("literal")` returns `&'static str`, so it goes anywhere a `&str` goes (egui calls, `Option<&'static str>`).
//! - With named placeholders it returns `String`: `t!("Task {id} merged into {base}", id = task.id, base = base)`.
//!   A bare name is shorthand for `name = name`: `t!("Download ({size})", size)`. Any `Display` value works.
//!
//! **Translations live in one file per area** (`pt_br/{common,settings,tasks,git,github,docs,app,core}.rs`), each
//! with `pub const ENTRIES: &[(&str, &str)]` as `(English, Portuguese)` pairs, so tasks converting different areas
//! never touch the same file. Text shared by several areas (Close, Cancel, Save...) goes in `common.rs`.
//!
//! **Text only agents read never goes through `t!`** and stays in English: `ask-agent` output, the Holt
//! instructions given to every tab, task briefs and feedback, bridge responses. The bridge also runs its request
//! threads under [`english`], so a core function shared by the UI and the bridge answers agents in English even when
//! the user chose Portuguese.
//!
//! **Coverage test** (`tests::every_t_literal_has_a_translation_and_every_entry_is_used`): it scans every
//! `crates/*/src/**/*.rs`, takes the string literal right after each `t!(` (escapes resolved like the compiler does,
//! including the `\` line continuation) and fails if a literal has no pt-BR entry or an entry is no longer used by
//! any `t!`. Lines that start with `//` (comments and doc comments) are skipped. So: write the key as a plain
//! `"..."` literal inside `t!` (no raw strings, no constants), add the pair to the area file, run
//! `cargo test -p holt-core i18n`.

use std::cell::Cell;
use std::fmt::Display;
use std::sync::atomic::{AtomicU8, Ordering};

use serde::{Deserialize, Deserializer, Serialize, Serializer};

pub mod pt_br;

/// The language of the app's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Lang {
    #[default]
    En,
    PtBr,
}

impl Lang {
    /// The code saved in `config.json`.
    pub fn code(self) -> &'static str {
        match self {
            Lang::En => "en",
            Lang::PtBr => "pt-BR",
        }
    }

    /// Reads a saved code; anything unknown is English (a bad value never throws away the whole config).
    pub fn from_code(code: &str) -> Lang {
        match code.trim().to_ascii_lowercase().as_str() {
            "pt-br" | "pt_br" | "pt" => Lang::PtBr,
            _ => Lang::En,
        }
    }
}

impl Serialize for Lang {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.code())
    }
}

impl<'de> Deserialize<'de> for Lang {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        // Any JSON value: a number or an object is English too, instead of failing the whole config.
        Ok(serde_json::Value::deserialize(d)?.as_str().map(Lang::from_code).unwrap_or_default())
    }
}

/// The process-wide language (0 = English, 1 = Portuguese).
static LANG: AtomicU8 = AtomicU8::new(0);

thread_local! {
    /// Overrides the process-wide language on this thread (see [`english`]).
    static FORCED: Cell<Option<Lang>> = const { Cell::new(None) };
}

/// Sets the process-wide language (the app calls it at startup and when the user switches).
pub fn set_lang(lang: Lang) {
    LANG.store(lang as u8, Ordering::Relaxed);
}

/// The language `t!` uses on this thread.
pub fn lang() -> Lang {
    FORCED.with(Cell::get).unwrap_or(if LANG.load(Ordering::Relaxed) == Lang::PtBr as u8 { Lang::PtBr } else { Lang::En })
}

/// Runs `f` with `lang` forced on this thread, whatever the process-wide language is. Nested calls restore the
/// outer one on the way out, even on panic.
pub fn with_lang<R>(lang: Lang, f: impl FnOnce() -> R) -> R {
    struct Restore(Option<Lang>);
    impl Drop for Restore {
        fn drop(&mut self) {
            FORCED.with(|c| c.set(self.0));
        }
    }
    let _restore = Restore(FORCED.with(|c| c.replace(Some(lang))));
    f()
}

/// Runs `f` in English: for code that answers agents (bridge request threads), which always read English.
pub fn english<R>(f: impl FnOnce() -> R) -> R {
    with_lang(Lang::En, f)
}

/// The text for the English `key` in the current language. Used by `t!`; call it directly only for keys that are
/// already literals somewhere in a `t!` (the coverage test only sees `t!`).
pub fn tr(key: &'static str) -> &'static str {
    match lang() {
        Lang::En => key,
        Lang::PtBr => pt_br::lookup(key).unwrap_or(key),
    }
}

/// Replaces each `{name}` in `text` with its value. A `{name}` with no value is left as is.
pub fn fill(text: &str, args: &[(&str, &dyn Display)]) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        out.push_str(&rest[..start]);
        let after = &rest[start + 1..];
        let value = after.find('}').and_then(|end| args.iter().find(|(name, _)| *name == &after[..end]).map(|(_, v)| (end, v)));
        match value {
            Some((end, v)) => {
                out.push_str(&v.to_string());
                rest = &after[end + 1..];
            }
            None => {
                out.push('{');
                rest = after;
            }
        }
    }
    out.push_str(rest);
    out
}

/// The `{name}` placeholders of a text, sorted (for the tests that compare both languages).
pub fn placeholders(text: &str) -> Vec<&str> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(start) = rest.find('{') {
        let after = &rest[start + 1..];
        match after.find('}') {
            Some(end) if !after[..end].is_empty() && after[..end].chars().all(|c| c.is_alphanumeric() || c == '_') => {
                found.push(&after[..end]);
                rest = &after[end + 1..];
            }
            _ => rest = after,
        }
    }
    found.sort();
    found
}

/// Translates user-facing text (see the module docs): `t!("Close")` is `&'static str`;
/// `t!("Model: {model}", model = m)` or `t!("Model: {model}", model)` is `String`.
#[macro_export]
macro_rules! t {
    ($key:literal $(,)?) => {
        $crate::i18n::tr($key)
    };
    ($key:literal, $($name:ident $(= $val:expr)?),+ $(,)?) => {
        $crate::i18n::fill(
            $crate::i18n::tr($key),
            &[$((stringify!($name), &$crate::t!(@arg $name $(= $val)?) as &dyn ::std::fmt::Display)),+],
        )
    };
    (@arg $name:ident = $val:expr) => {
        $val
    };
    (@arg $name:ident) => {
        $name
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::{BTreeSet, HashMap};
    use std::path::{Path, PathBuf};

    #[test]
    fn english_is_the_default_and_the_key() {
        assert_eq!(Lang::default(), Lang::En);
        assert_eq!(with_lang(Lang::En, || t!("Cancel")), "Cancel");
        assert_eq!(with_lang(Lang::PtBr, || t!("Cancel")), "Cancelar");
    }

    #[test]
    fn a_missing_translation_falls_back_to_english() {
        assert_eq!(with_lang(Lang::PtBr, || tr("No such key in any area file")), "No such key in any area file");
    }

    #[test]
    fn placeholders_are_filled_by_name_in_any_order() {
        let size = "1.6 GB";
        assert_eq!(with_lang(Lang::PtBr, || t!("Download ({size})", size)), "Baixar (1.6 GB)");
        assert_eq!(with_lang(Lang::En, || t!("{agent} · model", agent = "claude")), "claude · model");
        assert_eq!(fill("{b} then {a}", &[("a", &1), ("b", &"two")]), "two then 1");
        assert_eq!(fill("{missing} and {", &[("a", &1)]), "{missing} and {");
    }

    #[test]
    fn the_english_scope_wins_over_the_process_language_and_nests() {
        with_lang(Lang::PtBr, || {
            assert_eq!(lang(), Lang::PtBr);
            english(|| assert_eq!(t!("Close"), "Close"));
            assert_eq!(t!("Close"), "Fechar", "the outer scope comes back");
        });
        let caught = std::panic::catch_unwind(|| english(|| panic!("boom")));
        assert!(caught.is_err());
        assert_eq!(FORCED.with(Cell::get), None, "a panic inside the scope still restores it");
    }

    #[test]
    fn the_saved_code_round_trips_and_unknown_is_english() {
        for lang in [Lang::En, Lang::PtBr] {
            let json = serde_json::to_string(&lang).unwrap();
            assert_eq!(serde_json::from_str::<Lang>(&json).unwrap(), lang, "{json}");
        }
        assert_eq!(serde_json::to_string(&Lang::PtBr).unwrap(), "\"pt-BR\"");
        assert_eq!(serde_json::from_str::<Lang>("\"klingon\"").unwrap(), Lang::En);
        assert_eq!(serde_json::from_str::<Lang>("null").unwrap(), Lang::En);
    }

    #[test]
    fn no_english_key_has_two_different_translations() {
        // The same key in two area files is fine when both say the same (tasks converting areas in parallel may both
        // add it); two different translations would make one of them silently lose.
        let mut seen: HashMap<&str, (&str, &str)> = HashMap::new();
        for (area, entries) in pt_br::AREAS {
            for (en, pt) in *entries {
                if let Some((other, other_pt)) = seen.insert(en, (area, pt)) {
                    assert_eq!(
                        other_pt, *pt,
                        "\"{en}\" has different translations in pt_br/{other}.rs and pt_br/{area}.rs: pick one (or move it to common.rs)"
                    );
                }
            }
        }
    }

    #[test]
    fn each_entry_has_the_same_placeholders_in_both_languages() {
        for (area, entries) in pt_br::AREAS {
            for (en, pt) in *entries {
                assert_eq!(placeholders(en), placeholders(pt), "pt_br/{area}.rs: \"{en}\" → \"{pt}\"");
                assert!(!pt.trim().is_empty(), "pt_br/{area}.rs: \"{en}\" has an empty translation");
            }
        }
    }

    /// Every `*.rs` under `crates/*/src`.
    fn sources() -> Vec<PathBuf> {
        fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
            for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    walk(&path, out);
                } else if path.extension().is_some_and(|e| e == "rs") {
                    out.push(path);
                }
            }
        }
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("crates/").to_path_buf();
        let mut out = Vec::new();
        for krate in std::fs::read_dir(&crates).expect("crates/").flatten() {
            walk(&krate.path().join("src"), &mut out);
        }
        assert!(out.len() > 20, "the scan found too few sources under {}", crates.display());
        out
    }

    /// Reads the string literal at the start of `s` (after the opening `"`), resolving escapes the way rustc does.
    /// Returns the text, or `None` if the literal is not closed.
    fn string_literal(s: &str) -> Option<String> {
        let mut out = String::new();
        let mut chars = s.chars().peekable();
        while let Some(c) = chars.next() {
            match c {
                '"' => return Some(out),
                '\\' => match chars.next()? {
                    'n' => out.push('\n'),
                    't' => out.push('\t'),
                    'r' => out.push('\r'),
                    '0' => out.push('\0'),
                    '\\' => out.push('\\'),
                    '"' => out.push('"'),
                    '\'' => out.push('\''),
                    '\n' | '\r' => {
                        while chars.peek().is_some_and(|c| c.is_whitespace()) {
                            chars.next();
                        }
                    }
                    'u' => {
                        let hex: String = chars.by_ref().skip(1).take_while(|&c| c != '}').collect();
                        out.push(char::from_u32(u32::from_str_radix(&hex, 16).ok()?)?);
                    }
                    other => panic!("unexpected escape \\{other} in a t! literal"),
                },
                c => out.push(c),
            }
        }
        None
    }

    /// The macro call the scan looks for, spelled so that this file does not contain it.
    const CALL: &str = concat!("t", "!(");

    /// The keys of every `t!("...")` in `text` (comment lines skipped). `Err` = a `t!(` whose argument is not a
    /// plain string literal, which the scan could not check.
    fn t_literals(text: &str) -> Result<Vec<String>, String> {
        // Comment lines are blanked (kept as newlines, so a literal's line continuation still parses).
        let code: String = text.split_inclusive('\n').map(|line| if line.trim_start().starts_with("//") { "\n" } else { line }).collect();
        let mut keys = Vec::new();
        let mut from = 0;
        while let Some(found) = code[from..].find(CALL) {
            let at = from + found;
            from = at + 3;
            // `assert!(`, `format!(`...: the `t` must start the macro name (a path like `holt_core::t!(` is fine).
            if code[..at].chars().next_back().is_some_and(|c| c.is_alphanumeric() || c == '_') {
                continue;
            }
            let arg = code[from..].trim_start();
            if arg.starts_with('@') {
                continue; // the macro's own internal arm
            }
            let Some(lit) = arg.strip_prefix('"') else {
                let line = code[..at].matches('\n').count() + 1;
                return Err(format!("line {line}: t! takes a plain \"...\" literal"));
            };
            keys.push(string_literal(lit).ok_or_else(|| "unclosed literal".to_owned())?);
        }
        Ok(keys)
    }

    #[test]
    fn the_scan_reads_literals_like_the_compiler() {
        // Written with `T!(` so the scan of this very file does not take them for real calls.
        let src = "let a = T!(\"Close\");\n// T!(\"In a comment\")\nassert!(\"no\");\nlet b = crate::T!(\"Say \\\"hi\\\" {name}\", name);\nlet c = T!(\"one \\\n    two\");\n";
        assert_eq!(t_literals(&src.replace("T!(", CALL)).unwrap(), ["Close", "Say \"hi\" {name}", "one two"]);
        assert!(t_literals(&"T!(KEY)".replace('T', "t")).is_err());
    }

    #[test]
    fn every_t_literal_has_a_translation_and_every_entry_is_used() {
        let mut used = BTreeSet::new();
        let mut missing = Vec::new();
        for path in sources() {
            let text = std::fs::read_to_string(&path).unwrap_or_default();
            let keys = t_literals(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
            for key in keys {
                if pt_br::lookup(&key).is_none() {
                    missing.push(format!("{}: \"{key}\"", path.display()));
                }
                used.insert(key);
            }
        }
        assert!(missing.is_empty(), "t! literals with no pt-BR entry (add them to the area file in i18n/pt_br/):\n{}", missing.join("\n"));
        let unused: Vec<String> = pt_br::AREAS
            .iter()
            .flat_map(|(area, entries)| entries.iter().map(move |(en, _)| (area, en)))
            .filter(|(_, en)| !used.contains(**en))
            .map(|(area, en)| format!("pt_br/{area}.rs: \"{en}\""))
            .collect();
        assert!(unused.is_empty(), "pt-BR entries no t! uses any more (remove them):\n{}", unused.join("\n"));
    }
}

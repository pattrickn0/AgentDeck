//! The app's environment variables: all of them start with `HOLT_`.
//!
//! Every place that reads one of these variables goes through here (app, core and CLIs).
//!
//! Migration (remove after 0.4, `memory/MAINTENANCE.md` §6): tabs opened before the rename from AgentDeck still carry
//! `AGENTDECK_*`; reading falls back to the old prefix so that `ask-agent` keeps working in them until they are reopened.

use std::ffi::OsString;

/// The prefix of the app's variables.
pub const PREFIX: &str = "HOLT_";

/// The prefix before the rename from AgentDeck: still read as a fallback, never written.
const LEGACY_PREFIX: &str = "AGENTDECK_";

/// The name of `key` without the app prefix (case-insensitive, like Windows), or `None` if it is
/// not an app variable.
fn app_var_name(key: &str) -> Option<&str> {
    [PREFIX, LEGACY_PREFIX]
        .into_iter()
        .find_map(|prefix| key.get(..prefix.len()).filter(|k| k.eq_ignore_ascii_case(prefix)).map(|_| &key[prefix.len()..]))
}

/// `true` if `key` is an app variable.
pub fn is_app_var(key: &str) -> bool {
    app_var_name(key).is_some()
}

/// `true` if `key` is the app variable `name`.
pub fn is_app_var_named(key: &str, name: &str) -> bool {
    app_var_name(key).is_some_and(|rest| rest.eq_ignore_ascii_case(name))
}

/// Value of `HOLT_<name>` in the process environment (or, failing that, of the old `AGENTDECK_<name>`). An empty value
/// counts as absent.
pub fn env_var_os(name: &str) -> Option<OsString> {
    [PREFIX, LEGACY_PREFIX].into_iter().find_map(|prefix| std::env::var_os(format!("{prefix}{name}")).filter(|v| !v.is_empty()))
}

/// Like [`env_var_os`], as text (a value that is not UTF-8 counts as absent).
pub fn env_var(name: &str) -> Option<String> {
    env_var_os(name).and_then(|v| v.into_string().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_vars_are_recognized_by_the_prefix() {
        assert!(is_app_var("HOLT_ROLE") && is_app_var("holt_role"));
        assert!(!is_app_var("HOME") && !is_app_var("AGENT") && !is_app_var("€"));
        assert!(!is_app_var("COMMANDDECK_ROLE"), "the CommandDeck prefix no longer applies");
        assert!(is_app_var_named("HOLT_ASK_WAIT", "ASK_WAIT") && is_app_var_named("holt_ask_wait", "ASK_WAIT"));
        assert!(!is_app_var_named("HOLT_ASK_WAITX", "ASK_WAIT") && !is_app_var_named("HOLT_ROLE", "ASK_WAIT"));
    }

    #[test]
    fn the_agentdeck_prefix_still_counts_as_an_app_variable() {
        assert!(is_app_var("AGENTDECK_ROLE") && is_app_var("agentdeck_role"));
        assert!(is_app_var_named("AGENTDECK_ASK_WAIT", "ASK_WAIT") && !is_app_var_named("AGENTDECK_ROLE", "ASK_WAIT"));
    }

    #[test]
    fn old_agentdeck_variables_are_read_as_a_fallback() {
        // Names used only by this test: nothing else reads these variables.
        std::env::set_var("AGENTDECK_TESTE_LEGACY_ONLY", "old");
        assert_eq!(env_var("TESTE_LEGACY_ONLY").as_deref(), Some("old"));
        std::env::set_var("AGENTDECK_TESTE_LEGACY_BOTH", "old");
        std::env::set_var("HOLT_TESTE_LEGACY_BOTH", "new");
        assert_eq!(env_var("TESTE_LEGACY_BOTH").as_deref(), Some("new"), "the new prefix wins");
        std::env::set_var("HOLT_TESTE_LEGACY_BOTH", "");
        assert_eq!(env_var("TESTE_LEGACY_BOTH").as_deref(), Some("old"), "an empty new value falls back");
        for k in ["AGENTDECK_TESTE_LEGACY_ONLY", "AGENTDECK_TESTE_LEGACY_BOTH", "HOLT_TESTE_LEGACY_BOTH"] {
            std::env::remove_var(k);
        }
    }

    #[test]
    fn an_empty_value_counts_as_missing() {
        // Name used only by this test: nothing else reads this variable.
        std::env::set_var("HOLT_TESTE_ENV_VAZIA", "");
        assert_eq!(env_var_os("TESTE_ENV_VAZIA"), None);
        std::env::set_var("HOLT_TESTE_ENV_VAZIA", "x");
        assert_eq!(env_var("TESTE_ENV_VAZIA").as_deref(), Some("x"));
        std::env::remove_var("HOLT_TESTE_ENV_VAZIA");
    }
}

//! The app's environment variables: all of them start with `AGENTDECK_`.
//!
//! Every place that reads one of these variables goes through here (app, core and CLIs).

use std::ffi::OsString;

/// The prefix of the app's variables.
pub const PREFIX: &str = "AGENTDECK_";

/// The name of `key` without the app prefix (case-insensitive, like Windows), or `None` if it is
/// not an app variable.
fn app_var_name(key: &str) -> Option<&str> {
    key.get(..PREFIX.len()).filter(|k| k.eq_ignore_ascii_case(PREFIX)).map(|_| &key[PREFIX.len()..])
}

/// `true` if `key` is an app variable.
pub fn is_app_var(key: &str) -> bool {
    app_var_name(key).is_some()
}

/// `true` if `key` is the app variable `name`.
pub fn is_app_var_named(key: &str, name: &str) -> bool {
    app_var_name(key).is_some_and(|rest| rest.eq_ignore_ascii_case(name))
}

/// Value of `AGENTDECK_<name>` in the process environment. An empty value counts as absent.
pub fn env_var_os(name: &str) -> Option<OsString> {
    std::env::var_os(format!("{PREFIX}{name}")).filter(|v| !v.is_empty())
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
        assert!(is_app_var("AGENTDECK_ROLE") && is_app_var("agentdeck_role"));
        assert!(!is_app_var("HOME") && !is_app_var("AGENT") && !is_app_var("€"));
        assert!(!is_app_var("COMMANDDECK_ROLE"), "the old prefix no longer applies");
        assert!(is_app_var_named("AGENTDECK_ASK_WAIT", "ASK_WAIT") && is_app_var_named("agentdeck_ask_wait", "ASK_WAIT"));
        assert!(!is_app_var_named("AGENTDECK_ASK_WAITX", "ASK_WAIT") && !is_app_var_named("AGENTDECK_ROLE", "ASK_WAIT"));
    }

    #[test]
    fn an_empty_value_counts_as_missing() {
        // Name used only by this test: nothing else reads this variable.
        std::env::set_var("AGENTDECK_TESTE_ENV_VAZIA", "");
        assert_eq!(env_var_os("TESTE_ENV_VAZIA"), None);
        std::env::set_var("AGENTDECK_TESTE_ENV_VAZIA", "x");
        assert_eq!(env_var("TESTE_ENV_VAZIA").as_deref(), Some("x"));
        std::env::remove_var("AGENTDECK_TESTE_ENV_VAZIA");
    }
}

//! Variáveis de ambiente do app: todas começam por `AGENTDECK_`.
//!
//! Todo lugar que lê uma dessas variáveis passa por aqui (app, core e CLIs).

use std::ffi::OsString;

/// Prefixo das variáveis do app.
pub const PREFIX: &str = "AGENTDECK_";

/// O nome de `key` sem o prefixo do app (sem diferenciar maiúsculas, como o Windows), ou `None` se
/// não for uma variável do app.
fn app_var_name(key: &str) -> Option<&str> {
    key.get(..PREFIX.len()).filter(|k| k.eq_ignore_ascii_case(PREFIX)).map(|_| &key[PREFIX.len()..])
}

/// `true` se `key` é uma variável do app.
pub fn is_app_var(key: &str) -> bool {
    app_var_name(key).is_some()
}

/// `true` se `key` é a variável `name` do app.
pub fn is_app_var_named(key: &str, name: &str) -> bool {
    app_var_name(key).is_some_and(|rest| rest.eq_ignore_ascii_case(name))
}

/// Valor de `AGENTDECK_<name>` no ambiente do processo. Valor vazio conta como ausente.
pub fn env_var_os(name: &str) -> Option<OsString> {
    std::env::var_os(format!("{PREFIX}{name}")).filter(|v| !v.is_empty())
}

/// Como [`env_var_os`], em texto (um valor que não é UTF-8 conta como ausente).
pub fn env_var(name: &str) -> Option<String> {
    env_var_os(name).and_then(|v| v.into_string().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_vars_are_recognized_by_the_prefix() {
        assert!(is_app_var("AGENTDECK_ROLE") && is_app_var("agentdeck_role"));
        assert!(!is_app_var("HOME") && !is_app_var("AGENT") && !is_app_var("é"));
        assert!(!is_app_var("COMMANDDECK_ROLE"), "o prefixo antigo não vale mais");
        assert!(is_app_var_named("AGENTDECK_ASK_WAIT", "ASK_WAIT") && is_app_var_named("agentdeck_ask_wait", "ASK_WAIT"));
        assert!(!is_app_var_named("AGENTDECK_ASK_WAITX", "ASK_WAIT") && !is_app_var_named("AGENTDECK_ROLE", "ASK_WAIT"));
    }

    #[test]
    fn an_empty_value_counts_as_missing() {
        // Nome só deste teste: nenhum outro lê esta variável.
        std::env::set_var("AGENTDECK_TESTE_ENV_VAZIA", "");
        assert_eq!(env_var_os("TESTE_ENV_VAZIA"), None);
        std::env::set_var("AGENTDECK_TESTE_ENV_VAZIA", "x");
        assert_eq!(env_var("TESTE_ENV_VAZIA").as_deref(), Some("x"));
        std::env::remove_var("AGENTDECK_TESTE_ENV_VAZIA");
    }
}

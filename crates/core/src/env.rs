//! Variáveis de ambiente do app: `AGENTDECK_*`, com o prefixo do nome antigo (`COMMANDDECK_*`)
//! como reserva, para uma aba aberta pela versão anterior continuar funcionando.
//!
//! Todo lugar que lê uma dessas variáveis passa por aqui (app, core e CLIs). O app só ESCREVE o
//! prefixo novo no ambiente dos agentes.

use std::ffi::OsString;

/// Prefixo atual das variáveis.
pub const PREFIX: &str = "AGENTDECK_";
/// Prefixo do nome antigo, ainda lido (e filtrado) durante a migração. Não remova antes da hora:
/// ver `memory/aprendizados/nomes-antigos-aceitos-na-migracao.md`.
pub const LEGACY_PREFIX: &str = "COMMANDDECK_";

/// O nome de `key` sem o prefixo do app (o novo ou o antigo, sem diferenciar maiúsculas, como o
/// Windows), ou `None` se não for uma variável do app.
fn app_var_name(key: &str) -> Option<&str> {
    [PREFIX, LEGACY_PREFIX].iter().find_map(|p| key.get(..p.len()).filter(|k| k.eq_ignore_ascii_case(p)).map(|_| &key[p.len()..]))
}

/// `true` se `key` é uma variável do app, com o prefixo novo ou o antigo.
pub fn is_app_var(key: &str) -> bool {
    app_var_name(key).is_some()
}

/// `true` se `key` é a variável `name` do app, com qualquer um dos prefixos.
pub fn is_app_var_named(key: &str, name: &str) -> bool {
    app_var_name(key).is_some_and(|rest| rest.eq_ignore_ascii_case(name))
}

/// Lê `AGENTDECK_<name>` e, se faltar ou estiver vazia, `COMMANDDECK_<name>`, de `get`.
pub fn lookup(name: &str, get: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    [PREFIX, LEGACY_PREFIX].iter().find_map(|p| get(&format!("{p}{name}")).filter(|v| !v.is_empty()))
}

/// Valor de `AGENTDECK_<name>` (ou do nome antigo, se faltar ou estiver vazia) no ambiente do processo.
/// Valor vazio conta como ausente.
pub fn env_var_os(name: &str) -> Option<OsString> {
    lookup(name, |k| std::env::var_os(k))
}

/// Como [`env_var_os`], em texto (um valor que não é UTF-8 conta como ausente).
pub fn env_var(name: &str) -> Option<String> {
    env_var_os(name).and_then(|v| v.into_string().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<OsString> {
        let map: HashMap<String, OsString> = pairs.iter().map(|(k, v)| ((*k).to_owned(), OsString::from(v))).collect();
        move |k| map.get(k).cloned()
    }

    #[test]
    fn the_old_name_is_read_when_the_new_one_is_missing() {
        let get = env(&[("COMMANDDECK_PORT", "1234")]);
        assert_eq!(lookup("PORT", get), Some(OsString::from("1234")));
    }

    #[test]
    fn the_new_name_wins_when_both_exist() {
        let get = env(&[("COMMANDDECK_PORT", "1"), ("AGENTDECK_PORT", "2")]);
        assert_eq!(lookup("PORT", get), Some(OsString::from("2")));
    }

    #[test]
    fn an_empty_new_name_falls_back_to_the_old_one() {
        let get = env(&[("COMMANDDECK_PORT", "1"), ("AGENTDECK_PORT", "")]);
        assert_eq!(lookup("PORT", get), Some(OsString::from("1")));
        assert_eq!(lookup("PORT", env(&[("AGENTDECK_PORT", ""), ("COMMANDDECK_PORT", "")])), None);
        assert_eq!(lookup("PORT", env(&[])), None);
    }

    #[test]
    fn app_vars_are_recognized_with_both_prefixes() {
        assert!(is_app_var("AGENTDECK_ROLE") && is_app_var("COMMANDDECK_ROLE") && is_app_var("commanddeck_role"));
        assert!(!is_app_var("OLLAMA_HOST") && !is_app_var("AGENT") && !is_app_var("é"));
        assert!(is_app_var_named("AGENTDECK_ASK_WAIT", "ASK_WAIT") && is_app_var_named("commanddeck_ask_wait", "ASK_WAIT"));
        assert!(!is_app_var_named("AGENTDECK_ASK_WAITX", "ASK_WAIT") && !is_app_var_named("AGENTDECK_ROLE", "ASK_WAIT"));
    }
}

//! Hierarquia das instâncias de um projeto (quem é chefe de quem), guardada em
//! `ProjectCfg.tab_parents` (id da instância → id do chefe). Funções puras; o `Core` aplica.

use std::collections::BTreeMap;

/// Chefe e equipe de uma instância.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Team {
    /// A quem a instância responde (`None` = ao usuário).
    pub boss: Option<String>,
    /// Quem responde a ela, na ordem de `open`.
    pub crew: Vec<String>,
}

/// Pôr `child` sob `parent` criaria um ciclo (`parent` é o próprio `child` ou alguém da equipe dele).
pub fn would_cycle(parents: &BTreeMap<String, String>, child: &str, parent: &str) -> bool {
    let mut cur = Some(parent);
    // Sem ciclo, a cadeia tem no máximo `len + 1` ids até acabar; o limite cobre um mapa já
    // corrompido com ciclo (não trava).
    for _ in 0..parents.len() + 2 {
        match cur {
            Some(id) if id == child => return true,
            Some(id) => cur = parents.get(id).map(String::as_str),
            None => return false,
        }
    }
    true
}

/// Chefe e equipe de `id`, considerando só as instâncias abertas (`open`).
pub fn team_of(parents: &BTreeMap<String, String>, id: &str, open: &[String]) -> Team {
    Team {
        boss: parents.get(id).filter(|b| open.contains(b)).cloned(),
        crew: open.iter().filter(|o| parents.get(o.as_str()).is_some_and(|p| p == id)).cloned().collect(),
    }
}

/// A instância `closed` saiu: a equipe dela passa ao chefe dela (ou à raiz). Devolve quem mudou de
/// chefe (para avisar), mais o chefe dela, se houver: ele perdeu um subordinado (e talvez herdou a equipe).
pub fn remove(parents: &mut BTreeMap<String, String>, closed: &str) -> Vec<String> {
    let boss = parents.remove(closed);
    let crew: Vec<String> = parents.iter().filter(|(_, p)| *p == closed).map(|(c, _)| c.clone()).collect();
    for c in &crew {
        match &boss {
            Some(b) => parents.insert(c.clone(), b.clone()),
            None => parents.remove(c),
        };
    }
    let mut touched = crew;
    touched.extend(boss);
    touched
}

/// A instância `old` passou a se chamar `new`.
pub fn rename(parents: &mut BTreeMap<String, String>, old: &str, new: &str) {
    if let Some(p) = parents.remove(old) {
        parents.insert(new.to_owned(), p);
    }
    for p in parents.values_mut() {
        if p == old {
            *p = new.to_owned();
        }
    }
}

/// Aviso anexado à próxima mensagem entregue a uma instância cuja posição na equipe mudou.
pub fn notice(team: &Team) -> String {
    let boss = match &team.boss {
        Some(b) => format!("você responde a \"{b}\" (reporte a ele com ask-agent --boss \"mensagem\")"),
        None => "você responde diretamente ao usuário".to_owned(),
    };
    let crew = if team.crew.is_empty() { "nenhum agente".to_owned() } else { team.crew.join(", ") };
    format!(
        "[AgentDeck] Sua posição na equipe mudou: {boss}; sua equipe: {crew}. Delegue de preferência à sua equipe. \
Consulte a qualquer momento com ask-agent --team."
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn map(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs.iter().map(|(c, p)| (c.to_string(), p.to_string())).collect()
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn ciclo_e_recusado() {
        let m = map(&[("claude@dev", "claude"), ("codex", "claude@dev")]);
        assert!(would_cycle(&m, "claude", "claude"), "chefe de si mesmo");
        assert!(would_cycle(&m, "claude", "codex"), "o chefe ficaria abaixo do subordinado");
        assert!(!would_cycle(&m, "codex", "claude"));
        assert!(!would_cycle(&m, "antigravity", "codex"));
        // Mapa já corrompido não trava.
        assert!(would_cycle(&map(&[("a", "b"), ("b", "a")]), "c", "a"));
    }

    #[test]
    fn equipe_so_com_abertos() {
        let m = map(&[("claude@dev", "claude"), ("antigravity", "claude"), ("claude", "fechado")]);
        let open = ids(&["claude", "claude@dev", "antigravity"]);
        assert_eq!(team_of(&m, "claude", &open), Team { boss: None, crew: ids(&["claude@dev", "antigravity"]) });
        assert_eq!(team_of(&m, "antigravity", &open), Team { boss: Some("claude".into()), crew: vec![] });
    }

    #[test]
    fn fechar_passa_a_equipe_ao_avo() {
        let mut m = map(&[("dev", "claude"), ("codex", "dev"), ("qa", "dev")]);
        let mut touched = remove(&mut m, "dev");
        touched.sort();
        assert_eq!(touched, ids(&["claude", "codex", "qa"]));
        assert_eq!(m, map(&[("codex", "claude"), ("qa", "claude")]));
        // Sem chefe, a equipe vai para a raiz.
        assert_eq!(remove(&mut m, "claude"), ids(&["codex", "qa"]));
        assert!(m.is_empty());
    }

    #[test]
    fn o_chefe_que_perde_um_subordinado_sem_equipe_e_avisado() {
        let mut m = map(&[("dev", "claude"), ("qa", "claude")]);
        assert_eq!(remove(&mut m, "dev"), ids(&["claude"]));
        assert_eq!(m, map(&[("qa", "claude")]));
        // Sem chefe nem equipe, ninguém muda de posição.
        assert!(remove(&mut m, "solto").is_empty());
    }

    #[test]
    fn renomear_mantem_a_arvore() {
        let mut m = map(&[("dev", "claude"), ("codex", "dev")]);
        rename(&mut m, "dev", "dev@ana");
        assert_eq!(m, map(&[("dev@ana", "claude"), ("codex", "dev@ana")]));
    }
}

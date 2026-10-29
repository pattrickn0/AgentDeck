//! Hierarchy of a project's instances (who is whose boss), stored in
//! `ProjectCfg.tab_parents` (instance id → boss id). Pure functions; the `Core` applies them.

use std::collections::BTreeMap;

/// Boss and crew of an instance.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Team {
    /// Who the instance answers to (`None` = the user).
    pub boss: Option<String>,
    /// Who answers to it, in `open` order.
    pub crew: Vec<String>,
}

/// Putting `child` under `parent` would create a cycle (`parent` is `child` itself or someone in its crew).
pub fn would_cycle(parents: &BTreeMap<String, String>, child: &str, parent: &str) -> bool {
    let mut cur = Some(parent);
    // Without a cycle the chain has at most `len + 1` ids before it ends; the limit covers a map already
    // corrupted with a cycle (it does not hang).
    for _ in 0..parents.len() + 2 {
        match cur {
            Some(id) if id == child => return true,
            Some(id) => cur = parents.get(id).map(String::as_str),
            None => return false,
        }
    }
    true
}

/// Boss and crew of `id`, considering only the open instances (`open`).
pub fn team_of(parents: &BTreeMap<String, String>, id: &str, open: &[String]) -> Team {
    Team {
        boss: parents.get(id).filter(|b| open.contains(b)).cloned(),
        crew: open.iter().filter(|o| parents.get(o.as_str()).is_some_and(|p| p == id)).cloned().collect(),
    }
}

/// The instance `closed` left: its crew passes to its boss (or to the root). Returns who changed boss
/// (to be told), plus its own boss, if any: it lost a subordinate (and may have inherited the crew).
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

/// The instance `old` is now called `new`.
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

/// Notice attached to the next message delivered to an instance whose place in the team changed.
pub fn notice(team: &Team) -> String {
    let boss = match &team.boss {
        Some(b) => format!("you answer to \"{b}\" (report to him with ask-agent --boss \"message\")"),
        None => "you answer directly to the user".to_owned(),
    };
    let crew = if team.crew.is_empty() { "no agents".to_owned() } else { team.crew.join(", ") };
    format!(
        "[AgentDeck] Your place in the team changed: {boss}; your crew: {crew}. Delegate preferably to your crew. \
Check it at any time with ask-agent --team."
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
    fn a_cycle_is_refused() {
        let m = map(&[("claude@dev", "claude"), ("codex", "claude@dev")]);
        assert!(would_cycle(&m, "claude", "claude"), "boss of itself");
        assert!(would_cycle(&m, "claude", "codex"), "the boss would end up below its own subordinate");
        assert!(!would_cycle(&m, "codex", "claude"));
        assert!(!would_cycle(&m, "antigravity", "codex"));
        // A map already corrupted does not hang.
        assert!(would_cycle(&map(&[("a", "b"), ("b", "a")]), "c", "a"));
    }

    #[test]
    fn the_team_only_has_open_instances() {
        let m = map(&[("claude@dev", "claude"), ("antigravity", "claude"), ("claude", "fechado")]);
        let open = ids(&["claude", "claude@dev", "antigravity"]);
        assert_eq!(team_of(&m, "claude", &open), Team { boss: None, crew: ids(&["claude@dev", "antigravity"]) });
        assert_eq!(team_of(&m, "antigravity", &open), Team { boss: Some("claude".into()), crew: vec![] });
    }

    #[test]
    fn closing_hands_the_crew_to_the_grandparent() {
        let mut m = map(&[("dev", "claude"), ("codex", "dev"), ("qa", "dev")]);
        let mut touched = remove(&mut m, "dev");
        touched.sort();
        assert_eq!(touched, ids(&["claude", "codex", "qa"]));
        assert_eq!(m, map(&[("codex", "claude"), ("qa", "claude")]));
        // Without a boss, the crew goes to the root.
        assert_eq!(remove(&mut m, "claude"), ids(&["codex", "qa"]));
        assert!(m.is_empty());
    }

    #[test]
    fn the_boss_that_loses_a_subordinate_without_crew_is_told() {
        let mut m = map(&[("dev", "claude"), ("qa", "claude")]);
        assert_eq!(remove(&mut m, "dev"), ids(&["claude"]));
        assert_eq!(m, map(&[("qa", "claude")]));
        // Without boss or crew, nobody changes place.
        assert!(remove(&mut m, "solto").is_empty());
    }

    #[test]
    fn renaming_keeps_the_tree() {
        let mut m = map(&[("dev", "claude"), ("codex", "dev")]);
        rename(&mut m, "dev", "dev@ana");
        assert_eq!(m, map(&[("dev@ana", "claude"), ("codex", "dev@ana")]));
    }
}

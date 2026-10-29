//! Agent instances: every tab is an instance with its own identity.
//!
//! The id is `<model>[@<role>[@<name>]]`:
//!
//! - `claude` — the main instance (without a role), the one the user opens with "+";
//! - `claude@dev`, `claude@design` — instances with a role, opened by an agent (or by the user);
//! - `claude@dev@ana` — when another one with the same model **and** role already exists, the new one gets a name.
//!
//! `claude@main@bia` is Claude's second main instance (without a role). The model may contain
//! `:` and `/` (`agent:model`, `agent:user/model:tag`), which is why the separator is `@`.
//!
//! This module is only name logic and target resolution; the interface creates the tabs.

use crate::t;

/// Separator between model, role and name.
pub const SEP: char = '@';
/// Implicit role of an instance without a role.
pub const MAIN_ROLE: &str = "main";

/// Separator between the project and the id in scoped keys (never appears in an id).
pub const SCOPE_SEP: char = '#';

/// Key of an instance with the project scope (`p1a2b3c4d#claude@dev`). It is the key used in pending
/// questions, in activity, in queues and in the bridge's headless sessions: each project has its own
/// instances. What agents and people see is always the id without the project.
pub fn scoped(project: &str, id: &str) -> String {
    format!("{project}{SCOPE_SEP}{id}")
}

/// Undoes [`scoped`]: `(project, id)`. A key without scope is `("", key)`.
pub fn unscoped(key: &str) -> (&str, &str) {
    key.split_once(SCOPE_SEP).unwrap_or(("", key))
}

/// Names given to repeated instances (same model and role), in order.
const NAMES: &[&str] =
    &["ana", "bia", "caio", "davi", "eva", "fabio", "gabi", "hugo", "iris", "joao", "kaua", "lia", "marco", "nina", "otto", "paula"];

/// Identity of an instance.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    /// Model/agent (`claude`, `antigravity`…).
    pub kind: String,
    /// Role (`dev`, `design`…); `None` = main instance.
    pub role: Option<String>,
    /// Name, only when model + role repeats.
    pub name: Option<String>,
}

/// Role and name: lowercase letters, digits, `-` and `_`.
fn valid_word(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

/// Model: letters, digits and `:._/+-` (`claude`, `antigravity`…).
/// The id goes into prompts and environment variables: no spaces, quotes or control characters.
fn valid_kind(s: &str) -> bool {
    !s.is_empty() && s.len() <= 128 && s.chars().all(|c| c.is_ascii_alphanumeric() || ":._/+-".contains(c))
}

impl Ident {
    /// The main instance of a model.
    pub fn main(kind: &str) -> Self {
        Self { kind: kind.to_owned(), role: None, name: None }
    }

    /// `kind[@role[@name]]`.
    pub fn id(&self) -> String {
        match (&self.role, &self.name) {
            (None, None) => self.kind.clone(),
            (Some(r), None) => format!("{}{SEP}{r}", self.kind),
            (r, Some(n)) => format!("{}{SEP}{}{SEP}{n}", self.kind, r.as_deref().unwrap_or(MAIN_ROLE)),
        }
    }

    /// Reads an id. `None` if it comes malformed (invalid role/name, too many parts).
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(SEP);
        let kind = parts.next().filter(|k| valid_kind(k))?.to_owned();
        let role = parts.next();
        let name = parts.next();
        if parts.next().is_some() {
            return None;
        }
        let role = match role {
            None => None,
            Some(r) if !valid_word(r) => return None,
            Some(MAIN_ROLE) => None,
            Some(r) => Some(r.to_owned()),
        };
        let name = match name {
            None => None,
            Some(n) if !valid_word(n) => return None,
            Some(n) => Some(n.to_owned()),
        };
        Some(Self { kind, role, name })
    }

    /// Readable label: `Claude`, `Claude · Dev`, `Claude · Dev · Ana`. `kind_label` is the model's name.
    pub fn label(&self, kind_label: &str) -> String {
        let cap = |s: &str| {
            let mut c = s.chars();
            c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
        };
        let mut out = kind_label.to_owned();
        if let Some(r) = &self.role {
            out.push_str(&format!(" · {}", cap(r)));
        }
        if let Some(n) = &self.name {
            if self.role.is_none() {
                out.push_str(&format!(" · {}", cap(MAIN_ROLE)));
            }
            out.push_str(&format!(" · {}", cap(n)));
        }
        out
    }
}

/// Known instances (open tabs and the ones being opened).
#[derive(Debug, Default)]
pub struct Instances {
    list: Vec<Ident>,
}

impl Instances {
    pub fn ids(&self) -> Vec<String> {
        self.list.iter().map(Ident::id).collect()
    }

    pub fn contains(&self, id: &str) -> bool {
        self.list.iter().any(|i| i.id() == id)
    }

    pub fn get(&self, id: &str) -> Option<&Ident> {
        self.list.iter().find(|i| i.id() == id)
    }

    /// Registers an already defined instance (tab restoration). Idempotent.
    pub fn register(&mut self, ident: Ident) {
        if !self.contains(&ident.id()) {
            self.list.push(ident);
        }
    }

    pub fn remove(&mut self, id: &str) -> bool {
        let before = self.list.len();
        self.list.retain(|i| i.id() != id);
        self.list.len() != before
    }

    /// Renames an existing instance. Returns an error if the new id is already in use
    /// or if the original instance is not found.
    pub fn rename(&mut self, old_id: &str, new_ident: Ident) -> Result<(), String> {
        let new_id = new_ident.id();
        if old_id != new_id && self.contains(&new_id) {
            return Err(t!("There is already an instance with the id \"{id}\"", id = new_id));
        }
        let Some(pos) = self.list.iter().position(|i| i.id() == old_id) else {
            return Err(t!("Instance \"{id}\" not found", id = old_id));
        };
        self.list[pos] = new_ident;
        Ok(())
    }

    /// Creates and registers a **new** instance of the model/role: without a name if the model+role pair
    /// is free; otherwise with the first free name. `preferred` is a requested name, used if it is free.
    pub fn allocate(&mut self, kind: &str, role: Option<&str>, preferred: Option<&str>) -> Ident {
        let role = role.filter(|r| *r != MAIN_ROLE).map(str::to_owned);
        let make = |name: Option<String>| Ident { kind: kind.to_owned(), role: role.clone(), name };
        let taken = |ident: &Ident| self.contains(&ident.id());
        let ident = if let Some(n) = preferred.filter(|n| valid_word(n)).map(str::to_owned).filter(|n| !taken(&make(Some(n.clone())))) {
            make(Some(n))
        } else if !taken(&make(None)) {
            make(None)
        } else {
            let free = NAMES.iter().find(|n| !taken(&make(Some((**n).to_owned()))));
            match free {
                Some(n) => make(Some((*n).to_owned())),
                None => (2..).map(|i| make(Some(format!("n{i}")))).find(|c| !taken(c)).expect("there is always a free name"),
            }
        };
        self.list.push(ident.clone());
        ident
    }

    /// Resolves a question's target. Returns the instance and whether it was **created** now (the
    /// interface has to open the tab), or the ids in conflict when the target is ambiguous.
    ///
    /// - an existing exact id → it;
    /// - only the model (`claude`): the main one, otherwise the model's only instance; several without a main → ambiguous;
    /// - model + role: the only one with that role; several → ambiguous;
    /// - nothing found (or `force_new`) → creates it.
    pub fn resolve_or_create(&mut self, want: &Ident, force_new: bool) -> Result<(Ident, bool), Vec<String>> {
        if !force_new {
            if let Some(found) = self.find(want)? {
                return Ok((found, false));
            }
        }
        Ok((self.allocate(&want.kind, want.role.as_deref(), want.name.as_deref()), true))
    }

    /// Like [`Instances::resolve_or_create`], without creating: `Ok(None)` if no instance serves.
    pub fn find(&self, want: &Ident) -> Result<Option<Ident>, Vec<String>> {
        if let Some(found) = self.get(&want.id()) {
            return Ok(Some(found.clone()));
        }
        if want.name.is_none() {
            let same_role: Vec<&Ident> = self.list.iter().filter(|i| i.kind == want.kind && i.role == want.role).collect();
            match same_role.len() {
                1 => return Ok(Some(same_role[0].clone())),
                n if n > 1 => return Err(same_role.iter().map(|i| i.id()).collect()),
                _ => {}
            }
            // Only the model: without a main one, its single instance serves.
            if want.role.is_none() {
                let any: Vec<&Ident> = self.list.iter().filter(|i| i.kind == want.kind).collect();
                match any.len() {
                    1 => return Ok(Some(any[0].clone())),
                    n if n > 1 => return Err(any.iter().map(|i| i.id()).collect()),
                    _ => {}
                }
            }
        }
        Ok(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Ident {
        Ident::parse(s).expect("valid id")
    }

    #[test]
    fn scoped_keys_round_trip() {
        let key = scoped("p0011aabb", "outro:user/custom:latest@qa@ana");
        assert_eq!(key, "p0011aabb#outro:user/custom:latest@qa@ana");
        assert_eq!(unscoped(&key), ("p0011aabb", "outro:user/custom:latest@qa@ana"));
        assert_eq!(unscoped("claude"), ("", "claude"));
        assert!(Ident::parse(&key).is_none(), "`#` never appears in an id");
    }

    #[test]
    fn ids_round_trip() {
        for s in [
            "claude",
            "claude@dev",
            "claude@dev@ana",
            "claude@main@bia",
            "outro:qwen3.5:4b",
            "outro:qwen3.5:4b@dev",
            "outro:user/custom:latest@qa",
        ] {
            assert_eq!(id(s).id(), s, "{s}");
        }
        // "main" is the implicit role.
        assert_eq!(id("claude@main"), Ident::main("claude"));
        assert_eq!(id("claude@main").id(), "claude");
        assert_eq!(id("claude@main@bia").role, None);
        assert_eq!(id("claude@dev@ana"), Ident { kind: "claude".into(), role: Some("dev".into()), name: Some("ana".into()) });
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for s in [
            "",
            "@dev",
            "claude@",
            "claude@Dev",
            "claude@d ev",
            "claude@dev@ana@x",
            "claude@dev@",
            "claude@dev;rm",
            "claude@@ana",
            "claude@dev@josé",
            "claude@dev@ana!",
            "claude@cárgo",
        ] {
            assert!(Ident::parse(s).is_none(), "{s:?}");
        }
        assert!(Ident::parse(&format!("claude@{}", "a".repeat(33))).is_none());
        // The model is validated too: the id goes into prompts typed into other tabs.
        for s in ["cla ude", "claude\"", "claude\x1b[201~", "claude\n", "a&b", "x<y", "usuário"] {
            assert!(Ident::parse(s).is_none(), "{s:?}");
        }
        assert!(Ident::parse("outro:user/custom:latest").is_some());
        assert!(Ident::parse("outro:qwen3.5:9b@dev").is_some());
    }

    #[test]
    fn labels_are_readable() {
        assert_eq!(id("claude").label("Claude"), "Claude");
        assert_eq!(id("claude@dev").label("Claude"), "Claude · Dev");
        assert_eq!(id("claude@dev@ana").label("Claude"), "Claude · Dev · Ana");
        assert_eq!(id("claude@main@bia").label("Claude"), "Claude · Main · Bia");
    }

    #[test]
    fn duplicates_get_names_in_order() {
        let mut r = Instances::default();
        assert_eq!(r.allocate("claude", None, None).id(), "claude");
        assert_eq!(r.allocate("claude", None, None).id(), "claude@main@ana");
        assert_eq!(r.allocate("claude", None, None).id(), "claude@main@bia");
        // A different role does not count as a repetition.
        assert_eq!(r.allocate("claude", Some("dev"), None).id(), "claude@dev");
        assert_eq!(r.allocate("claude", Some("dev"), None).id(), "claude@dev@ana");
        assert_eq!(r.allocate("antigravity", None, None).id(), "antigravity");
        // A requested name, if free; otherwise the next one.
        assert_eq!(r.allocate("claude", Some("dev"), Some("zeca")).id(), "claude@dev@zeca");
        assert_eq!(r.allocate("claude", Some("dev"), Some("zeca")).id(), "claude@dev@bia");
        // Closing frees the name.
        assert!(r.remove("claude@dev@ana"));
        assert_eq!(r.allocate("claude", Some("dev"), None).id(), "claude@dev@ana");
    }

    #[test]
    fn names_never_run_out() {
        let mut r = Instances::default();
        let ids: Vec<String> = (0..40).map(|_| r.allocate("claude", None, None).id()).collect();
        let unique: std::collections::HashSet<&String> = ids.iter().collect();
        assert_eq!(unique.len(), 40);
    }

    #[test]
    fn resolving_a_plain_model() {
        let mut r = Instances::default();
        // Nothing open: it creates the main one.
        let (a, created) = r.resolve_or_create(&Ident::main("claude"), false).unwrap();
        assert_eq!((a.id().as_str(), created), ("claude", true));
        // Again: it is the same one, already existing.
        let (a, created) = r.resolve_or_create(&Ident::main("claude"), false).unwrap();
        assert_eq!((a.id().as_str(), created), ("claude", false));
        // With other instances, "claude" is still the main one.
        r.allocate("claude", Some("dev"), None);
        assert_eq!(r.resolve_or_create(&Ident::main("claude"), false).unwrap().0.id(), "claude");
    }

    #[test]
    fn a_plain_model_without_main_uses_the_only_instance_or_is_ambiguous() {
        let mut r = Instances::default();
        r.register(id("claude@dev"));
        assert_eq!(r.resolve_or_create(&Ident::main("claude"), false).unwrap(), (id("claude@dev"), false));
        r.register(id("claude@design"));
        assert_eq!(
            r.resolve_or_create(&Ident::main("claude"), false).unwrap_err(),
            vec!["claude@dev".to_owned(), "claude@design".to_owned()]
        );
    }

    #[test]
    fn resolving_a_role() {
        let mut r = Instances::default();
        r.register(Ident::main("claude"));
        // The role does not exist yet: it creates it.
        let (dev, created) = r.resolve_or_create(&id("claude@dev"), false).unwrap();
        assert_eq!((dev.id().as_str(), created), ("claude@dev", true));
        // Asking for the same role again reuses the instance.
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap(), (id("claude@dev"), false));
        // --new forces another one: it gets a name.
        let (second, created) = r.resolve_or_create(&id("claude@dev"), true).unwrap();
        assert_eq!((second.id().as_str(), created), ("claude@dev@ana", true));
        // Now "claude@dev" is the exact id of the first one (without a name): it keeps resolving to it.
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap().0.id(), "claude@dev");
        // And the exact name too.
        assert_eq!(r.resolve_or_create(&id("claude@dev@ana"), false).unwrap(), (id("claude@dev@ana"), false));
    }

    #[test]
    fn a_role_with_only_named_instances_is_ambiguous_when_there_are_several() {
        let mut r = Instances::default();
        r.register(id("claude@dev@ana"));
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap(), (id("claude@dev@ana"), false), "only one: it serves");
        r.register(id("claude@dev@bia"));
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap_err().len(), 2);
    }

    #[test]
    fn finding_never_creates() {
        let mut r = Instances::default();
        assert_eq!(r.find(&Ident::main("claude")), Ok(None));
        assert!(r.ids().is_empty());
        r.register(id("antigravity@dev"));
        assert_eq!(r.find(&Ident::main("antigravity")), Ok(Some(id("antigravity@dev"))), "the model's only one");
        assert_eq!(r.find(&id("antigravity@qa")), Ok(None));
        r.register(id("antigravity@qa"));
        assert_eq!(r.find(&Ident::main("antigravity")).unwrap_err().len(), 2);
    }

    #[test]
    fn models_with_colons_work() {
        let mut r = Instances::default();
        let (a, c) = r.resolve_or_create(&id("outro:qwen3.5:4b@qa"), false).unwrap();
        assert_eq!((a.id().as_str(), c), ("outro:qwen3.5:4b@qa", true));
        assert_eq!(a.kind, "outro:qwen3.5:4b");
    }

    #[test]
    fn rename_instance_works_and_checks_duplicates() {
        let mut r = Instances::default();
        r.register(id("claude@dev"));
        r.register(id("claude@design"));
        assert!(r.rename("claude@inexistente", id("claude@test")).is_err());
        assert!(r.rename("claude@dev", id("claude@design")).is_err());
        assert!(r.rename("claude@dev", id("claude@lead")).is_ok());
        assert_eq!(r.ids(), vec!["claude@lead", "claude@design"]);
    }
}

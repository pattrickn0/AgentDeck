//! Instâncias de agentes: cada aba é uma instância com identidade própria.
//!
//! O id é `<modelo>[@<cargo>[@<nome>]]`:
//!
//! - `claude` — a instância principal (sem cargo), a que o usuário abre pelo "+";
//! - `claude@dev`, `claude@design` — instâncias com cargo, abertas por um agente (ou pelo usuário);
//! - `claude@dev@ana` — quando já existe outra do mesmo modelo **e** cargo, a nova ganha um nome.
//!
//! `claude@main@bia` é a segunda instância principal (sem cargo) do Claude. O modelo pode conter
//! `:` e `/` (`ollama:qwen3.5:4b`, `ollama:user/custom:latest`), por isso o separador é `@`.
//!
//! Este módulo é só lógica de nomes e de resolução de destino; quem cria abas é a interface.

/// Separador entre modelo, cargo e nome.
pub const SEP: char = '@';
/// Cargo implícito de uma instância sem cargo.
pub const MAIN_ROLE: &str = "main";

/// Nomes dados a instâncias repetidas (mesmo modelo e cargo), na ordem.
const NAMES: &[&str] =
    &["ana", "bia", "caio", "davi", "eva", "fabio", "gabi", "hugo", "iris", "joao", "kaua", "lia", "marco", "nina", "otto", "paula"];

/// Identidade de uma instância.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Ident {
    /// Modelo/agente (`claude`, `antigravity`, `ollama:qwen3.5:4b`…).
    pub kind: String,
    /// Cargo (`dev`, `design`…); `None` = instância principal.
    pub role: Option<String>,
    /// Nome, só quando há repetição de modelo + cargo.
    pub name: Option<String>,
}

/// Cargo e nome: letras minúsculas, números, `-` e `_`.
fn valid_word(s: &str) -> bool {
    !s.is_empty() && s.len() <= 32 && s.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

impl Ident {
    /// A instância principal de um modelo.
    pub fn main(kind: &str) -> Self {
        Self { kind: kind.to_owned(), role: None, name: None }
    }

    /// `kind[@cargo[@nome]]`.
    pub fn id(&self) -> String {
        match (&self.role, &self.name) {
            (None, None) => self.kind.clone(),
            (Some(r), None) => format!("{}{SEP}{r}", self.kind),
            (r, Some(n)) => format!("{}{SEP}{}{SEP}{n}", self.kind, r.as_deref().unwrap_or(MAIN_ROLE)),
        }
    }

    /// Lê um id. `None` se vier mal formado (cargo/nome inválidos, partes demais).
    pub fn parse(text: &str) -> Option<Self> {
        let mut parts = text.split(SEP);
        let kind = parts.next().filter(|k| !k.is_empty())?.to_owned();
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

    /// Rótulo legível: `Claude`, `Claude · Dev`, `Claude · Dev · Ana`. `kind_label` é o nome do modelo.
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

/// Instâncias conhecidas (abas abertas e as que estão sendo abertas).
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

    /// Registra uma instância já definida (restauração de abas). Idempotente.
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

    /// Cria e registra uma instância **nova** do modelo/cargo: sem nome se o par modelo+cargo
    /// está livre; senão com o primeiro nome livre. `preferred` é um nome pedido, usado se estiver livre.
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
                None => (2..).map(|i| make(Some(format!("n{i}")))).find(|c| !taken(c)).expect("sempre há um nome livre"),
            }
        };
        self.list.push(ident.clone());
        ident
    }

    /// Resolve o destino de uma pergunta. Devolve a instância e se ela foi **criada** agora (a
    /// interface precisa abrir a aba), ou os ids em conflito quando o destino é ambíguo.
    ///
    /// - id exato existente → ele;
    /// - só o modelo (`claude`): a principal, senão a única do modelo; várias sem principal → ambíguo;
    /// - modelo + cargo: a única com esse cargo; várias → ambíguo;
    /// - nada encontrado (ou `force_new`) → cria.
    pub fn resolve_or_create(&mut self, want: &Ident, force_new: bool) -> Result<(Ident, bool), Vec<String>> {
        if !force_new {
            if let Some(found) = self.get(&want.id()) {
                return Ok((found.clone(), false));
            }
            if want.name.is_none() {
                let same_role: Vec<&Ident> = self.list.iter().filter(|i| i.kind == want.kind && i.role == want.role).collect();
                match same_role.len() {
                    1 => return Ok((same_role[0].clone(), false)),
                    n if n > 1 => return Err(same_role.iter().map(|i| i.id()).collect()),
                    _ => {}
                }
                // Só o modelo: sem principal, serve a única instância dele.
                if want.role.is_none() {
                    let any: Vec<&Ident> = self.list.iter().filter(|i| i.kind == want.kind).collect();
                    match any.len() {
                        1 => return Ok((any[0].clone(), false)),
                        n if n > 1 => return Err(any.iter().map(|i| i.id()).collect()),
                        _ => {}
                    }
                }
            }
        }
        Ok((self.allocate(&want.kind, want.role.as_deref(), want.name.as_deref()), true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(s: &str) -> Ident {
        Ident::parse(s).expect("id válido")
    }

    #[test]
    fn ids_round_trip() {
        for s in [
            "claude",
            "claude@dev",
            "claude@dev@ana",
            "claude@main@bia",
            "ollama:qwen3.5:4b",
            "ollama:qwen3.5:4b@dev",
            "ollama:user/custom:latest@qa",
        ] {
            assert_eq!(id(s).id(), s, "{s}");
        }
        // "main" é o cargo implícito.
        assert_eq!(id("claude@main"), Ident::main("claude"));
        assert_eq!(id("claude@main").id(), "claude");
        assert_eq!(id("claude@main@bia").role, None);
        assert_eq!(id("claude@dev@ana"), Ident { kind: "claude".into(), role: Some("dev".into()), name: Some("ana".into()) });
    }

    #[test]
    fn malformed_ids_are_rejected() {
        for s in ["", "@dev", "claude@", "claude@Dev", "claude@d ev", "claude@dev@ana@x", "claude@dev@", "claude@dev;rm", "claude@@ana"] {
            assert!(Ident::parse(s).is_none(), "{s:?}");
        }
        assert!(Ident::parse(&format!("claude@{}", "a".repeat(33))).is_none());
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
        // Cargo diferente não conta como repetição.
        assert_eq!(r.allocate("claude", Some("dev"), None).id(), "claude@dev");
        assert_eq!(r.allocate("claude", Some("dev"), None).id(), "claude@dev@ana");
        assert_eq!(r.allocate("antigravity", None, None).id(), "antigravity");
        // Nome pedido, se livre; senão o próximo.
        assert_eq!(r.allocate("claude", Some("dev"), Some("zeca")).id(), "claude@dev@zeca");
        assert_eq!(r.allocate("claude", Some("dev"), Some("zeca")).id(), "claude@dev@bia");
        // Fechar libera o nome.
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
        // Nada aberto: cria a principal.
        let (a, created) = r.resolve_or_create(&Ident::main("claude"), false).unwrap();
        assert_eq!((a.id().as_str(), created), ("claude", true));
        // De novo: é a mesma, já existente.
        let (a, created) = r.resolve_or_create(&Ident::main("claude"), false).unwrap();
        assert_eq!((a.id().as_str(), created), ("claude", false));
        // Com outras instâncias, "claude" continua sendo a principal.
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
        // O cargo não existe ainda: cria.
        let (dev, created) = r.resolve_or_create(&id("claude@dev"), false).unwrap();
        assert_eq!((dev.id().as_str(), created), ("claude@dev", true));
        // Pedir o mesmo cargo de novo reaproveita a instância.
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap(), (id("claude@dev"), false));
        // --new força outra: ganha nome.
        let (second, created) = r.resolve_or_create(&id("claude@dev"), true).unwrap();
        assert_eq!((second.id().as_str(), created), ("claude@dev@ana", true));
        // Agora "claude@dev" é o id exato da primeira (sem nome): continua resolvendo nela.
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap().0.id(), "claude@dev");
        // E o nome exato também.
        assert_eq!(r.resolve_or_create(&id("claude@dev@ana"), false).unwrap(), (id("claude@dev@ana"), false));
    }

    #[test]
    fn a_role_with_only_named_instances_is_ambiguous_when_there_are_several() {
        let mut r = Instances::default();
        r.register(id("claude@dev@ana"));
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap(), (id("claude@dev@ana"), false), "uma só: serve");
        r.register(id("claude@dev@bia"));
        assert_eq!(r.resolve_or_create(&id("claude@dev"), false).unwrap_err().len(), 2);
    }

    #[test]
    fn models_with_colons_work() {
        let mut r = Instances::default();
        let (a, c) = r.resolve_or_create(&id("ollama:qwen3.5:4b@qa"), false).unwrap();
        assert_eq!((a.id().as_str(), c), ("ollama:qwen3.5:4b@qa", true));
        assert_eq!(a.kind, "ollama:qwen3.5:4b");
    }
}

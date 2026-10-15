//! Registro de agentes do lado da UI: cores/nomes (COLORS/NAMES do renderer), listas dos
//! menus e opções do seletor "Enviar para" (`buildMenus` / `renderTargetDropdown`).

use std::collections::HashMap;

use commanddeck_core::agents::{COLOR_ANTIGRAVITY, COLOR_CLAUDE, COLOR_OLLAMA, COLOR_SHELL};
use commanddeck_core::{AgentInfo, Ident};
use egui::Color32;

use crate::theme;

/// Nome exibido para o terminal comum (o renderer original sempre usa "PowerShell").
pub fn shell_label(core_label: Option<&str>) -> String {
    if cfg!(windows) {
        "PowerShell".into()
    } else {
        core_label.filter(|l| !l.is_empty()).unwrap_or("Terminal").to_owned()
    }
}

/// Opção do `<select id="target">`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetOption {
    pub label: String,
    pub value: String,
}

/// `<optgroup>` do seletor.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TargetGroup {
    pub label: String,
    pub options: Vec<TargetOption>,
}

fn opt(label: impl Into<String>, value: impl Into<String>) -> TargetOption {
    TargetOption { label: label.into(), value: value.into() }
}

fn is_ollama(id: &str) -> bool {
    id == "ollama" || id.starts_with("ollama:")
}

/// Porta fiel de `renderTargetDropdown(agents, mainAgents, ollamaModels)`.
pub fn build_target_groups(main_agents: &[AgentInfo], ollama_models: &[AgentInfo]) -> Vec<TargetGroup> {
    let mut groups = vec![TargetGroup {
        label: "Destinatários Diretos".into(),
        options: vec![opt("Aba ativa", "active"), opt("Todos os agentes abertos", "all")],
    }];

    let main_peers: Vec<&AgentInfo> = main_agents.iter().filter(|a| a.peer && a.installed).collect();
    let ollama_peers: Vec<&AgentInfo> = ollama_models.iter().filter(|a| a.peer && a.installed).collect();
    let mut tables = Vec::new();

    // 1. Mesas entre agentes principais de nuvem/CLI (ex: Claude ↔ Antigravity)
    for a in &main_peers {
        for b in &main_peers {
            if a.id != b.id {
                tables.push(opt(format!("Mesa: {} → {}", a.label, b.label), format!("table:{},{}", a.id, b.id)));
            }
        }
    }

    // 2. Mesas com Ollama: utiliza o modelo local preferencial (ex: 9b > 4b)
    let preferred = |id: &str| {
        let l = id.to_lowercase();
        ["9b", "7b", "14b", "32b"].iter().any(|s| l.contains(s))
    };
    let primary = ollama_peers.iter().find(|o| preferred(&o.id)).or(ollama_peers.first()).copied();
    if let Some(p) = primary {
        let o_name = p.model.clone().unwrap_or_else(|| "Local".into());
        for m in &main_peers {
            tables.push(opt(format!("Mesa: {} ↔ Ollama ({o_name})", m.label), format!("table:{},{}", m.id, p.id)));
        }
    }

    // 3. Se houver apenas modelos locais sem agentes de nuvem: mesa entre os 2 primeiros modelos
    if main_peers.is_empty() && ollama_peers.len() >= 2 {
        let (o1, o2) = (ollama_peers[0], ollama_peers[1]);
        let m = |o: &AgentInfo| o.model.clone().unwrap_or_else(|| "undefined".into());
        tables.push(opt(format!("Mesa: Ollama ({}) ↔ Ollama ({})", m(o1), m(o2)), format!("table:{},{}", o1.id, o2.id)));
    }

    // 4. Mesa Geral (todos os agentes principais + Ollama)
    let mut all: Vec<&AgentInfo> = main_peers.clone();
    if let Some(p) = primary {
        all.push(p);
    }
    if all.len() > 2 {
        let summary =
            main_peers.iter().map(|p| p.label.as_str()).collect::<Vec<_>>().join(" + ") + if primary.is_some() { " + Ollama" } else { "" };
        let ids = all.iter().map(|p| p.id.as_str()).collect::<Vec<_>>().join(",");
        tables.push(opt(format!("Mesa Geral ({summary})"), format!("table:{ids}")));
    }

    if !tables.is_empty() {
        groups.push(TargetGroup { label: "Mesas de Orquestração".into(), options: tables });
    }
    groups
}

/// Item de menu (nova aba / estado vazio).
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub kind: String,
    pub label: String,
    pub color: Color32,
}

/// Estado derivado da lista de agentes.
#[derive(Debug, Default, Clone)]
pub struct Agents {
    pub loaded: bool,
    colors: HashMap<String, Color32>,
    names: HashMap<String, String>,
    /// `availableAgents`
    pub available: Vec<AgentInfo>,
    /// Itens do menu "+" (`renderNewTabMenu`).
    pub new_tab_menu: Vec<MenuItem>,
    /// Botões do estado vazio (`renderEmptyStateActions`).
    pub empty_actions: Vec<MenuItem>,
    pub targets: Vec<TargetGroup>,
}

impl Agents {
    pub fn new() -> Self {
        Self { targets: build_target_groups(&[], &[]), ..Self::default() }
    }

    /// Porta de `buildMenus(agents)`.
    pub fn build(&mut self, agents: &[AgentInfo]) {
        let color_of = |a: &AgentInfo| theme::parse_hex_color(&a.color).unwrap_or(theme::SHELL);
        for a in agents {
            self.colors.insert(a.id.clone(), color_of(a));
            let name = if a.id == "shell" { shell_label(Some(&a.label)) } else { a.label.clone() };
            self.names.insert(a.id.clone(), name);
            if a.id.starts_with("ollama:") {
                self.colors.entry("ollama".into()).or_insert(color_of(a));
                self.names.entry("ollama".into()).or_insert_with(|| "Ollama".into());
            }
        }
        self.colors.entry("shell".into()).or_insert(theme::SHELL);
        let shell_core = agents.iter().find(|a| a.id == "shell");
        self.names.insert("shell".into(), shell_label(shell_core.map(|a| a.label.as_str())));

        self.available =
            agents.iter().filter(|a| a.installed || a.id == "shell" || a.id == "claude" || a.id == "antigravity").cloned().collect();
        let main: Vec<AgentInfo> = self.available.iter().filter(|a| !is_ollama(&a.id)).cloned().collect();
        let ollama: Vec<AgentInfo> = self.available.iter().filter(|a| is_ollama(&a.id)).cloned().collect();

        let ai: Vec<&AgentInfo> =
            self.available.iter().filter(|a| a.id != "shell" && (a.installed || a.id == "claude" || a.id == "antigravity")).collect();
        let shell_color = shell_core.map(color_of).unwrap_or(theme::SHELL);
        let shell_name = shell_label(shell_core.map(|a| a.label.as_str()));

        self.new_tab_menu = ai
            .iter()
            .map(|a| MenuItem {
                kind: a.id.clone(),
                label: format!("{}{}", a.label, if a.installed { "" } else { " (não instalado)" }),
                color: color_of(a),
            })
            .chain(std::iter::once(MenuItem { kind: "shell".into(), label: shell_name.clone(), color: shell_color }))
            .collect();
        self.empty_actions = ai
            .iter()
            .map(|a| MenuItem { kind: a.id.clone(), label: format!("Abrir {}", a.label), color: color_of(a) })
            .chain(std::iter::once(MenuItem { kind: "shell".into(), label: format!("Abrir {shell_name}"), color: shell_color }))
            .collect();
        self.targets = build_target_groups(&main, &ollama);
        self.loaded = true;
    }

    /// `COLORS[kind]` (com os fallbacks de `openTab` para Ollama).
    pub fn color(&self, kind: &str) -> Option<Color32> {
        self.colors.get(kind).copied().or_else(|| is_ollama(kind).then_some(theme::OLLAMA))
    }

    /// Cor da aba: `COLORS[kind] || '#9aa0a6'`, com as cores oficiais como fallback quando o
    /// backend ainda não listou o agente.
    pub fn tab_color(&self, kind: &str) -> Color32 {
        self.color(kind).unwrap_or_else(|| {
            let hex = match kind {
                "claude" => COLOR_CLAUDE,
                "antigravity" => COLOR_ANTIGRAVITY,
                k if is_ollama(k) => COLOR_OLLAMA,
                _ => COLOR_SHELL,
            };
            theme::parse_hex_color(hex).unwrap_or(theme::SHELL)
        })
    }

    /// `NAMES[kind] || kind` (com rótulos oficiais como fallback).
    pub fn name(&self, kind: &str) -> String {
        if let Some(n) = self.names.get(kind) {
            return n.clone();
        }
        if let Some(model) = kind.strip_prefix("ollama:") {
            return format!("Ollama ({model})");
        }
        match kind {
            "claude" => "Claude".into(),
            "antigravity" => "Antigravity".into(),
            "ollama" => "Ollama".into(),
            "shell" => shell_label(None),
            other => other.to_owned(),
        }
    }

    /// `whoColor(name)`; uma instância (`claude@dev`) usa a cor do modelo.
    pub fn who_color(&self, name: Option<&str>) -> Color32 {
        if name == Some("você") {
            return theme::USER;
        }
        name.and_then(|n| self.colors.get(n).copied().or_else(|| Ident::parse(n).and_then(|i| self.colors.get(&i.kind).copied())))
            .unwrap_or(theme::WHO_DEFAULT)
    }

    /// `whoName(name)`; uma instância vira `Claude · Dev · Ana`.
    pub fn who_name(&self, name: &str) -> String {
        if let Some(n) = self.names.get(name) {
            return n.clone();
        }
        match Ident::parse(name) {
            Some(i) if i.role.is_some() || i.name.is_some() => i.label(&self.who_name(&i.kind)),
            _ => name.to_owned(),
        }
    }

    /// Restauração só abre abas de tipos conhecidos (`if (COLORS[kind])`).
    pub fn knows(&self, kind: &str) -> bool {
        self.colors.contains_key(kind)
    }

    /// Kind padrão do Ctrl+T sem aba ativa.
    pub fn fallback_kind(&self) -> String {
        self.available.iter().find(|a| a.id != "shell").or(self.available.first()).map(|a| a.id.clone()).unwrap_or_else(|| "shell".into())
    }

    pub fn has_option(&self, value: &str) -> bool {
        self.targets.iter().any(|g| g.options.iter().any(|o| o.value == value))
    }

    pub fn option_label(&self, value: &str) -> Option<&str> {
        self.targets.iter().flat_map(|g| g.options.iter()).find(|o| o.value == value).map(|o| o.label.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, label: &str, installed: bool, peer: bool, model: Option<&str>) -> AgentInfo {
        AgentInfo { id: id.into(), label: label.into(), color: "#d97757".into(), installed, peer, model: model.map(Into::into) }
    }

    fn values(groups: &[TargetGroup]) -> Vec<(String, String)> {
        groups.iter().flat_map(|g| g.options.iter().map(|o| (o.label.clone(), o.value.clone()))).collect()
    }

    #[test]
    fn direct_group_always_present() {
        let g = build_target_groups(&[], &[]);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].label, "Destinatários Diretos");
        assert_eq!(values(&g), vec![("Aba ativa".into(), "active".into()), ("Todos os agentes abertos".into(), "all".into())]);
    }

    #[test]
    fn tables_between_main_agents_and_preferred_ollama() {
        let main = vec![
            agent("claude", "Claude", true, true, None),
            agent("antigravity", "Antigravity", true, true, None),
            agent("shell", "Terminal", true, false, None),
        ];
        let ollama = vec![
            agent("ollama:qwen3.5:4b", "Ollama (qwen3.5:4b)", true, true, Some("qwen3.5:4b")),
            agent("ollama:qwen3.5:9b", "Ollama (qwen3.5:9b)", true, true, Some("qwen3.5:9b")),
        ];
        let g = build_target_groups(&main, &ollama);
        assert_eq!(g.len(), 2);
        assert_eq!(g[1].label, "Mesas de Orquestração");
        let v: Vec<(String, String)> = g[1].options.iter().map(|o| (o.label.clone(), o.value.clone())).collect();
        assert_eq!(
            v,
            vec![
                ("Mesa: Claude → Antigravity".into(), "table:claude,antigravity".into()),
                ("Mesa: Antigravity → Claude".into(), "table:antigravity,claude".into()),
                ("Mesa: Claude ↔ Ollama (qwen3.5:9b)".into(), "table:claude,ollama:qwen3.5:9b".into()),
                ("Mesa: Antigravity ↔ Ollama (qwen3.5:9b)".into(), "table:antigravity,ollama:qwen3.5:9b".into()),
                ("Mesa Geral (Claude + Antigravity + Ollama)".into(), "table:claude,antigravity,ollama:qwen3.5:9b".into()),
            ]
        );
    }

    #[test]
    fn only_local_models() {
        let ollama = vec![agent("ollama:a", "Ollama (a)", true, true, Some("a")), agent("ollama:b", "Ollama (b)", true, true, Some("b"))];
        let g = build_target_groups(&[], &ollama);
        assert_eq!(values(&g)[2], ("Mesa: Ollama (a) ↔ Ollama (b)".into(), "table:ollama:a,ollama:b".into()));
        assert_eq!(values(&g).len(), 3);
    }

    #[test]
    fn uninstalled_or_non_peer_are_ignored() {
        let main = vec![agent("claude", "Claude", false, true, None), agent("antigravity", "Antigravity", true, true, None)];
        let g = build_target_groups(&main, &[]);
        assert_eq!(g.len(), 1);
    }

    #[test]
    fn build_menus_and_names() {
        let mut a = Agents::new();
        a.build(&[
            agent("claude", "Claude", false, true, None),
            agent("codex", "Codex", false, true, None),
            agent("ollama:qwen3.5:9b", "Ollama (qwen3.5:9b)", true, true, Some("qwen3.5:9b")),
            agent("shell", "Terminal", true, false, None),
        ]);
        let kinds: Vec<&str> = a.new_tab_menu.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(kinds, vec!["claude", "ollama:qwen3.5:9b", "shell"]);
        assert_eq!(a.new_tab_menu[0].label, "Claude (não instalado)");
        assert_eq!(a.empty_actions[0].label, "Abrir Claude");
        assert!(a.knows("ollama"));
        assert_eq!(a.name("ollama"), "Ollama");
        assert_eq!(a.name("desconhecido"), "desconhecido");
        assert_eq!(a.fallback_kind(), "claude");
        assert_eq!(a.who_color(Some("você")), theme::USER);
        assert_eq!(a.who_color(Some("desconhecido")), theme::WHO_DEFAULT);
    }
}

//! Registro de agentes do lado da UI: cores/nomes (COLORS/NAMES do renderer) e listas dos
//! menus (`buildMenus`).

use std::collections::HashMap;

use agentdeck_core::agents::{COLOR_ANTIGRAVITY, COLOR_CLAUDE, COLOR_OLLAMA, COLOR_SHELL};
use agentdeck_core::{AgentInfo, Ident};
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

fn is_ollama(id: &str) -> bool {
    id == "ollama" || id.starts_with("ollama:")
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
}

impl Agents {
    pub fn new() -> Self {
        Self::default()
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
            return theme::pal().success;
        }
        name.and_then(|n| self.colors.get(n).copied().or_else(|| Ident::parse(n).and_then(|i| self.colors.get(&i.kind).copied())))
            .unwrap_or(theme::pal().text_2)
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
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, label: &str, installed: bool, peer: bool, model: Option<&str>) -> AgentInfo {
        AgentInfo { id: id.into(), label: label.into(), color: "#d97757".into(), installed, peer, model: model.map(Into::into) }
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
        assert_eq!(a.who_color(Some("você")), theme::pal().success);
        assert_eq!(a.who_color(Some("desconhecido")), theme::pal().text_2);
    }
}

//! UI-side agent registry: colors/names (the renderer's COLORS/NAMES) and the menu lists
//! (`buildMenus`).

use std::collections::HashMap;

use agentdeck_core::agents::{COLOR_ANTIGRAVITY, COLOR_CLAUDE, COLOR_SHELL};
use agentdeck_core::{t, AgentInfo, Ident};
use egui::Color32;

use crate::theme;

/// Label shown for the plain terminal (the original renderer always uses "PowerShell").
pub fn shell_label(core_label: Option<&str>) -> String {
    if cfg!(windows) {
        "PowerShell".into()
    } else {
        core_label.filter(|l| !l.is_empty()).unwrap_or("Terminal").to_owned()
    }
}

/// Menu item (new tab / empty state).
#[derive(Debug, Clone, PartialEq)]
pub struct MenuItem {
    pub kind: String,
    pub label: String,
    pub color: Color32,
}

/// State derived from the agent list.
#[derive(Debug, Default, Clone)]
pub struct Agents {
    pub loaded: bool,
    colors: HashMap<String, Color32>,
    names: HashMap<String, String>,
    /// `availableAgents`
    pub available: Vec<AgentInfo>,
    /// "+" menu items (`renderNewTabMenu`).
    pub new_tab_menu: Vec<MenuItem>,
    /// Empty-state buttons (`renderEmptyStateActions`).
    pub empty_actions: Vec<MenuItem>,
}

impl Agents {
    pub fn new() -> Self {
        Self::default()
    }

    /// Port of `buildMenus(agents)`.
    pub fn build(&mut self, agents: &[AgentInfo]) {
        let color_of = |a: &AgentInfo| theme::parse_hex_color(&a.color).unwrap_or(theme::SHELL);
        for a in agents {
            self.colors.insert(a.id.clone(), color_of(a));
            let name = if a.id == "shell" { shell_label(Some(&a.label)) } else { a.label.clone() };
            self.names.insert(a.id.clone(), name);
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
                label: if a.installed { a.label.clone() } else { t!("{agent} (not installed)", agent = a.label) },
                color: color_of(a),
            })
            .chain(std::iter::once(MenuItem { kind: "shell".into(), label: shell_name.clone(), color: shell_color }))
            .collect();
        self.empty_actions = ai
            .iter()
            .map(|a| MenuItem { kind: a.id.clone(), label: t!("Open {agent}", agent = a.label), color: color_of(a) })
            .chain(std::iter::once(MenuItem { kind: "shell".into(), label: t!("Open {agent}", agent = shell_name), color: shell_color }))
            .collect();
        self.loaded = true;
    }

    /// `COLORS[kind]`.
    pub fn color(&self, kind: &str) -> Option<Color32> {
        self.colors.get(kind).copied()
    }

    /// Tab color: `COLORS[kind] || '#9aa0a6'`, with the official colors as a fallback when the
    /// backend has not listed the agent yet.
    pub fn tab_color(&self, kind: &str) -> Color32 {
        self.color(kind).unwrap_or_else(|| {
            let hex = match kind {
                "claude" => COLOR_CLAUDE,
                "antigravity" => COLOR_ANTIGRAVITY,
                _ => COLOR_SHELL,
            };
            theme::parse_hex_color(hex).unwrap_or(theme::SHELL)
        })
    }

    /// `NAMES[kind] || kind` (with the official labels as a fallback).
    pub fn name(&self, kind: &str) -> String {
        if let Some(n) = self.names.get(kind) {
            return n.clone();
        }
        match kind {
            "claude" => "Claude".into(),
            "antigravity" => "Antigravity".into(),
            "shell" => shell_label(None),
            other => other.to_owned(),
        }
    }

    /// `whoColor(name)`; an instance (`claude@dev`) uses the model's color.
    pub fn who_color(&self, name: Option<&str>) -> Color32 {
        // "você" is the user's name in chat history saved before the English rename.
        if name == Some("you") || name == Some("você") {
            return theme::pal().user;
        }
        name.and_then(|n| self.colors.get(n).copied().or_else(|| Ident::parse(n).and_then(|i| self.colors.get(&i.kind).copied())))
            .unwrap_or(theme::pal().text_2)
    }

    /// `whoName(name)`; an instance becomes `Claude · Dev · Ana`.
    pub fn who_name(&self, name: &str) -> String {
        if let Some(n) = self.names.get(name) {
            return n.clone();
        }
        match Ident::parse(name) {
            Some(i) if i.role.is_some() || i.name.is_some() => i.label(&self.who_name(&i.kind)),
            _ => name.to_owned(),
        }
    }

    /// Restoration only opens tabs of known kinds (`if (COLORS[kind])`).
    pub fn knows(&self, kind: &str) -> bool {
        self.colors.contains_key(kind)
    }

    /// Default kind for Ctrl+T with no active tab.
    pub fn fallback_kind(&self) -> String {
        self.available.iter().find(|a| a.id != "shell").or(self.available.first()).map(|a| a.id.clone()).unwrap_or_else(|| "shell".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agent(id: &str, label: &str, installed: bool, peer: bool) -> AgentInfo {
        AgentInfo { id: id.into(), label: label.into(), color: "#d97757".into(), installed, peer }
    }

    #[test]
    fn build_menus_and_names() {
        let mut a = Agents::new();
        a.build(&[
            agent("claude", "Claude", false, true),
            agent("codex", "Codex", false, true),
            agent("opencode", "OpenCode", true, true),
            agent("shell", "Terminal", true, false),
        ]);
        let kinds: Vec<&str> = a.new_tab_menu.iter().map(|m| m.kind.as_str()).collect();
        assert_eq!(kinds, vec!["claude", "opencode", "shell"]);
        assert_eq!(a.new_tab_menu[0].label, "Claude (not installed)");
        assert_eq!(a.empty_actions[0].label, "Open Claude");
        assert!(a.knows("opencode") && !a.knows("ollama"));
        assert_eq!(a.name("opencode"), "OpenCode");
        assert_eq!(a.name("unknown"), "unknown");
        assert_eq!(a.fallback_kind(), "claude");
        assert_eq!(a.who_color(Some("you")), theme::pal().user);
        assert_eq!(a.who_color(Some("unknown")), theme::pal().text_2);
    }
}

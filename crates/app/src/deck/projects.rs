//! Projects (sidebar API).

use super::*;

impl Deck {
    /// Projects for the sidebar, in the saved order, with each one's agents (for a project
    /// not yet opened in this session, the saved ones, with no tab).
    pub fn project_views(&self) -> Vec<ProjectView> {
        if let Some(demo) = &self.debug_projects {
            return demo.clone();
        }
        self.core
            .projects()
            .into_iter()
            .map(|p| {
                let loaded = self.projects.get(&p.id).is_some_and(|s| s.loaded);
                let agents = if loaded {
                    (0..self.tabs.len())
                        .filter(|&i| self.tabs[i].project == p.id)
                        .map(|i| {
                            let t = &self.tabs[i];
                            AgentBadge {
                                id: t.ident.id(),
                                parent: p.tab_parents.get(&t.ident.id()).cloned(),
                                kind: t.kind.clone(),
                                label: t.label.clone(),
                                model: p.tab_models.get(&t.ident.id()).cloned(),
                                color: t.color,
                                busy: t.busy,
                                done: t.done,
                                alive: t.alive,
                                tab: Some(i),
                            }
                        })
                        .collect()
                } else {
                    p.open_tabs
                        .iter()
                        .filter_map(|id| Ident::parse(id))
                        .map(|ident| AgentBadge {
                            id: ident.id(),
                            parent: p.tab_parents.get(&ident.id()).cloned(),
                            label: ident.label(&self.agents.name(&ident.kind)),
                            model: p.tab_models.get(&ident.id()).cloned(),
                            color: self.agents.tab_color(&ident.kind),
                            kind: ident.kind,
                            busy: false,
                            done: false,
                            alive: false,
                            tab: None,
                        })
                        .collect()
                };
                ProjectView { active: p.id == self.active_project, expanded: !p.collapsed, id: p.id, name: p.name, path: p.path, agents }
            })
            .collect()
    }

    /// Shows or hides the agent list of project `id` in the sidebar (saved in the config).
    pub fn toggle_project_expanded(&mut self, id: &str) {
        self.core.update_project(id, |p| p.collapsed = !p.collapsed);
    }

    /// Shows (`true`) or hides the agent list of every project.
    pub fn set_all_projects_expanded(&mut self, expanded: bool) {
        for p in self.core.projects() {
            self.core.update_project(&p.id, |p| p.collapsed = !expanded);
        }
    }

    /// Agent dragged in the sidebar: from now on it answers to `boss` (`None` = the user). The core
    /// refuses cycles; the team notice goes with the next message of the affected agents.
    pub fn set_agent_boss(&mut self, project: &str, id: &str, boss: Option<&str>) {
        if let Err(e) = self.core.set_boss(project, id, boss) {
            self.system(t!("Could not change the team: {error}.", error = e));
        }
    }

    /// Click on an agent in the sidebar: brings its project to the screen (opening the saved tabs,
    /// if they have not opened yet) and activates the tab.
    pub fn focus_agent(&mut self, project: &str, id: &str) {
        self.switch_project(project);
        self.show_agents();
        if let Some(i) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == id) {
            self.activate(i);
        }
    }

    /// "+" of the PROJECTS section: chooses the new project's folder.
    pub fn new_project_dialog(&self) {
        self.pick_workspace();
    }

    /// Gives the project another name (does not change the folder).
    pub fn rename_project(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            self.core.rename_project(id, name);
        }
    }

    /// Takes the project off the list and closes its tabs (the folder stays). If it was the one on
    /// screen, the core activates the first one left (or none).
    pub fn remove_project_by_id(&mut self, id: &str) {
        while let Some(i) = self.tabs.iter().position(|t| t.project == id) {
            self.close_tab(i);
        }
        self.projects.remove(id);
        self.docs.drop_project(id);
        let was_active = id == self.active_project;
        self.core.remove_project(id);
        if was_active {
            self.active_project.clear();
            self.active = None;
            self.group_order.clear();
            self.split_ratios = split::Ratios::default();
            self.feed.clear();
            let next = self.core.active_project_id();
            if next.is_empty() {
                self.after_project_change();
            } else {
                self.switch_project(&next);
            }
        }
    }
}

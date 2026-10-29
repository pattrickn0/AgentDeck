//! Sidebar and file tree.

use super::*;

impl Deck {
    pub fn toggle_sidebar(&mut self) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        self.persist_layout();
    }

    pub fn refresh_tree(&mut self) {
        self.workspace = self.core.workspace();
        self.tree.cache.clear();
    }

    /// Visible rows of the tree (loads expanded folders on demand).
    pub fn tree_rows(&mut self) -> Vec<TreeRow> {
        let mut rows = Vec::new();
        if self.tree.expanded.contains("") {
            self.collect_rows("", 1, &mut rows);
        }
        rows
    }

    fn collect_rows(&mut self, rel: &str, depth: usize, out: &mut Vec<TreeRow>) {
        if !self.tree.cache.contains_key(rel) {
            let items = self.core.read_dir(rel);
            self.tree.cache.insert(rel.to_owned(), items);
        }
        let items = self.tree.cache.get(rel).cloned().unwrap_or_default();
        for e in items {
            let expanded = e.is_dir && self.tree.expanded.contains(&e.rel_path);
            let rel_path = e.rel_path.clone();
            out.push(TreeRow { entry: e, depth, expanded });
            if expanded {
                self.collect_rows(&rel_path, depth + 1, out);
            }
        }
    }

    pub fn toggle_dir(&mut self, rel: &str) {
        if !self.tree.expanded.remove(rel) {
            self.tree.expanded.insert(rel.to_owned());
            // Reloads when expanding (like `loadDirItems` on every click).
            self.tree.cache.remove(rel);
        }
    }

    pub fn tree_file_click(&mut self, rel: &str) {
        self.tree.pending_click = Some((rel.to_owned(), Instant::now()));
    }

    /// Double click in the explorer: opens the file in Documents mode (text or markdown, read only).
    pub fn tree_file_double_click(&mut self, rel: &str) {
        self.tree.pending_click = None;
        self.docs_open_file(rel);
    }

    pub fn pick_workspace(&self) {
        let start = if self.workspace.as_os_str().is_empty() { dirs_home() } else { self.workspace.clone() };
        self.spawn_bg("pick-workspace", move |_| {
            let dir = rfd::FileDialog::new().set_directory(&start).pick_folder()?;
            Some(UiMsg::WorkspacePicked(dir))
        });
    }

    /// Asks (on a thread, without freezing the frame) whether the project leaves the list. Removing closes its documents
    /// without saving, so the question counts the ones with unsaved edits.
    pub fn confirm_remove_project(&self, id: String, name: String) {
        let unsaved = self.docs.unsaved().iter().filter(|t| t.project == id).count();
        let lost = match unsaved {
            0 => String::new(),
            1 => t!("\n\nOne of its documents has unsaved edits, which will be lost.").to_owned(),
            n => t!("\n\n{count} of its documents have unsaved edits, which will be lost.", count = n),
        };
        self.spawn_bg("confirm-remove-project", move |_| {
            let ok = rfd::MessageDialog::new()
                .set_title(t!("Remove project"))
                .set_description(t!("Do you want to remove \"{name}\" from the project list?{lost}", name = name, lost = lost))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes;
            ok.then_some(UiMsg::RemoveProject(id))
        });
    }

    /// A folder chosen in the "+" of the PROJECTS section: becomes a project (or goes back to the one it already is).
    pub fn add_project(&mut self, path: PathBuf) {
        let project = self.core.create_project(&path, None);
        self.switch_project(&project.id);
    }

    /// Switches the project on screen. The tabs of the one leaving keep running, hidden; the ones of the one
    /// entering appear (the first time, the saved ones are opened).
    pub fn switch_project(&mut self, id: &str) {
        if id == self.active_project {
            return;
        }
        // Saves the tabs and the layout of the project that is leaving.
        self.persist_tabs_now();
        self.persist_layout_now();
        if let Err(e) = self.core.set_active_project(id) {
            self.system(t!("Could not open the project: {error}", error = e));
            return;
        }
        let old = std::mem::replace(&mut self.active_project, id.to_owned());
        // Menus and modals point to tabs of the project that left.
        self.rename_tab = None;
        self.tab_context_menu = None;
        self.new_tab_menu_open = false;
        self.new_tab_models_for = None;
        self.new_tab_provider = None;
        self.pane_drag = None;
        if !old.is_empty() {
            let uid = self.active.and_then(|i| self.tabs.get(i)).map(|t| t.uid);
            let state = self.projects.entry(old).or_default();
            state.active_uid = uid;
            state.group_order = std::mem::take(&mut self.group_order);
            state.split_ratios = std::mem::take(&mut self.split_ratios);
        }
        self.active = None;
        match self.projects.get_mut(id) {
            Some(state) if state.loaded => {
                self.group_order = std::mem::take(&mut state.group_order);
                self.split_ratios = std::mem::take(&mut state.split_ratios);
                let uid = state.active_uid;
                self.sync_groups();
                self.show_project_tab(uid);
            }
            _ => self.load_project(id),
        }
        self.after_project_change();
    }

    /// The project on screen changed: folder, tree (from scratch), git and github.
    pub(super) fn after_project_change(&mut self) {
        self.workspace = self.core.workspace();
        self.tree = TreeState { expanded: HashSet::from([String::new()]), ..Default::default() };
        self.refresh_tree();
        self.git_reset_for_project();
        self.git_refresh();
        self.tasks_reset_for_project();
        self.tasks_refresh();
        self.github_reset_for_project();
        // Documents belong to the project: the center only stays in Documents if the new project has any.
        self.docs.sync(&self.active_project);
        self.docs_reload_files();
        self.persist_tabs();
    }
}

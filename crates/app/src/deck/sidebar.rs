//! Barra lateral e árvore de arquivos.

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

    /// Linhas visíveis da árvore (carrega pastas expandidas sob demanda).
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
            // Recarrega ao expandir (como `loadDirItems` a cada clique).
            self.tree.cache.remove(rel);
        }
    }

    pub fn tree_file_click(&mut self, rel: &str) {
        self.tree.pending_click = Some((rel.to_owned(), Instant::now()));
    }

    /// Duplo clique no explorador: abre o arquivo no modo Documentos (texto ou markdown, só leitura).
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

    /// Pergunta (numa thread, sem congelar o quadro) se o projeto sai da lista. Remover fecha os documentos dele sem
    /// gravar, então a pergunta conta os que têm edição não salva.
    pub fn confirm_remove_project(&self, id: String, name: String) {
        let unsaved = self.docs.unsaved().iter().filter(|t| t.project == id).count();
        let lost = match unsaved {
            0 => String::new(),
            1 => "\n\nUm documento dele tem edição não salva, que será perdida.".to_owned(),
            n => format!("\n\n{n} documentos dele têm edição não salva, que será perdida."),
        };
        self.spawn_bg("confirm-remove-project", move |_| {
            let ok = rfd::MessageDialog::new()
                .set_title("Remover projeto")
                .set_description(format!("Deseja remover \"{name}\" da lista de projetos?{lost}"))
                .set_buttons(rfd::MessageButtons::YesNo)
                .show()
                == rfd::MessageDialogResult::Yes;
            ok.then_some(UiMsg::RemoveProject(id))
        });
    }

    /// Uma pasta escolhida no "+" da seção PROJETOS: vira projeto (ou volta ao que já é dela).
    pub fn add_project(&mut self, path: PathBuf) {
        let project = self.core.create_project(&path, None);
        self.switch_project(&project.id);
    }

    /// Troca o projeto da tela. As abas do que sai continuam rodando, escondidas; as do que entra
    /// aparecem (na primeira vez, as salvas são abertas).
    pub fn switch_project(&mut self, id: &str) {
        if id == self.active_project {
            return;
        }
        // Grava as abas e o layout do projeto que está saindo.
        self.persist_tabs_now();
        self.persist_layout_now();
        if let Err(e) = self.core.set_active_project(id) {
            self.system(format!("Não foi possível abrir o projeto: {e}"));
            return;
        }
        let old = std::mem::replace(&mut self.active_project, id.to_owned());
        // Menus e modais apontam para abas do projeto que saiu.
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
            state.feed = std::mem::take(&mut self.feed);
        }
        self.active = None;
        self.unseen = 0;
        self.feed_scroll_to_bottom = true;
        match self.projects.get_mut(id) {
            Some(state) if state.loaded => {
                self.group_order = std::mem::take(&mut state.group_order);
                self.split_ratios = std::mem::take(&mut state.split_ratios);
                self.feed = std::mem::take(&mut state.feed);
                let uid = state.active_uid;
                self.sync_groups();
                self.show_project_tab(uid);
            }
            Some(state) => {
                self.feed = std::mem::take(&mut state.feed);
                self.load_project(id);
            }
            None => self.load_project(id),
        }
        self.after_project_change();
    }

    /// O projeto da tela mudou: pasta, árvore (do zero), git e github.
    pub(super) fn after_project_change(&mut self) {
        self.workspace = self.core.workspace();
        self.tree = TreeState { expanded: HashSet::from([String::new()]), ..Default::default() };
        self.refresh_tree();
        self.git_reset_for_project();
        self.git_refresh();
        self.tasks_reset_for_project();
        self.tasks_refresh();
        self.github_reset_for_project();
        // Os documentos são do projeto: o centro só fica em Documentos se o projeto novo tem algum.
        self.docs.sync(&self.active_project);
        self.docs_reload_files();
        self.persist_tabs();
    }
}

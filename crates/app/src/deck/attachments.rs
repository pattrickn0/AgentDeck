//! Anexos, arrastar e soltar e ditado por voz.

use super::*;

impl Deck {
    /// Abre o seletor de arquivos para anexar à aba `uid` (botão de anexo no cabeçalho do painel).
    pub fn pick_tab_attachments(&self, uid: u64) {
        self.pick_attachments_for(AttachTarget::Tab(uid));
    }

    /// Abre o seletor de arquivos para `target` (aba ou formulário "Nova tarefa").
    pub fn pick_attachments_for(&self, target: AttachTarget) {
        self.spawn_bg("pick-attachments", move |core| {
            let files = rfd::FileDialog::new().pick_files()?;
            Some(UiMsg::Attached(target, attachment_paths(&core, core.attach_files(&files))))
        });
    }

    /// Apaga uma cópia em `dados/anexos` (chamado ao remover chip de anexo no formulário de tarefa).
    pub fn delete_attachment_copy(&self, path: PathBuf) {
        if !in_attachments_dir(&self.core.paths().data_dir, &path) {
            return;
        }
        self.spawn_bg("remove-attachment", move |_| {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("apagar anexo {}: {e}", path.display());
            }
            None
        });
    }

    /// Decide qual painel de agente (ou o formulário "Nova tarefa") recebe arquivos arrastados
    /// na posição `pointer`. Funciona em tela dividida, com um único painel e sobre o formulário.
    pub fn resolve_drop_zone(&self, pointer: Option<egui::Pos2>, fallback_rect: egui::Rect) -> Option<DropZone> {
        let task_form_open = self.right_view == RightView::Tarefas && matches!(self.tasks.view, crate::tasks_ui::TasksView::New);
        let over_right = self.right_panel_rect.is_some_and(|r| pointer.is_some_and(|p| r.contains(p)));
        if self.debug_drop_task_form || (task_form_open && (over_right || self.panes.is_empty())) {
            let rect = self.right_panel_rect.unwrap_or(fallback_rect);
            return Some(DropZone {
                rect,
                target: AttachTarget::TaskForm(self.active_project.clone()),
                title: "Solte para anexar à nova tarefa".to_owned(),
                subtitle: "Cópia em dados/anexos citada no pedido".to_owned(),
            });
        }
        let slot = if let Some(s) = self.debug_drop_slot {
            self.panes.get(s)
        } else if let Some(p) = pointer {
            self.panes
                .iter()
                .find(|s| s.rect.contains(p))
                .or_else(|| self.focused_slot().and_then(|s| self.panes.get(s)).or_else(|| self.panes.first()))
        } else {
            self.focused_slot().and_then(|s| self.panes.get(s)).or_else(|| self.panes.first())
        }?;
        let tab = self.tabs.iter().find(|t| t.uid == slot.shown_uid)?;
        Some(DropZone {
            rect: slot.rect,
            target: AttachTarget::Tab(slot.shown_uid),
            title: format!("Solte para anexar em {}", tab.label),
            subtitle: "O caminho será inserido no prompt desta aba".to_owned(),
        })
    }

    /// Arquivos soltos na janela (`WindowEvent::DroppedFile`): envia ao painel sob o ponteiro
    /// (na aba visível dele) ou ao formulário "Nova tarefa".
    pub fn attach_dropped_files(&mut self, paths: Vec<PathBuf>, pointer: Option<egui::Pos2>) {
        if paths.is_empty() {
            return;
        }
        let target = self
            .resolve_drop_zone(pointer, egui::Rect::NOTHING)
            .map(|z| z.target)
            .or_else(|| self.last_drop_target.take())
            .or_else(|| self.active_tab().map(|t| AttachTarget::Tab(t.uid)));
        let Some(target) = target else {
            self.system("Abra um agente ou o formulário de nova tarefa para anexar arquivos.");
            return;
        };
        self.spawn_bg("attach-files", move |core| Some(UiMsg::Attached(target, attachment_paths(&core, core.attach_files(&paths)))));
    }

    /// Clique no microfone do cabeçalho de um painel (pelo `uid` da aba visível nele).
    pub fn toggle_tab_mic(&mut self, uid: u64) {
        self.toggle_mic_for(MicTarget::Tab(uid));
    }

    /// Atalho `Ctrl+Shift+M`: se já há uma aba abrindo/gravando, para/cancela nela (mesmo que o
    /// usuário tenha trocado de aba); se está livre, inicia na aba ativa.
    pub fn toggle_active_tab_mic(&mut self) {
        match self.mic_phase {
            MicPhase::Opening(MicTarget::Tab(uid)) | MicPhase::Recording(MicTarget::Tab(uid)) => {
                self.toggle_mic_for(MicTarget::Tab(uid));
            }
            MicPhase::Idle => {
                if let Some(tab) = self.active_tab() {
                    if !tab.alive {
                        self.system("A aba ativa não tem um processo rodando.");
                        return;
                    }
                    let uid = tab.uid;
                    self.toggle_mic_for(MicTarget::Tab(uid));
                }
            }
            _ => {}
        }
    }
}

/// O arquivo é uma cópia nossa em `dados/anexos` (só essas o "×" apaga).
fn in_attachments_dir(data_dir: &Path, path: &Path) -> bool {
    path.parent().is_some_and(|p| p == data_dir.join("anexos"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_copies_in_the_attachments_folder_are_deleted() {
        let data = Path::new("/dados");
        assert!(in_attachments_dir(data, &data.join("anexos").join("s_x_1-foto.png")));
        assert!(!in_attachments_dir(data, &data.join("anexos").join("sub").join("a.png")));
        assert!(!in_attachments_dir(data, Path::new("/home/ana/foto.png")));
    }
}

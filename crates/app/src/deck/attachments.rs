//! Attachments, drag and drop and voice dictation.

use super::*;

impl Deck {
    /// Opens the file picker to attach to the tab `uid` (attach button in the panel header).
    pub fn pick_tab_attachments(&self, uid: u64) {
        self.pick_attachments_for(AttachTarget::Tab(uid));
    }

    /// Opens the file picker for `target` (a tab or the "New task" form).
    pub fn pick_attachments_for(&self, target: AttachTarget) {
        self.spawn_bg("pick-attachments", move |core| {
            let files = rfd::FileDialog::new().pick_files()?;
            Some(UiMsg::Attached(target, attachment_paths(&core, core.attach_files(&files))))
        });
    }

    /// Deletes a copy in `dados/anexos` (called when removing an attachment chip in the task form).
    pub fn delete_attachment_copy(&self, path: PathBuf) {
        if !in_attachments_dir(&self.core.paths().data_dir, &path) {
            return;
        }
        self.spawn_bg("remove-attachment", move |_| {
            if let Err(e) = std::fs::remove_file(&path) {
                log::warn!("delete attachment {}: {e}", path.display());
            }
            None
        });
    }

    /// Decides which agent panel (or the "New task" form) receives files dropped
    /// at `pointer`. Works in split view, with a single panel and over the form.
    pub fn resolve_drop_zone(&self, pointer: Option<egui::Pos2>, fallback_rect: egui::Rect) -> Option<DropZone> {
        let task_form_open = self.right_view == RightView::Tarefas && matches!(self.tasks.view, crate::tasks_ui::TasksView::New);
        let over_right = self.right_panel_rect.is_some_and(|r| pointer.is_some_and(|p| r.contains(p)));
        if self.debug_drop_task_form || (task_form_open && (over_right || self.panes.is_empty())) {
            let rect = self.right_panel_rect.unwrap_or(fallback_rect);
            return Some(DropZone {
                rect,
                target: AttachTarget::TaskForm(self.active_project.clone()),
                title: t!("Drop to attach to the new task").to_owned(),
                subtitle: t!("Copy under dados/anexos cited in the request").to_owned(),
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
            title: t!("Drop to attach to {tab}", tab = tab.label),
            subtitle: t!("The path will be inserted into this tab's prompt").to_owned(),
        })
    }

    /// Files dropped on the window (`WindowEvent::DroppedFile`): sends them to the panel under the pointer
    /// (to its visible tab) or to the "New task" form.
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
            self.system(t!("Open an agent or the new task form to attach files."));
            return;
        };
        self.spawn_bg("attach-files", move |core| Some(UiMsg::Attached(target, attachment_paths(&core, core.attach_files(&paths)))));
    }

    /// Click on the microphone in a panel header (through the `uid` of the tab visible in it).
    pub fn toggle_tab_mic(&mut self, uid: u64) {
        self.toggle_mic_for(MicTarget::Tab(uid));
    }

    /// `Ctrl+Shift+M` shortcut: if a tab is already opening/recording, stops/cancels it (even if the
    /// user switched tabs); if free, starts on the active tab.
    pub fn toggle_active_tab_mic(&mut self) {
        match self.mic_phase {
            MicPhase::Opening(MicTarget::Tab(uid)) | MicPhase::Recording(MicTarget::Tab(uid)) => {
                self.toggle_mic_for(MicTarget::Tab(uid));
            }
            MicPhase::Idle => {
                if let Some(tab) = self.active_tab() {
                    if !tab.alive {
                        self.system(t!("The active tab has no running process."));
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

/// The file is one of our copies in `dados/anexos` (only those the "×" deletes).
fn in_attachments_dir(data_dir: &Path, path: &Path) -> bool {
    path.parent().is_some_and(|p| p == data_dir.join("anexos"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_our_copies_in_the_attachments_folder_are_deleted() {
        let data = Path::new("/dados");
        assert!(in_attachments_dir(data, &data.join("anexos").join("s_x_1-photo.png")));
        assert!(!in_attachments_dir(data, &data.join("anexos").join("sub").join("a.png")));
        assert!(!in_attachments_dir(data, Path::new("/home/ana/photo.png")));
    }
}

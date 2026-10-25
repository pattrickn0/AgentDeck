//! Teclado: IME, atalhos e teclas que vão ao terminal.

use super::*;

impl Deck {
    /// IME com o terminal focado (teclados CJK, seletor de emoji `Win+.`): a confirmação vai ao
    /// processo como se tivesse sido digitada; a pré-edição só é desenhada, sem enviar nada.
    pub fn handle_ime(&mut self, ime: &winit::event::Ime) {
        use winit::event::Ime;
        match ime {
            Ime::Preedit(text, _) => self.ime_preedit = text.clone(),
            Ime::Commit(text) => {
                self.ime_preedit.clear();
                if let (false, Some(idx), CenterView::Agents) = (text.is_empty(), self.active, self.docs.view) {
                    self.blink_epoch = Instant::now();
                    self.terminal_data(idx, text.as_bytes());
                }
            }
            Ime::Enabled => {}
            Ime::Disabled => self.ime_preedit.clear(),
        }
    }

    /// Trata uma tecla vinda do winit. Retorna `true` se consumiu (não repassar ao egui).
    pub fn handle_key(&mut self, ev: &KeyEvent, mods: ModifiersState, egui_has_focus: bool) -> bool {
        // Lendo documentos o terminal está escondido: não recebe tecla nenhuma, mesmo no quadro em que a chave trocou
        // e `term_focus` ainda não foi desligado (ver `Deck::center_changed`).
        let term_visible = self.docs.view == CenterView::Agents;
        let term_keys = term_visible && !egui_has_focus && self.term_focus && self.active.is_some() && !self.modal_open;
        if ev.state != ElementState::Pressed {
            return term_keys;
        }
        let ctrl = mods.control_key();
        let lower = match &ev.logical_key {
            Key::Character(s) => s.to_lowercase(),
            _ => String::new(),
        };
        // Ctrl+/ (ou Ctrl+?) abre/fecha os atalhos de qualquer lugar, inclusive do terminal.
        if ctrl && !mods.alt_key() && (lower == "/" || lower == "?") {
            if !ev.repeat {
                self.toggle_shortcuts = true;
            }
            return true;
        }
        // Com um modal aberto, o teclado é só dele (Esc, Enter, a caixa de texto).
        if self.modal_open {
            return false;
        }
        if ctrl && mods.alt_key() && !mods.shift_key() {
            let dir = match &ev.logical_key {
                Key::Named(NamedKey::ArrowLeft) => Some((-1, 0)),
                Key::Named(NamedKey::ArrowRight) => Some((1, 0)),
                Key::Named(NamedKey::ArrowUp) => Some((0, -1)),
                Key::Named(NamedKey::ArrowDown) => Some((0, 1)),
                _ => None,
            };
            if let Some((dx, dy)) = dir {
                self.push_action(Action::FocusPane { dx, dy });
                return true;
            }
        }
        if ctrl && !mods.alt_key() {
            if mods.shift_key() && lower == "m" && !ev.repeat {
                self.push_action(Action::ToggleTabMic);
                return true;
            }
            if mods.shift_key() && lower == "d" {
                if !ev.repeat {
                    self.push_action(Action::ToggleCenter);
                }
                return true;
            }
            // Lendo documentos: Ctrl+S grava o que se edita e Ctrl+F abre a busca (o terminal está escondido).
            if !mods.shift_key() && !term_visible && matches!(lower.as_str(), "s" | "f") {
                if !ev.repeat {
                    self.push_action(if lower == "s" { Action::SaveDoc } else { Action::FindInDoc });
                }
                return true;
            }
            let app_key = app_claims_ctrl(&lower, term_keys, self.app_shortcuts_first);
            match lower.as_str() {
                "t" if !ev.repeat => {
                    self.push_action(Action::NewTabShortcut);
                    return true;
                }
                "b" if !ev.repeat && app_key => {
                    self.push_action(Action::ToggleSidebar);
                    return true;
                }
                "v" => self.check_clipboard_image(egui_has_focus),
                _ => {}
            }
            if ev.logical_key == Key::Named(NamedKey::Tab) {
                self.push_action(Action::CycleTab { back: mods.shift_key() });
                return true;
            }
        }
        if egui_has_focus || !self.term_focus || !term_visible {
            return false;
        }
        let Some(idx) = self.active else { return false };
        self.blink_epoch = Instant::now();

        // Ctrl+C copia quando há seleção; Ctrl+V cola.
        if ctrl && !mods.alt_key() && !mods.shift_key() {
            if lower == "c" {
                let sel = self.tabs[idx].session.shared.emu.lock().term.selection_to_string();
                if let Some(text) = sel.filter(|s| !s.is_empty()) {
                    self.push_action(Action::CopyText(text));
                    return true;
                }
            } else if lower == "v" {
                if let Some(text) = clipboard_text().filter(|t| !t.is_empty()) {
                    if self.tabs[idx].alive {
                        let bracketed = self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                        self.user_input(idx, &input::paste_bytes(&text, bracketed), true);
                    }
                }
                return true;
            }
        }

        let (app_cursor, _) = {
            let emu = self.tabs[idx].session.shared.emu.lock();
            (emu.term.mode().contains(TermMode::APP_CURSOR), ())
        };
        let m = Mods { shift: mods.shift_key(), ctrl, alt: mods.alt_key(), meta: mods.super_key() };
        let text = ev.text.as_ref().map(|t| t.as_str());
        let Some(bytes) = input::encode_key_physical(&ev.logical_key, Some(&ev.physical_key), text, m, app_cursor) else { return true };
        self.terminal_data(idx, &bytes);
        true
    }

    /// `term.onData`: envia ao processo ou reinicia com Enter quando morto.
    fn terminal_data(&mut self, idx: usize, bytes: &[u8]) {
        if self.tabs[idx].alive {
            if bytes == b"\r" {
                let tab = &mut self.tabs[idx];
                if !tab.busy {
                    tab.busy_output = false;
                    tab.idle_deadline = Some(Instant::now() + NO_OUTPUT_IDLE);
                }
                tab.busy = true;
                tab.turn_confirmed = false;
                tab.done = false;
            }
            {
                let mut emu = self.tabs[idx].session.shared.emu.lock();
                emu.term.scroll_display(Scroll::Bottom);
                emu.term.selection = None;
            }
            let typing = delivery::typing_after(bytes, self.tabs[idx].typing);
            self.user_input(idx, bytes, typing);
        } else if bytes == b"\r" {
            self.start(idx);
        }
    }

    /// Bytes digitados (ou colados) pelo usuário. Registra a tecla para a regra de entrega e, se
    /// um envio ainda espera o Enter, segura os bytes até ele sair.
    pub(super) fn user_input(&mut self, idx: usize, bytes: &[u8], typing: bool) {
        let tab = &mut self.tabs[idx];
        tab.last_key = Some(Instant::now());
        tab.typing = typing;
        if tab.pending_enter.is_empty() {
            tab.session.write(bytes);
        } else {
            tab.held_keys.extend_from_slice(bytes);
        }
    }

    /// Escreve bytes gerados pelo mouse (roda em modo de relatório) no processo.
    pub fn terminal_mouse_bytes(&mut self, idx: usize, bytes: &[u8]) {
        if self.tabs[idx].alive && !bytes.is_empty() {
            self.tabs[idx].session.write(bytes);
        }
    }

    /// Se a área de transferência não tem texto mas tem imagem, anexa como PNG (no formulário
    /// "Nova tarefa" quando focado, ou no prompt da aba ativa).
    ///
    /// O clipboard é lido aqui, na thread da UI: duas threads nele ao mesmo tempo corrompiam o
    /// heap do Windows (`0xC0000374`). Só a conversão para PNG e o anexo vão para segundo plano.
    fn check_clipboard_image(&self, egui_has_focus: bool) {
        let target =
            if egui_has_focus && self.right_view == RightView::Tarefas && matches!(self.tasks.view, crate::tasks_ui::TasksView::New) {
                AttachTarget::TaskForm(self.active_project.clone())
            } else if let Some(tab) = self.active_tab().filter(|t| t.alive) {
                AttachTarget::Tab(tab.uid)
            } else {
                return;
            };
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
        if cb.get_text().is_ok_and(|t| !t.is_empty()) {
            return;
        }
        let Ok(img) = cb.get_image() else { return };
        let (width, height, bytes) = (img.width as u32, img.height as u32, img.bytes.into_owned());
        self.spawn_bg("clipboard-image", move |core| match core.attach_rgba_png(width, height, &bytes) {
            Ok(r) => Some(UiMsg::Attached(target, attachment_paths(&core, vec![r]))),
            Err(e) => {
                log::warn!("colar imagem: {e}");
                None
            }
        });
    }
}

/// Ctrl+<tecla> é atalho do app? Ctrl+T sempre; Ctrl+B (prefixo do tmux) vai ao terminal quando
/// ele tem o foco, a não ser que o usuário dê prioridade ao app.
fn app_claims_ctrl(key: &str, term_focused: bool, app_first: bool) -> bool {
    match key {
        "b" => !term_focused || app_first,
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctrl_b_goes_to_the_focused_terminal_unless_the_app_comes_first() {
        // Terminal focado: Ctrl+B é do tmux; Ctrl+T continua do app.
        assert!(!app_claims_ctrl("b", true, false));
        assert!(app_claims_ctrl("t", true, false));
        // Fora do terminal, ou com a opção ligada, todos são do app.
        assert!(app_claims_ctrl("b", false, false));
        assert!(app_claims_ctrl("b", true, true));
    }
}

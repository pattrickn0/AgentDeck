//! Quadro: processa eventos, mensagens e temporizadores antes de pintar.

use super::*;

impl Deck {
    /// Processa eventos, mensagens e temporizadores antes de pintar.
    pub fn before_frame(&mut self, ctx: &egui::Context) {
        if self.metrics.is_none() {
            self.metrics = Some(Metrics::measure(ctx));
        }
        for a in std::mem::take(&mut self.actions) {
            self.apply_action(ctx, a);
        }
        if self.docs.view != self.center_seen {
            self.center_changed();
        }
        if let Some(t) = self.work(Some(ctx)) {
            ctx.request_repaint_after(t.saturating_duration_since(Instant::now()));
        }
    }

    /// O trabalho do `before_frame` que não depende de pintar (eventos do core, respostas das threads, entregas aos
    /// terminais, temporizadores), para a janela escondida ou minimizada, que não recebe quadros (nem teclado, nem
    /// ações). Devolve quando chamar de novo; `None` é nada agendado (acorda pelos eventos do core).
    pub fn background(&mut self) -> Option<Instant> {
        self.work(None)
    }

    /// Eventos, mensagens e `tick`: o mesmo trabalho com ou sem quadro (`ctx`), e só uma vez por vez que o laço o pede.
    fn work(&mut self, ctx: Option<&egui::Context>) -> Option<Instant> {
        while let Ok(ev) = self.events.try_recv() {
            self.on_core_event(ev);
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(msg);
        }
        self.tick(ctx)
    }

    fn apply_action(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::NewTabShortcut if self.sidebar_collapsed => {
                // Sem a barra lateral não há "+ Novo agente": o atalho abre o menu de agentes.
                self.new_tab_menu_open = true;
                self.settings_open = false;
                self.refresh_agents();
            }
            Action::NewTabShortcut => {
                let kind = self.active.map(|i| self.tabs[i].kind.clone()).unwrap_or_else(|| self.agents.fallback_kind());
                self.open_tab(&kind, false);
            }
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::ToggleCenter => self.docs.toggle(&self.active_project),
            Action::SaveDoc => {
                if let Some(uid) = self.docs.current_uid(&self.active_project) {
                    self.docs_save(uid, crate::docs::After::Nothing);
                }
            }
            Action::FindInDoc => {
                if let Some(uid) = self.docs.current_uid(&self.active_project) {
                    self.docs_find(uid);
                }
            }
            Action::CycleTab { back } if self.docs.view == CenterView::Docs => self.docs.cycle(&self.active_project, back),
            Action::CycleTab { back } => {
                // Só entre as abas da janela focada.
                let ring = self.focused_pane_tabs();
                let n = ring.len();
                if n > 0 {
                    let cur = self.active.and_then(|a| ring.iter().position(|&i| i == a)).unwrap_or(0) as isize;
                    let next = (cur + if back { -1 } else { 1 } + n as isize).rem_euclid(n as isize) as usize;
                    self.activate(ring[next]);
                }
            }
            Action::FocusPane { dx, dy } => self.focus_pane(dx, dy),
            Action::ToggleTabMic => self.toggle_active_tab_mic(),
            Action::CopyText(t) => ctx.copy_text(t),
        }
    }

    pub(crate) fn on_core_event(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::Feed(entry) => {
                let chime = entry.kind == FeedKind::Answer;
                self.add_feed(entry);
                if chime {
                    self.play_sound(false);
                }
            }
            CoreEvent::WorkspaceChanged => {
                self.refresh_tree();
                self.docs_reload_files();
                self.git_refresh_soon();
            }
            CoreEvent::GitChanged => self.git_refresh_soon(),
            CoreEvent::DictationChanged => {
                self.dictation_installed = self.core.dictation_installed();
                self.refresh_engine();
            }
            CoreEvent::Download(_) if self.debug_hold_download => {}
            CoreEvent::Deliver(d) => self.on_deliver(d),
            CoreEvent::AgentSignal { project, id, busy } => self.on_agent_signal(&project, &id, busy),
            CoreEvent::UsageChanged => {}
            CoreEvent::Activate => self.activate_requested = true,
            CoreEvent::TasksChanged { project } => self.tasks_changed(&project),
            CoreEvent::CloseInstance { project, id } => {
                // Tarefa mesclada ou descartada: o core já esqueceu a instância; a aba (se aberta) fecha aqui.
                if let Some(idx) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == id) {
                    self.close_tab(idx);
                }
            }
            CoreEvent::Github(ev) => self.on_github_event(ev),
            CoreEvent::Download(p) => {
                self.download = match p.state {
                    DownloadState::Done => None,
                    // O erro fica visível por alguns segundos (o detalhe também vai para a Conversa integrada).
                    DownloadState::Error => Some((p, Some(Instant::now() + Duration::from_secs(8)))),
                    _ => Some((p, None)),
                }
            }
        }
    }

    fn on_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Models(kind, list) => {
                self.models_loading.remove(&kind);
                self.model_lists.insert(kind, (list, Instant::now()));
            }
            UiMsg::Agents(list) => {
                self.agents.build(&list);
                for t in &mut self.tabs {
                    t.color = self.agents.tab_color(&t.kind);
                }
                if !self.initialized {
                    self.initialized = true;
                    self.restore_tabs();
                    if let Some(id) = self.debug_switch_project.take() {
                        self.switch_project(&id);
                    }
                    if self.debug_close_all {
                        while !self.tabs.is_empty() {
                            self.close_tab(0);
                        }
                    }
                }
            }
            UiMsg::Attached(target, files) => {
                if files.is_empty() {
                    return;
                }
                match target {
                    AttachTarget::Tab(uid) => {
                        let pairs: Vec<(u64, bool)> = self.tabs.iter().map(|t| (t.uid, t.alive)).collect();
                        match crate::dictation::resolve_tab_delivery(&pairs, uid) {
                            TabDelivery::TypeInTab(idx) => {
                                let refs: Vec<String> = files.iter().map(|f| agentdeck_core::util::ref_path(&self.workspace, f)).collect();
                                let text = util::tab_file_refs_text(&self.tabs[idx].kind, &refs);
                                if !text.is_empty() {
                                    let bracketed =
                                        self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                                    let safe = agentdeck_core::util::terminal_text(&text);
                                    self.user_input(idx, &input::paste_bytes(&safe, bracketed), true);
                                    self.activate(idx);
                                }
                            }
                            TabDelivery::TabDead => {
                                self.system("O processo da aba encerrou antes de receber o anexo.");
                            }
                            TabDelivery::TabClosed => {
                                self.system("A aba do anexo foi fechada antes da cópia terminar.");
                            }
                        }
                    }
                    AttachTarget::TaskForm(project) => {
                        if project == self.active_project {
                            self.tasks.form.attachments.extend(files);
                        }
                    }
                }
            }
            UiMsg::Transcribed(res) => {
                let Some(MicTarget::Tab(uid)) = crate::dictation::finish_transcribe(&mut self.mic_phase) else {
                    return;
                };
                self.refresh_engine();
                let text = match res {
                    Ok(t) => t.trim().to_owned(),
                    Err(e) => {
                        self.system(format!("Não foi possível transcrever: {e}."));
                        return;
                    }
                };
                if text.is_empty() {
                    return;
                }
                let pairs: Vec<(u64, bool)> = self.tabs.iter().map(|t| (t.uid, t.alive)).collect();
                match crate::dictation::resolve_tab_delivery(&pairs, uid) {
                    TabDelivery::TypeInTab(idx) => {
                        let bracketed = self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                        let safe = agentdeck_core::util::terminal_text(&text);
                        self.user_input(idx, &input::paste_bytes(&safe, bracketed), true);
                    }
                    TabDelivery::TabDead => {
                        self.system("O processo da aba encerrou durante a transcrição: o texto foi descartado.");
                    }
                    TabDelivery::TabClosed => {
                        self.system("A aba do ditado foi fechada antes do fim da transcrição: o texto foi descartado.");
                    }
                }
            }
            UiMsg::Engine(label) => self.engine_label = label,
            UiMsg::WorkspacePicked(dir) => {
                self.add_project(dir);
            }
            UiMsg::RemoveProject(id) => self.remove_project_by_id(&id),
            UiMsg::MicOpened(res) => self.on_mic_opened(res),
            UiMsg::Git(generation, reply) => self.on_git(generation, reply),
            UiMsg::Tasks(generation, reply) => self.on_tasks(generation, reply),
            UiMsg::Doc(uid, loaded) => self.on_doc_loaded(uid, loaded),
            UiMsg::DocSaved(uid, saved, result) => self.on_doc_saved(uid, saved, result),
        }
    }

    /// Temporizadores (setTimeout do renderer). Sem `ctx` (janela escondida) nada depende do quadro: o que precisa
    /// dele (copiar para a área de transferência, o mouse pressionado) espera o próximo quadro ou é dado como solto.
    /// Devolve o próximo instante em que há o que fazer.
    fn tick(&mut self, ctx: Option<&egui::Context>) -> Option<Instant> {
        let now = Instant::now();
        let mut next: Option<Instant> = None;
        let mut want = |t: Instant| next = Some(next.map_or(t, |n: Instant| n.min(t)));

        if let Some(at) = self.persist_at {
            if now >= at {
                self.persist_tabs_now();
            } else {
                want(at);
            }
        }
        if let Some((_, Some(at))) = self.download {
            if now >= at {
                self.download = None;
            } else {
                want(at);
            }
        }
        if let Some(at) = self.layout_persist_at {
            if now >= at {
                self.persist_layout_now();
            } else {
                want(at);
            }
        }
        if let Some(since) = self.recording_since {
            let end = since + Duration::from_secs(audio::MAX_RECORDING_SECS.into());
            if now >= end {
                self.stop_recording();
                self.system(format!(
                    "A gravação chegou ao limite de {} minutos: parei e estou transcrevendo o que foi gravado.",
                    audio::MAX_RECORDING_SECS / 60
                ));
            } else {
                want(end);
            }
        }
        if let Some((rel, at)) = self.tree.pending_click.clone() {
            if now >= at + TREE_CLICK_DELAY {
                self.tree.pending_click = None;
                self.tree.active_file = Some(rel.clone());
                if let Some(idx) = self.active.filter(|&i| self.tabs[i].alive) {
                    let text = util::tab_file_refs_text(&self.tabs[idx].kind, &[rel]);
                    if !text.is_empty() {
                        let bracketed = self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                        let safe = agentdeck_core::util::terminal_text(&text);
                        self.user_input(idx, &input::paste_bytes(&safe, bracketed), true);
                        self.set_term_focus(true);
                    }
                }
            } else {
                want(at + TREE_CLICK_DELAY);
            }
        }

        let mut chime = false;
        let metrics = self.metrics.clone();
        let dragging = ctx.is_some_and(|ctx| ctx.input(|i| i.pointer.primary_down()));
        let mut exits = Vec::new();
        let active = self.active;
        // Lendo documentos, nenhuma aba está à vista: o fim de um turno também conta como "não visto".
        let reading = self.docs.view == CenterView::Docs;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(code) = tab.session.take_exit() {
                exits.push((i, code));
            }
            if tab.session.take_bell() {
                chime = true;
            }
            // OSC 52 das outras abas é descartado. Sem quadro, o pedido fica para o próximo (é o `ctx` que copia).
            if let Some(ctx) = ctx {
                if let Some(text) = tab.session.take_clipboard() {
                    if crate::terminal::clipboard_allowed(i, active, &tab.project, &self.active_project) {
                        ctx.copy_text(text);
                    }
                }
            }
            // Enters agendados por `submit`; as teclas seguradas nesse meio-tempo vão logo depois.
            while let Some(&at) = tab.pending_enter.front() {
                if now < at {
                    want(at);
                    break;
                }
                tab.pending_enter.pop_front();
                tab.session.write(b"\r");
                if tab.pending_enter.is_empty() && !tab.held_keys.is_empty() {
                    let held = std::mem::take(&mut tab.held_keys);
                    tab.session.write(&held);
                }
            }
            let seq = tab.session.shared.output_seq.load(std::sync::atomic::Ordering::Relaxed);
            if seq != tab.seen_output {
                tab.seen_output = seq;
                let last = (*tab.session.shared.last_output.lock()).unwrap_or(now);
                self.core.touch_activity(&tab.project, &tab.ident.id(), last);
                tab.busy_output |= tab.busy;
                if tab.busy && tab.turn_confirmed {
                    // Quem libera é o hook de fim de turno; isto só cobre um aviso perdido.
                    tab.idle_deadline = Some(last + HOOK_STALE);
                } else if tab.busy {
                    tab.idle_deadline = Some(last + IDLE_CHIME);
                }
            }
            if tab.busy {
                // Num turno longo o prazo das perguntas a esta aba só começa a correr quando ele acabar.
                self.core.touch_activity(&tab.project, &tab.ident.id(), now);
            }
            if let Some(at) = tab.idle_deadline {
                if now >= at {
                    tab.idle_deadline = None;
                    if tab.busy {
                        tab.busy = false;
                        if std::mem::take(&mut tab.busy_output) {
                            if active != Some(i) || reading {
                                tab.done = true;
                            }
                            chime = true;
                        }
                    }
                } else {
                    want(at);
                }
            }
            if let Some(at) = tab.session.shared.emu.lock().expire_sync(now) {
                want(at);
            }
            if let Some(at) = tab.session.record_tick(now) {
                want(at);
            }
            if let Some((size, at)) = tab.pending_resize {
                if !resize_due(now, at, dragging) {
                    // Mouse ainda pressionado ou tamanho ainda mudando: soltar já pede um quadro; isto é só a garantia.
                    want(if now < at { at } else { now + RESIZE_DEBOUNCE });
                } else if let Some(ready) = tab.resize_blocked_until(now) {
                    // O CLI ainda está reimprimindo a conversa do redimensionamento anterior.
                    want(ready);
                } else {
                    tab.pending_resize = None;
                    if let Some(m) = &metrics {
                        tab.last_resize = Some(now);
                        tab.session.resize(size, m.cell_w, m.cell_h);
                    }
                }
            }
        }
        for (i, code) in exits {
            let tab = &mut self.tabs[i];
            tab.alive = false;
            tab.busy = false;
            tab.done = false;
            for q in tab.queue.drain() {
                if q.answer {
                    log::warn!("resposta atrasada #{} para \"{}\" perdida: a aba encerrou antes de recebê-la", q.request, tab.ident.id());
                    self.core.answer_lost(&tab.project, q.request, "a aba encerrou antes de recebê-la");
                } else if q.request != 0 {
                    self.core.fail_delivery(q.request, "a aba encerrou antes de receber a mensagem");
                }
            }
            self.core.instance_ended(&tab.project, &tab.ident.id());
            tab.session.print(&format!("\r\n\x1b[90m[processo encerrado (código {code}) — pressione Enter para reiniciar]\x1b[0m\r\n"));
        }
        // Mensagens de outros agentes: digitadas no terminal quando a aba fica livre (uma por vez;
        // regra em `agentdeck_core::delivery`).
        for i in 0..self.tabs.len() {
            let Some(since) = self.tabs[i].queue.front_since() else { continue };
            match delivery::gate(&self.tab_state(i), since, now) {
                Gate::Fail => {
                    let tab = &mut self.tabs[i];
                    for q in tab.queue.drain() {
                        if q.answer {
                            log::warn!("resposta atrasada #{} perdida: a aba não está rodando", q.request);
                            self.core.answer_lost(&tab.project, q.request, "a aba não estava rodando");
                        } else if q.request != 0 {
                            self.core.fail_delivery(q.request, "a aba não está rodando");
                        }
                    }
                }
                Gate::Ready => {
                    if let Some(q) = self.tabs[i].queue.pop() {
                        // Pergunta cancelada na fila (expirou, quem perguntou fechou): não faz o agente
                        // trabalhar à toa nem pedir um `--reply` que daria 404.
                        if q.request != 0 && !q.answer && !self.core.question_pending(q.request) {
                            log::info!("pergunta #{} descartada antes de ser digitada: não espera mais resposta", q.request);
                        } else {
                            // O aviso de equipe só é gasto quando a mensagem é digitada de fato.
                            let mut prompt = q.prompt;
                            if let Some(notice) = self.core.take_team_notice(&self.tabs[i].project, &self.tabs[i].ident.id()) {
                                prompt.push_str("\n\n");
                                prompt.push_str(&notice);
                            }
                            self.submit(i, &prompt);
                            if q.answer {
                                self.core.answer_typed(q.request);
                            }
                            want(now + FIRST_ENTER);
                        }
                    }
                    if !self.tabs[i].queue.is_empty() {
                        want(now + delivery::POLL);
                    }
                }
                Gate::WaitUntil(t) => want(t),
            }
        }
        if chime {
            self.play_sound(false);
        }
        if let Some(t) = self.git_tick(now) {
            want(t);
        }
        next
    }

    pub fn play_sound(&self, force: bool) {
        if self.sound_enabled || force {
            audio::play_chime();
        }
    }
}

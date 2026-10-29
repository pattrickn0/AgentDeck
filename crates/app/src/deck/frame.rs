//! Frame: processes events, messages and timers before painting.

use super::*;

impl Deck {
    /// Processes events, messages and timers before painting.
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

    /// The `before_frame` work that does not depend on painting (core events, thread replies, deliveries to the
    /// terminals, timers), for the hidden or minimized window, which gets no frames (nor keyboard, nor
    /// actions). Returns when to call it again; `None` means nothing scheduled (wake on core events).
    pub fn background(&mut self) -> Option<Instant> {
        self.work(None)
    }

    /// Events, messages and `tick`: the same work with or without a frame (`ctx`), and only once per loop request.
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
                // With no sidebar there is no "+ New agent": the shortcut opens the agent menu.
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
                // Only among the tabs of the focused window.
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
                // Merged or discarded task: the core already forgot the instance; the tab (if open) closes here.
                if let Some(idx) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == id) {
                    self.close_tab(idx);
                }
            }
            CoreEvent::Github(ev) => self.on_github_event(ev),
            CoreEvent::Download(p) => {
                self.download = match p.state {
                    DownloadState::Done => None,
                    // The error stays visible for a few seconds (the detail also goes to the Integrated conversation).
                    DownloadState::Error => Some((p, Some(Instant::now() + Duration::from_secs(8)))),
                    _ => Some((p, None)),
                };
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
                                let refs: Vec<String> = files.iter().map(|f| holt_core::util::ref_path(&self.workspace, f)).collect();
                                let text = util::tab_file_refs_text(&self.tabs[idx].kind, &refs);
                                if self.chat_insert(idx, &text) {
                                    self.activate(idx);
                                } else if !text.is_empty() {
                                    let bracketed =
                                        self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                                    let safe = holt_core::util::terminal_text(&text);
                                    self.user_input(idx, &input::paste_bytes(&safe, bracketed), true);
                                    self.activate(idx);
                                }
                            }
                            TabDelivery::TabDead => {
                                self.system(t!("The tab's process ended before receiving the attachment."));
                            }
                            TabDelivery::TabClosed => {
                                self.system(t!("The attachment's tab was closed before the copy finished."));
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
                        self.system(t!("Could not transcribe: {error}.", error = e));
                        return;
                    }
                };
                if text.is_empty() {
                    return;
                }
                let pairs: Vec<(u64, bool)> = self.tabs.iter().map(|t| (t.uid, t.alive)).collect();
                match crate::dictation::resolve_tab_delivery(&pairs, uid) {
                    TabDelivery::TypeInTab(idx) if self.chat_insert(idx, &text) => {}
                    TabDelivery::TypeInTab(idx) => {
                        let bracketed = self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                        let safe = holt_core::util::terminal_text(&text);
                        self.user_input(idx, &input::paste_bytes(&safe, bracketed), true);
                    }
                    TabDelivery::TabDead => {
                        self.system(t!("The tab's process ended during the transcription: the text was discarded."));
                    }
                    TabDelivery::TabClosed => {
                        self.system(t!("The dictation's tab was closed before the transcription finished: the text was discarded."));
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

    /// Timers (the renderer's setTimeout). Without `ctx` (hidden window) nothing depends on the frame: what needs
    /// it (copying to the clipboard, the mouse held down) waits for the next frame or is taken as released.
    /// Returns the next instant when there is something to do.
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
                self.system(t!(
                    "The recording reached the {minutes}-minute limit: I stopped and am transcribing what was recorded.",
                    minutes = audio::MAX_RECORDING_SECS / 60
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
                    if self.chat_insert(idx, &text) {
                        // The reference is in the composer.
                    } else if !text.is_empty() {
                        let bracketed = self.tabs[idx].session.shared.emu.lock().term.mode().contains(TermMode::BRACKETED_PASTE);
                        let safe = holt_core::util::terminal_text(&text);
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
        // Chat tabs: what their processes said, and the ones that ended.
        let mut exits = self.pump_chat();
        // Tabs whose turn ended because the end notice never came (see `idle_deadline`): the hook that did come
        // already told the core (`POST /signal`), which reminds about unread mail.
        let mut released = Vec::new();
        let active = self.active;
        // While reading documents no tab is in view: the end of a turn also counts as "unseen".
        let reading = self.docs.view == CenterView::Docs;
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(code) = tab.session.take_exit() {
                exits.push((i, code));
            }
            if tab.session.take_bell() {
                chime = true;
            }
            // OSC 52 from the other tabs is discarded. Without a frame, the request waits for the next one (`ctx` is what copies).
            if let Some(ctx) = ctx {
                if let Some(text) = tab.session.take_clipboard() {
                    if crate::terminal::clipboard_allowed(i, active, &tab.project, &self.active_project) {
                        ctx.copy_text(text);
                    }
                }
            }
            // Enters scheduled by `submit`; the keys held meanwhile go right after.
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
                    // The end-of-turn hook is what frees it; this only covers a lost notice.
                    tab.idle_deadline = Some(last + HOOK_STALE);
                } else if tab.busy {
                    tab.idle_deadline = Some(last + IDLE_CHIME);
                }
            }
            if tab.busy {
                // In a long turn the deadline for questions to this tab only starts running when it ends.
                self.core.touch_activity(&tab.project, &tab.ident.id(), now);
            }
            // Antigravity names its conversation in its log: save it as soon as it shows up (and again after `/new`).
            if let Some(log) = tab.spec.as_ref().filter(|_| tab.alive).and_then(|s| s.session_log.as_deref()) {
                if now >= tab.session_poll_at {
                    tab.session_poll_at = now + SESSION_LOG_POLL;
                    self.core.sync_agy_session(&tab.project, &tab.ident.id(), log, &mut tab.session_log_pos);
                }
                want(tab.session_poll_at);
            }
            if let Some(at) = tab.idle_deadline {
                if now >= at {
                    tab.idle_deadline = None;
                    if tab.busy {
                        tab.busy = false;
                        released.push((tab.project.clone(), tab.ident.id()));
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
                    // Mouse still pressed or size still changing: releasing already asks for a frame; this is only the guarantee.
                    want(if now < at { at } else { now + RESIZE_DEBOUNCE });
                } else if let Some(ready) = tab.resize_blocked_until(now) {
                    // The CLI is still reprinting the conversation from the previous resize.
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
        for (project, id) in released {
            self.core.mail_turn_ended(&project, &id);
        }
        for (i, code) in exits {
            if self.reopen_without_resume(i, code) {
                continue;
            }
            self.chat_ended(i, code);
            let tab = &mut self.tabs[i];
            tab.alive = false;
            tab.busy = false;
            tab.done = false;
            for q in tab.queue.drain() {
                if let Some(mail) = q.mail {
                    self.core.mail_dropped(&tab.project, mail);
                }
                if q.answer {
                    if q.request != 0 {
                        log::warn!("late answer #{} to \"{}\" lost: the tab ended before receiving it", q.request, tab.ident.id());
                        self.core.answer_lost(&tab.project, q.request, "the tab ended before receiving it");
                    }
                } else if q.request != 0 {
                    self.core.fail_delivery(q.request, "the tab ended before receiving the message");
                }
            }
            self.core.instance_ended(&tab.project, &tab.ident.id());
            if tab.chat.is_none() {
                tab.session
                    .print(&format!("\r\n\x1b[90m[{}]\x1b[0m\r\n", t!("process ended (code {code}) — press Enter to restart", code)));
            }
        }
        // Messages from other agents: typed into the terminal when the tab becomes free (one at a time;
        // rule in `holt_core::delivery`).
        for i in 0..self.tabs.len() {
            let Some(since) = self.tabs[i].queue.front_since() else { continue };
            match delivery::gate(&self.tab_state(i), since, now) {
                Gate::Fail => {
                    let tab = &mut self.tabs[i];
                    for q in tab.queue.drain() {
                        if let Some(mail) = q.mail {
                            self.core.mail_dropped(&tab.project, mail);
                        }
                        if q.answer {
                            if q.request != 0 {
                                log::warn!("late answer #{} lost: the tab is not running", q.request);
                                self.core.answer_lost(&tab.project, q.request, "the tab was not running");
                            }
                        } else if q.request != 0 {
                            self.core.fail_delivery(q.request, "the tab is not running");
                        }
                    }
                }
                Gate::Ready => {
                    if let Some(q) = self.tabs[i].queue.pop() {
                        // Question canceled in the queue (expired, the asker closed): does not make the agent
                        // work for nothing nor ask for a `--reply` that would return 404.
                        if q.request != 0 && !q.answer && !self.core.question_pending(q.request) {
                            log::info!("question #{} dropped before being typed: no answer is expected any more", q.request);
                            if let Some(mail) = q.mail {
                                self.core.mail_dropped(&self.tabs[i].project, mail);
                            }
                        } else if q.mail.is_some_and(|m| !self.core.mail_unread(&self.tabs[i].project, m)) {
                            // Read while the notice waited for its turn: nothing to remind.
                            log::info!("mail notice dropped before being typed: it was already read");
                        } else {
                            // The team notice is only spent when the message is actually typed.
                            let mut prompt = q.prompt;
                            if let Some(notice) = self.core.take_team_notice(&self.tabs[i].project, &self.tabs[i].ident.id()) {
                                prompt.push_str("\n\n");
                                prompt.push_str(&notice);
                            }
                            if self.tabs[i].chat.is_some() {
                                self.chat_deliver(i, &prompt, &q.from);
                            } else {
                                self.submit(i, &prompt);
                            }
                            if q.answer {
                                self.core.answer_typed(q.request);
                            }
                            if let Some(mail) = q.mail {
                                self.core.mail_typed(&self.tabs[i].project, mail);
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

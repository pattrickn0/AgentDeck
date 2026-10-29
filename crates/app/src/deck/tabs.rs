//! Tabs: open, restore, activate, close, rename, split and resize the PTY.

use super::*;

impl Deck {
    /// At startup: registers the saved tabs of every project in the core (the bridge does not create
    /// duplicates of them) and opens only those of the active project; the others open when the
    /// project is selected (or when an agent calls someone from there).
    pub(super) fn restore_tabs(&mut self) {
        for p in self.core.projects() {
            if p.id != self.active_project {
                // Tab of an agent that no longer exists (old config, e.g. `ollama:…`): left out, silently.
                for id in p.open_tabs.iter().filter(|id| Ident::parse(id).is_some_and(|i| self.agents.knows(&i.kind))) {
                    self.core.restore_instance(&p.id, id);
                }
            }
        }
        let active = self.active_project.clone();
        if !active.is_empty() {
            self.load_project(&active);
        }
        self.refresh_tree();
    }

    /// Opens the saved tabs of project `id` (once per session). Being the active one, it takes over the
    /// order and the ratios of the saved windows and activates the saved tab; otherwise the tabs run hidden.
    pub(super) fn load_project(&mut self, id: &str) {
        if self.projects.get(id).is_some_and(|s| s.loaded) {
            return;
        }
        let Some(project) = self.core.projects().into_iter().find(|p| p.id == id) else { return };
        let ratios = split::Ratios { rows: project.split_rows.clone(), cols: project.split_cols.clone() };
        let state = self.projects.entry(id.to_owned()).or_default();
        state.loaded = true;
        state.group_order = project.pane_order.clone();
        state.split_ratios = ratios.clone();
        let shown = id == self.active_project;
        if shown {
            self.group_order = project.pane_order.clone();
            self.split_ratios = ratios;
        }
        // Opens only what was open (no agent opens by itself, not even the first time). `active_tab`
        // points at `open_tabs`; tabs of agents that disappeared are skipped.
        self.restoring = true;
        let mut active_uid = None;
        for (i, saved) in project.open_tabs.iter().enumerate() {
            let Some(ident) = Ident::parse(saved).filter(|i| self.agents.knows(&i.kind)) else { continue };
            // Already open (an agent called this instance before the project was loaded).
            if self.tabs.iter().any(|t| t.project == id && t.ident.id() == *saved) {
                continue;
            }
            self.core.restore_instance(id, saved);
            self.add_tab(id.to_owned(), ident, true, false);
            if i as i32 == project.active_tab {
                active_uid = self.tabs.last().map(|t| t.uid);
            }
        }
        self.restoring = false;
        if shown {
            self.sync_groups();
            self.show_project_tab(active_uid);
        } else if let Some(state) = self.projects.get_mut(id) {
            state.active_uid = active_uid;
        }
    }

    /// Activates tab `uid` of the active project (or the one used last in it).
    pub(super) fn show_project_tab(&mut self, uid: Option<u64>) {
        let visible = self.visible();
        let idx = uid
            .and_then(|u| visible.iter().copied().find(|&i| self.tabs[i].uid == u))
            .or_else(|| self.most_recent(visible.iter().copied()));
        match idx {
            Some(i) => self.activate(i),
            None => self.active = None,
        }
    }

    /// Indices (in `tabs`) of the active project's tabs: only those show. The others keep running.
    pub fn visible(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].project == self.active_project).collect()
    }

    /// Initial grid of a tab: the one of its group's window (or of the whole area, if the group is new;
    /// the first frame fixes it).
    fn current_grid(&self, kind: &str) -> GridSize {
        let group = split::group_key(kind);
        let rect = self.panes.iter().find(|p| p.groups.iter().any(|g| g == group)).map(|p| p.body).or(self.terminal_rect);
        match (&self.metrics, rect) {
            (Some(m), Some(r)) => m.grid_size_for(r),
            _ => GridSize { cols: 120, rows: 30 },
        }
    }

    /// Opens a new tab of model `kind` (+ menu, Ctrl+T): `claude`, and `claude@main@ana` if there already is one.
    pub fn open_tab(&mut self, kind: &str, restoring: bool) {
        self.open_tab_with_model(kind, None, restoring);
    }

    /// Like [`Deck::open_tab`], with the LLM model chosen in the menu (`None` = the role's or the CLI's default).
    pub fn open_tab_with_model(&mut self, kind: &str, model: Option<&str>, restoring: bool) {
        if !restoring {
            self.show_agents();
        }
        let project = self.active_project.clone();
        if project.is_empty() {
            self.system(t!("Open a folder as a project before opening an agent."));
            self.new_project_dialog();
            return;
        }
        match self.core.register_instance_with_model(&project, kind, model) {
            Ok(ident) => self.add_tab(project, ident, restoring, false),
            Err(e) => self.system(t!("Could not open {kind}: {error}.", kind, error = e)),
        }
    }

    /// Another agent's question (`ask-agent`) for an instance: opens its tab, if it does not exist
    /// yet (in the background, without stealing the focus), and queues the question to be typed
    /// into the terminal as soon as it is ready. The answer comes back when the tab's agent runs
    /// `ask-agent --reply <n> "answer"`.
    ///
    /// A late answer (`d.answer`) goes to the tab of whoever asked only if it is still open and
    /// running: it reopens nothing (the answer is already in the integrated Conversation).
    pub(crate) fn on_deliver(&mut self, d: Delivery) {
        let id = d.ident.id();
        // A project not yet opened in this session opens its saved tabs (hidden) first.
        if !d.answer {
            self.load_project(&d.project);
        }
        let found = self.tabs.iter().position(|t| t.project == d.project && t.ident.id() == id);
        if d.answer {
            match found {
                // In a plain terminal the text would run as a command (the bridge already avoids that).
                Some(i) if self.tabs[i].alive && self.tabs[i].kind != "shell" => {
                    self.tabs[i].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, answer: true, mail: d.mail })
                }
                _ => {
                    if let Some(mail) = d.mail {
                        // Stays unread: the notice comes again when the tab is back.
                        self.core.mail_dropped(&d.project, mail);
                    }
                    if d.request != 0 {
                        log::warn!("late answer #{} for \"{id}\" discarded: the tab is not open", d.request);
                        self.core.answer_lost(&d.project, d.request, "the tab was not open");
                    }
                }
            }
            return;
        }
        // New tab or dead process: the CLI comes up already with the task's initial prompt (command line), instead of
        // the text being typed into a terminal that is still starting and getting lost.
        let boots = d.boot.as_deref().filter(|_| found.is_none_or(|i| !self.tabs[i].alive));
        if let Some(line) = boots {
            self.core.set_boot_prompt(&d.project, &id, line.to_owned());
        }
        let idx = match found {
            Some(i) => i,
            None => {
                // A task's tab does not count toward the limit of tabs opened by other agents: the user asked for it.
                let task = self.core.task_of_instance(&d.project, &id);
                let reason = if !self.agents.knows(&d.ident.kind) {
                    Some(format!("the agent \"{}\" is not available", d.ident.kind))
                } else if task.is_none() && self.tabs.iter().filter(|t| t.auto).count() >= MAX_AUTO_TABS {
                    Some(format!("limit of {MAX_AUTO_TABS} automatically opened tabs; close one"))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.core.take_boot_prompt(&d.project, &id);
                    self.core.fail_delivery(d.request, &reason);
                    self.core.close_instance(&d.project, &id);
                    return;
                }
                self.add_tab(d.project.clone(), d.ident.clone(), false, true);
                let label = self.tabs.last().map(|t| t.label.clone()).unwrap_or_else(|| id.clone());
                match &task {
                    Some(t) => {
                        self.system_in(&d.project, t!("Opened the {label} tab for task {id}: {title}.", label, id = t.id, title = t.title))
                    }
                    None => self.system_in(&d.project, t!("Opened the {label} tab for the conversation with another agent.", label)),
                }
                self.tabs.len() - 1
            }
        };
        if !self.tabs[idx].alive {
            self.start(idx); // the process had ended: start it again to receive the question
        }
        // The initial prompt is for one opening only: left over (the CLI did not accept it, or the tab was already
        // alive) = type the text.
        let booted = boots.is_some() && self.core.take_boot_prompt(&d.project, &id).is_none() && self.tabs[idx].alive;
        if !booted {
            self.tabs[idx].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, answer: false, mail: d.mail });
        }
    }

    /// Message from the interface itself (GitHub tab) to an instance of the active project: opens the tab
    /// if it does not exist (or, with `force_new`, opens another one of the same role) and queues the text,
    /// which is typed when the tab is free (same rule as `ask-agent`, in `delivery`).
    /// Returns the destination tab's label or the reason for the refusal.
    pub fn send_to_instance(&mut self, want: &Ident, force_new: bool, prompt: String) -> Result<String, String> {
        let project = self.active_project.clone();
        if project.is_empty() {
            return Err(t!("Open a folder as a project before sending to an agent.").into());
        }
        if !self.agents.knows(&want.kind) || want.kind == "shell" {
            return Err(t!("The agent \"{agent}\" is not available.", agent = want.kind));
        }
        self.load_project(&project);
        let ident = match self.core.resolve_instance(&project, want, force_new) {
            Ok((ident, _)) => ident,
            Err(options) => {
                return Err(t!("\"{name}\" is ambiguous: {options}. Pick one.", name = want.id(), options = options.join(", ")));
            }
        };
        let id = ident.id();
        let idx = match self.tabs.iter().position(|t| t.project == project && t.ident.id() == id) {
            Some(i) => i,
            None => {
                self.add_tab(project.clone(), ident, false, false);
                self.tabs.len() - 1
            }
        };
        if !self.tabs[idx].alive {
            self.start(idx);
        }
        self.activate(idx);
        self.tabs[idx].queue.push(Instant::now(), Queued { request: 0, prompt: prompt.clone(), answer: false, mail: None });
        self.add_feed(FeedEntry::new(FeedKind::User, Some("you"), Some(&id), prompt));
        Ok(self.tabs[idx].label.clone())
    }

    /// Shows the tab of a task's agent. If it was closed and the task is still running, in
    /// review or waiting for validation, registers the instance again and opens the tab: the terminal opens in the task's
    /// worktree (`spawn_spec_for`) with the task's model. Returns the reason for the refusal.
    pub fn open_task_tab(&mut self, task: &holt_core::Task) -> Result<(), String> {
        use holt_core::TaskStatus;
        let project = self.active_project.clone();
        self.show_agents();
        if let Some(i) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == task.instance) {
            self.activate(i);
            return Ok(());
        }
        if !matches!(task.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating) {
            return Err(t!("Task {id} is {status}: no agent is running.", id = task.id, status = task.status.label()));
        }
        let Some(ident) = Ident::parse(&task.instance).filter(|i| self.agents.knows(&i.kind)) else {
            return Err(t!("The agent \"{agent}\" for this task is not available.", agent = task.kind));
        };
        self.core.restore_instance(&project, &task.instance);
        self.core.assign_model(&project, &ident, task.model.as_deref());
        self.add_tab(project, ident, false, false);
        Ok(())
    }

    /// What the delivery rule needs to know about the tab.
    pub(super) fn tab_state(&self, idx: usize) -> TabState {
        let tab = &self.tabs[idx];
        TabState {
            alive: tab.alive,
            // A send whose Enter has not gone out yet also occupies the tab.
            busy: tab.busy || !tab.pending_enter.is_empty(),
            hooked: tab.hooked,
            spawned_at: tab.spawned_at,
            last_output: *tab.session.shared.last_output.lock(),
            last_key: tab.last_key,
            typing: tab.typing,
        }
    }

    /// The agent's hook (`ask-agent --signal`): start or end of turn.
    pub(crate) fn on_agent_signal(&mut self, project: &str, id: &str, busy: bool) {
        let Some((idx, tab)) = self.tabs.iter_mut().enumerate().find(|(_, t)| t.project == project && t.ident.id() == id) else {
            return;
        };
        tab.hooked = true;
        tab.turn_confirmed = busy;
        // Busy: if the end-of-turn notice gets lost, the tab is released after HOOK_STALE.
        tab.idle_deadline = busy.then(|| Instant::now() + HOOK_STALE);
        // Turn confirmed by the hook: if the end notice gets lost, the release still counts as an end.
        tab.busy_output |= busy;
        let finished = tab.busy && !busy;
        tab.busy = busy;
        if busy {
            tab.done = false;
        } else if finished {
            if self.active != Some(idx) || self.docs.view == CenterView::Docs {
                tab.done = true;
            }
            self.play_sound(false);
        }
    }

    /// Creates the instance's tab and starts the process. An `auto` tab stays in the background (it only
    /// gets the focus if there is no other one).
    fn add_tab(&mut self, project: String, ident: Ident, restoring: bool, auto: bool) {
        let kind = ident.kind.clone();
        let label = ident.label(&self.agents.name(&kind));
        // Request > saved > role default: saves it to show in the sidebar and come back with the app.
        let model = self.core.assign_model(&project, &ident, None);
        let tab = Tab {
            uid: self.next_uid,
            kind: kind.clone(),
            ident,
            project,
            label,
            model,
            color: self.agents.tab_color(&kind),
            session: Session::new(self.current_grid(&kind)),
            view: ViewState::default(),
            alive: false,
            busy: false,
            done: false,
            shown: 0,
            auto,
            queue: Mailbox::default(),
            last_key: None,
            typing: false,
            hooked: false,
            turn_confirmed: false,
            pending_enter: VecDeque::new(),
            held_keys: Vec::new(),
            seen_output: 0,
            idle_deadline: None,
            busy_output: false,
            spec: None,
            pending_resize: None,
            last_resize: None,
            spawned_at: None,
            session_log_pos: 0,
            session_poll_at: Instant::now(),
        };
        self.next_uid += 1;
        // The tab of another project runs hidden: it does not touch the windows or the focus.
        let shown = tab.project == self.active_project;
        self.tabs.push(tab);
        let idx = self.tabs.len() - 1;
        if shown && !self.restoring {
            self.sync_groups();
        }
        if shown && (!auto || self.active.is_none()) {
            self.activate(idx);
        }
        if !restoring {
            self.persist_tabs();
        }
        self.start(idx);
    }

    /// `start(tab)`: restarts the terminal and the process.
    pub fn start(&mut self, idx: usize) {
        let wake = self.wake.clone();
        let grid = self.current_grid(&self.tabs[idx].kind);
        let ident = self.tabs[idx].ident.clone();
        let spec = match self.core.spawn_spec_for(&self.tabs[idx].project, &ident) {
            Ok(spec) => Ok(spec),
            Err(e) if self.debug_fake_shell => {
                log::warn!("HOLT_DEBUG_FAKE_SHELL: using the local shell instead of '{}' ({e})", self.tabs[idx].kind);
                let cwd = self.core.project_path(&self.tabs[idx].project).unwrap_or_else(|| self.workspace.clone());
                Ok(debug_fake_spec(&self.tabs[idx].kind, &cwd))
            }
            Err(e) => Err(e),
        };
        let tab = &mut self.tabs[idx];
        tab.session.reset();
        // New process: the hooks have to introduce themselves again, and nothing from the previous one counts.
        tab.hooked = false;
        tab.turn_confirmed = false;
        tab.busy = false;
        tab.done = false;
        tab.pending_enter.clear();
        tab.held_keys.clear();
        if let Some(m) = &self.metrics {
            tab.session.resize(grid, m.cell_w, m.cell_h);
        }
        if self.debug_pty {
            tab.session.record_to(self.core.paths().data_dir.join("pty"), ident.id());
        }
        match spec.and_then(|s| tab.session.spawn(&s, wake).map(|_| s).map_err(|e| e.to_string())) {
            Ok(spec) => {
                tab.alive = true;
                tab.spawned_at = Some(Instant::now());
                tab.session_log_pos = 0;
                tab.session_poll_at = Instant::now();
                tab.model = spec.model.clone();
                tab.spec = Some(spec);
                // Mail that nobody told this process about (the app restarted, or the notice was lost).
                self.core.mail_tab_started(&tab.project, &ident.id());
            }
            Err(msg) => {
                tab.alive = false;
                tab.spec = None;
                let msg = msg.replace("\r\n", "\n").replace('\n', "\r\n");
                tab.session.print(&format!("\x1b[33m{msg}\x1b[0m\r\n\r\n\x1b[90m[{}]\x1b[0m\r\n", t!("press Enter to try again")));
            }
        }
    }

    /// The tab's process ended with `code`. If it had been launched to resume its saved conversation and failed right away
    /// (the CLI does not know the session), forgets the id, starts the tab again from scratch and warns once in the
    /// chat. The new launch has no stored id, so it cannot repeat. Returns whether it did.
    pub(crate) fn reopen_without_resume(&mut self, idx: usize, code: i64) -> bool {
        let tab = &self.tabs[idx];
        let resumed = tab.spec.as_ref().is_some_and(|s| s.resumed);
        if !resume::resume_failed(resumed, tab.spawned_at, Instant::now(), code) {
            return false;
        }
        let (project, id, label) = (tab.project.clone(), tab.ident.id(), tab.label.clone());
        log::warn!("{id}: the saved conversation could not be resumed (exit code {code}); opening it from scratch");
        self.core.forget_tab_session(&project, &id);
        self.core.set_restart_boot_prompt(&project, &id);
        self.start(idx);
        self.system_in(&project, t!("{label} could not resume its previous conversation. It was opened again from scratch.", label));
        true
    }

    pub fn activate(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        // Tab of another project (e.g. a click on the sidebar tree): its project comes to the screen.
        if self.tabs[idx].project != self.active_project {
            let (project, uid) = (self.tabs[idx].project.clone(), self.tabs[idx].uid);
            self.switch_project(&project);
            if let Some(i) = self.tabs.iter().position(|t| t.uid == uid) {
                self.activate(i);
            }
            return;
        }
        self.active = Some(idx);
        self.show_seq += 1;
        self.tabs[idx].shown = self.show_seq;
        // In Highlight the large window follows the active tab: switching changes the windows' width, and the PTY
        // waits longer for the CLI to finish reprinting (see `Deck::fit_tab`).
        if self.organization == Organization::Highlight {
            self.highlight_switch_until = Some(Instant::now() + HIGHLIGHT_RESIZE_DEBOUNCE);
        }
        // Reading documents, the activated tab stays out of sight: the key's badge still counts.
        if self.docs.view == CenterView::Agents {
            self.tabs[idx].done = false;
        }
        self.set_term_focus(true);
        self.persist_tabs();
    }

    /// Keeps `group_order` equal to the open groups: a new group enters at the end (becomes a new window)
    /// and leaves when its last tab closes.
    pub(super) fn sync_groups(&mut self) {
        let kinds: Vec<String> = self.visible().into_iter().map(|i| self.tabs[i].kind.clone()).collect();
        split::sync_order(&mut self.group_order, kinds.iter().map(String::as_str));
    }

    /// Windows of this frame in area `area`: the automatic grid with one group per window (Split), the selected
    /// CLI's window large with the others stacked beside it (Highlight) or a single window
    /// (Tabs). Stores the result in `panes` for the shortcuts. In Highlight, window 0 is the large one.
    pub fn plan_panes(&mut self, area: egui::Rect) -> (Vec<usize>, Vec<PanePlan>) {
        let many = self.group_order.len() > 1;
        let (shape, rects, groups) = if self.organization == Organization::Split && many {
            let shape = split::grid_shape(self.group_order.len(), area.width(), area.height());
            // Only adjusts for drawing: the saved ratios change only when the user drags.
            let rects = split::layout(area, &shape, &self.split_ratios.fitted(&shape));
            let groups = split::assign(&self.group_order, rects.len());
            (shape, rects, groups)
        } else if self.organization == Organization::Highlight && many {
            let featured = self.active_tab().map(|t| split::group_key(&t.kind).to_owned());
            let order = split::highlight_order(&self.group_order, featured.as_deref());
            let windows = split::highlight_windows(order.len(), area.width(), area.height());
            let rects = split::highlight_layout(area, windows, self.highlight_ratio);
            let groups = split::assign(&order, rects.len());
            (vec![rects.len()], rects, groups)
        } else {
            (vec![1], vec![area], vec![self.group_order.clone()])
        };
        let one = groups.len() == 1;
        let visible = self.visible();
        let keys: Vec<&str> = visible.iter().map(|&i| split::group_key(&self.tabs[i].kind)).collect();
        let plans: Vec<PanePlan> = rects
            .into_iter()
            .zip(groups)
            .map(|(rect, groups)| {
                // A single window: all tabs in opening order (as without a split).
                let tabs: Vec<usize> =
                    if one { visible.clone() } else { split::pane_tabs(&keys, &groups).into_iter().map(|k| visible[k]).collect() };
                let shown = self.most_recent(tabs.iter().copied()).unwrap_or(0);
                PanePlan { rect, tabs, shown, groups }
            })
            .filter(|p| !p.tabs.is_empty())
            .collect();
        self.panes = plans
            .iter()
            .map(|p| PaneSlot {
                rect: p.rect,
                body: crate::ui::pane_body(p.rect),
                uids: p.tabs.iter().map(|&i| self.tabs[i].uid).collect(),
                shown_uid: self.tabs[p.shown].uid,
                groups: p.groups.clone(),
            })
            .collect();
        (shape, plans)
    }

    /// The most recently activated tab among `tabs` (tie: the first one).
    fn most_recent(&self, tabs: impl Iterator<Item = usize>) -> Option<usize> {
        let shown: Vec<u64> = self.tabs.iter().map(|t| t.shown).collect();
        split::most_recent(&shown, tabs)
    }

    /// Window (from the last frame) that has the active tab.
    pub(super) fn focused_slot(&self) -> Option<usize> {
        let uid = self.active_tab()?.uid;
        self.panes.iter().position(|p| p.uids.contains(&uid))
    }

    fn slot_tabs(&self, slot: usize) -> Vec<usize> {
        self.panes[slot].uids.iter().filter_map(|u| self.tabs.iter().position(|t| t.uid == *u)).collect()
    }

    /// Tabs of the focused window (the project's ones, if there has been no frame yet).
    pub(super) fn focused_pane_tabs(&self) -> Vec<usize> {
        match self.focused_slot() {
            Some(s) => self.slot_tabs(s),
            None => self.visible(),
        }
    }

    /// Focus on the neighboring window (Ctrl+Alt+arrows): activates its visible tab.
    pub fn focus_pane(&mut self, dx: i32, dy: i32) {
        let Some(from) = self.focused_slot() else { return };
        let rects: Vec<egui::Rect> = self.panes.iter().map(|p| p.rect).collect();
        if let Some(to) = split::neighbor(&rects, from, dx, dy) {
            if let Some(i) = self.most_recent(self.slot_tabs(to).into_iter()) {
                self.activate(i);
            }
        }
    }

    /// Swaps two windows around (dragging one's handle over the other).
    pub fn swap_panes(&mut self, a: usize, b: usize) {
        let first = |s: usize| self.panes.get(s).and_then(|p| p.groups.first()).cloned();
        let (Some(ga), Some(gb)) = (first(a), first(b)) else { return };
        let ia = self.group_order.iter().position(|g| *g == ga);
        let ib = self.group_order.iter().position(|g| *g == gb);
        if let (Some(ia), Some(ib)) = (ia, ib) {
            self.group_order.swap(ia, ib);
            self.persist_tabs();
        }
    }

    /// Changes how the windows arrange themselves (Tabs, Split or Highlight).
    pub fn set_organization(&mut self, org: Organization) {
        if self.organization != org {
            self.organization = org;
            self.pane_drag = None;
            self.persist_layout();
        }
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        tab.session.kill();
        for q in tab.queue.drain() {
            if q.answer {
                log::warn!("late answer #{} lost: the tab was closed before receiving it", q.request);
                self.core.answer_lost(&tab.project, q.request, "the tab was closed before receiving it");
            } else if q.request != 0 {
                self.core.fail_delivery(q.request, "the tab was closed before receiving the message");
            }
        }
        // Forgets the instance and releases whoever was waiting for an answer from it.
        self.core.close_instance(&tab.project, &tab.ident.id());
        // Indices kept between frames (rename modal, context menu) follow their tab.
        let shift = |i: usize| match i.cmp(&idx) {
            std::cmp::Ordering::Less => Some(i),
            std::cmp::Ordering::Equal => None,
            std::cmp::Ordering::Greater => Some(i - 1),
        };
        self.rename_tab = self.rename_tab.take().and_then(|(i, buf, err)| Some((shift(i)?, buf, err)));
        self.tab_context_menu = self.tab_context_menu.and_then(|(i, pos)| Some((shift(i)?, pos)));
        self.sync_groups();
        match self.active {
            Some(a) if a == idx => {
                // Stays in the same window (the group's tab used last); with none of them, the last one used.
                let visible = self.visible();
                let kinds: Vec<&str> = visible.iter().map(|&i| self.tabs[i].kind.as_str()).collect();
                let shown: Vec<u64> = visible.iter().map(|&i| self.tabs[i].shown).collect();
                match split::after_close(&kinds, &shown, &tab.kind) {
                    Some(k) => self.activate(visible[k]),
                    None => self.active = None,
                }
            }
            Some(a) if a > idx => self.active = Some(a - 1),
            _ => {}
        }
        self.persist_tabs_now();
    }

    /// Folder of the terminal's diagnostic recordings (`data/pty`).
    pub fn pty_dir(&self) -> PathBuf {
        self.core.paths().data_dir.join("pty")
    }

    /// Turns the terminal diagnostic recording of tab `idx` on or off (tab menu item) and says where the files are.
    pub fn toggle_terminal_recording(&mut self, idx: usize) {
        let dir = self.pty_dir();
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let id = tab.ident.id();
        if tab.session.recording() {
            tab.session.set_recording(None);
            self.system(t!("Terminal recording for {id} turned off. The files are in {dir}.", id, dir = dir.display()));
        } else {
            tab.session.set_recording(Some((dir.clone(), id.clone())));
            self.system(t!(
                "Recording the terminal of {id} from now on to {dir} (three files per recording: .bin, .events.txt and .grid.txt). When the defect shows up, use \"Save grid snapshot now\" in the tab menu (right click) and send those files.",
                id,
                dir = dir.display()
            ));
        }
    }

    /// Saves the terminal grid of tab `idx` now and opens the folder (tab menu item).
    pub fn save_terminal_snapshot(&mut self, idx: usize) {
        let dir = self.pty_dir();
        let Some(tab) = self.tabs.get(idx) else { return };
        let id = tab.ident.id();
        match tab.session.save_snapshot(&dir, &id) {
            Ok(path) => {
                self.system(t!("Terminal grid snapshot for {id} saved to {path}.", id, path = path.display()));
                open_in_file_manager(&dir);
            }
            Err(e) => self.system(t!("Could not save the terminal grid snapshot for {id}: {error}", id, error = e)),
        }
    }

    pub fn start_rename_tab(&mut self, idx: usize) {
        if idx < self.tabs.len() {
            let id = self.tabs[idx].ident.id();
            self.rename_tab = Some((idx, id, None));
            self.tab_context_menu = None;
        }
    }

    pub fn apply_rename_tab(&mut self, idx: usize, new_text: &str) -> Result<(), String> {
        if idx >= self.tabs.len() {
            return Err(t!("tab not found").into());
        }
        let old_id = self.tabs[idx].ident.id();
        let kind = self.tabs[idx].kind.clone();
        let trimmed = new_text.trim();
        // With "@" the text already is a full id (the model is checked below); without it, it is just the role.
        let full_text = if trimmed == kind || trimmed.contains('@') {
            trimmed.to_owned()
        } else if trimmed.is_empty() {
            kind.clone()
        } else {
            format!("{kind}@{trimmed}")
        };
        let Some(new_ident) = Ident::parse(&full_text) else {
            return Err(t!("invalid format: \"{text}\". Use model@role@name (e.g. {kind}@dev or {kind}@dev@ana)", text = trimmed, kind));
        };
        if new_ident.kind != kind {
            return Err(t!("the model cannot be changed from \"{kind}\" to \"{other}\"", kind, other = new_ident.kind));
        }
        let new_id = new_ident.id();
        if new_id != old_id {
            self.core.rename_instance(&self.tabs[idx].project, &old_id, &new_ident)?;
            let new_label = new_ident.label(&self.agents.name(&kind));
            self.tabs[idx].ident = new_ident;
            self.tabs[idx].label = new_label;
            // Updates the project's integrated conversation: the items keep the display name
            // (`who_name`), and the "thinking" moves to the new id (its next message erases it).
            let project = self.tabs[idx].project.clone();
            let (old_who, new_who) = (self.agents.who_name(&old_id), self.agents.who_name(&new_id));
            let feed =
                if project == self.active_project { &mut self.feed } else { &mut self.projects.entry(project.clone()).or_default().feed };
            rename_in_feed(feed, &old_who, &new_who);
            rename_thinking(&mut self.thinking, &project, &old_id, &new_id);
            self.persist_tabs();
        }
        self.rename_tab = None;
        Ok(())
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    /// Plan usage by model (the fabricated one in debug).
    pub fn usage(&self) -> HashMap<String, holt_core::AgentUsage> {
        match &self.debug_usage {
            Some(u) => u.clone(),
            None => self.core.usage_snapshot(),
        }
    }

    /// Accent color: the focused tab's; with no tab, a neutral light gray. It is the only strong color in the
    /// interface (selection, primary action, active divider) and is never a fixed brand color.
    pub fn accent(&self) -> Color32 {
        self.active_tab().map(|t| t.color).unwrap_or(theme::pal().accent_neutral)
    }

    /// The focused tab's color, if there is one (feeds `theme::set_accent`).
    pub fn accent_opt(&self) -> Option<Color32> {
        self.active_tab().map(|t| t.color)
    }

    pub(super) fn persist_tabs(&mut self) {
        if self.restoring {
            return;
        }
        self.persist_at = Some(Instant::now() + PERSIST_DEBOUNCE);
    }

    pub fn persist_tabs_now(&mut self) {
        if self.restoring {
            return;
        }
        self.persist_at = None;
        // Each loaded project saves its tabs; those not yet opened in this session stay
        // as they are (otherwise they would lose their saved tabs).
        let loaded: Vec<String> = self.projects.iter().filter(|(_, s)| s.loaded).map(|(k, _)| k.clone()).collect();
        for project in loaded {
            let shown = project == self.active_project;
            let mine: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].project == project).collect();
            let open: Vec<String> = mine.iter().map(|&i| self.tabs[i].ident.id()).collect();
            let state = &self.projects[&project];
            let active_uid = if shown { self.active_tab().map(|t| t.uid) } else { state.active_uid };
            let idx = active_uid.and_then(|u| mine.iter().position(|&i| self.tabs[i].uid == u)).map_or(-1, |i| i as i32);
            let order = if shown { self.group_order.clone() } else { state.group_order.clone() };
            self.core.update_project(&project, |p| {
                p.open_tabs = open;
                p.active_tab = idx;
                p.pane_order = order;
            });
        }
    }

    /// Pins agent `model`'s group `group` in the title bar (`None` goes back to the summary) and saves it.
    pub fn set_usage_pin(&mut self, model: &str, group: Option<String>) {
        match &group {
            Some(g) => self.usage_pins.insert(model.to_owned(), g.clone()),
            None => self.usage_pins.remove(model),
        };
        let model = model.to_owned();
        self.core.update_config(|c| match group {
            Some(g) => {
                c.usage_pins.insert(model, g);
            }
            None => {
                c.usage_pins.remove(&model);
            }
        });
    }

    pub fn persist_layout(&mut self) {
        self.layout_persist_at = Some(Instant::now() + Duration::from_millis(400));
    }

    pub fn persist_layout_now(&mut self) {
        self.layout_persist_at = None;
        let (sw, sc, rw) = (self.sidebar_w, self.sidebar_collapsed, self.right_w);
        let (org, highlight, ratios) = (self.organization, self.highlight_ratio, self.split_ratios.clone());
        self.core.update_config(|c| {
            c.sidebar_width = sw;
            c.sidebar_collapsed = sc;
            c.right_panel_width = rw;
            c.set_organization(org);
            c.highlight_ratio = highlight;
        });
        // The screen's project (the core's may have changed already, e.g. when the active one is removed).
        if self.active_project.is_empty() {
            return;
        }
        self.core.update_project(&self.active_project, |p| {
            p.split_rows = ratios.rows;
            p.split_cols = ratios.cols;
        });
    }

    /// Fits a visible tab's grid to its area, when the size stops changing (see [`resize_due`]).
    /// Switching tabs in Highlight changes the windows' size without the user dragging anything, and there the PTY
    /// gets more slack (see [`HIGHLIGHT_RESIZE_DEBOUNCE`]).
    pub fn fit_tab(&mut self, idx: usize, rect: egui::Rect) {
        let Some(m) = self.metrics.clone() else { return };
        let size = m.grid_size_for(rect);
        let now = Instant::now();
        let delay = if self.highlight_switch_until.is_some_and(|until| now < until) { HIGHLIGHT_RESIZE_DEBOUNCE } else { RESIZE_DEBOUNCE };
        let tab = &mut self.tabs[idx];
        // Back to the size the PTY already has (A→B→A within the deadline): there is nothing to resize, and the
        // pending one from the middle of the way is canceled instead of becoming a pointless resize.
        if size == tab.session.size() {
            tab.pending_resize = None;
            return;
        }
        let current = tab.pending_resize.map(|p| p.0).unwrap_or(tab.session.size());
        if size != current {
            tab.pending_resize = Some((size, now + delay));
        }
    }

    pub fn set_term_focus(&mut self, on: bool) {
        self.term_focus = on && self.docs.view == CenterView::Agents;
        if self.term_focus {
            self.blink_epoch = Instant::now();
            if let Some(i) = self.active {
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.done = false;
                }
            }
        }
    }
}

/// Renames the display name of who speaks/to whom in the Conversation items (renamed tab).
fn rename_in_feed(feed: &mut [FeedItem], old_who: &str, new_who: &str) {
    for item in feed {
        for who in [&mut item.who, &mut item.to] {
            if who.as_deref() == Some(old_who) {
                *who = Some(new_who.to_owned());
            }
        }
    }
}

/// `old_id`'s "thinking" in the project becomes `new_id`'s (key `project#id`, see `add_feed`).
fn rename_thinking(thinking: &mut HashMap<String, u64>, project: &str, old_id: &str, new_id: &str) {
    if let Some(item) = thinking.remove(&format!("{project}#{old_id}")) {
        thinking.insert(format!("{project}#{new_id}"), item);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(id: u64, who: &str, to: Option<&str>) -> FeedItem {
        FeedItem {
            id,
            kind: FeedKind::Thinking,
            who: Some(who.into()),
            to: to.map(Into::into),
            text: String::new(),
            time: String::new(),
            color: Color32::WHITE,
        }
    }

    #[test]
    fn renaming_a_tab_renames_its_feed_items_and_thinking_entry() {
        let mut feed = vec![item(1, "Claude", Some("Antigravity")), item(2, "Antigravity", Some("Claude")), item(3, "Codex", None)];
        rename_in_feed(&mut feed, "Claude", "Claude@dev");
        let names: Vec<_> = feed.iter().map(|f| (f.who.as_deref().unwrap(), f.to.as_deref())).collect();
        assert_eq!(names, [("Claude@dev", Some("Antigravity")), ("Antigravity", Some("Claude@dev")), ("Codex", None)]);

        let mut thinking = HashMap::from([("p#claude".to_owned(), 1), ("q#claude".to_owned(), 9)]);
        rename_thinking(&mut thinking, "p", "claude", "claude@dev");
        assert_eq!(thinking.get("p#claude@dev"), Some(&1));
        assert!(!thinking.contains_key("p#claude"));
        // The same id in another project is not it.
        assert_eq!(thinking.get("q#claude"), Some(&9));
    }
}

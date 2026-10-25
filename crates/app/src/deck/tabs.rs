//! Abas: abrir, restaurar, ativar, fechar, renomear, dividir e redimensionar o PTY.

use super::*;

impl Deck {
    /// Ao iniciar: registra no core as abas salvas de todos os projetos (a ponte não cria
    /// duplicatas delas) e abre só as do projeto ativo; as outras abrem quando o projeto é
    /// selecionado (ou quando um agente chama alguém de lá).
    pub(super) fn restore_tabs(&mut self) {
        for p in self.core.projects() {
            if p.id != self.active_project {
                // Aba de um agente que não existe mais (config antiga, ex.: `ollama:…`): fica de fora, em silêncio.
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

    /// Abre as abas salvas do projeto `id` (uma vez por sessão). Sendo o ativo, ele assume a ordem
    /// e as proporções das janelas salvas e ativa a aba salva; senão as abas rodam escondidas.
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
        // Abre só o que estava aberto (nenhum agente abre sozinho, nem na primeira vez). `active_tab`
        // aponta para `open_tabs`; abas de agentes que sumiram são puladas.
        self.restoring = true;
        let mut active_uid = None;
        for (i, saved) in project.open_tabs.iter().enumerate() {
            let Some(ident) = Ident::parse(saved).filter(|i| self.agents.knows(&i.kind)) else { continue };
            // Já aberta (um agente chamou esta instância antes de o projeto ser carregado).
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

    /// Ativa a aba `uid` do projeto ativo (ou a usada por último nele).
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

    /// Índices (em `tabs`) das abas do projeto ativo: só elas aparecem. As outras continuam rodando.
    pub fn visible(&self) -> Vec<usize> {
        (0..self.tabs.len()).filter(|&i| self.tabs[i].project == self.active_project).collect()
    }

    /// Grade inicial de uma aba: a da janela do grupo dela (ou a da área toda, se o grupo é novo;
    /// o primeiro quadro corrige).
    fn current_grid(&self, kind: &str) -> GridSize {
        let group = split::group_key(kind);
        let rect = self.panes.iter().find(|p| p.groups.iter().any(|g| g == group)).map(|p| p.body).or(self.terminal_rect);
        match (&self.metrics, rect) {
            (Some(m), Some(r)) => m.grid_size_for(r),
            _ => GridSize { cols: 120, rows: 30 },
        }
    }

    /// Abre uma aba nova do modelo `kind` (menu "+", Ctrl+T): `claude`, e `claude@main@ana` se já há uma.
    pub fn open_tab(&mut self, kind: &str, restoring: bool) {
        self.open_tab_with_model(kind, None, restoring);
    }

    /// Como [`Deck::open_tab`], com o modelo de LLM escolhido no menu (`None` = o padrão do cargo ou do CLI).
    pub fn open_tab_with_model(&mut self, kind: &str, model: Option<&str>, restoring: bool) {
        if !restoring {
            self.show_agents();
        }
        let project = self.active_project.clone();
        if project.is_empty() {
            self.system("Abra uma pasta como projeto antes de abrir um agente.");
            self.new_project_dialog();
            return;
        }
        match self.core.register_instance_with_model(&project, kind, model) {
            Ok(ident) => self.add_tab(project, ident, restoring, false),
            Err(e) => self.system(format!("Não foi possível abrir {kind}: {e}.")),
        }
    }

    /// Pergunta de outro agente (`ask-agent`) para uma instância: abre a aba dela, se ainda não
    /// existe (em segundo plano, sem roubar o foco), e coloca a pergunta na fila para ser
    /// digitada no terminal assim que ele estiver pronto. A resposta volta quando o agente da
    /// aba rodar `ask-agent --reply <n> "resposta"`.
    ///
    /// Uma resposta atrasada (`d.answer`) vai para a aba de quem perguntou só se ela ainda está
    /// aberta e rodando: não reabre nada (a resposta já está na Conversa integrada).
    pub(crate) fn on_deliver(&mut self, d: Delivery) {
        let id = d.ident.id();
        // Um projeto que ainda não foi aberto nesta sessão abre as abas salvas (escondidas) antes.
        if !d.answer {
            self.load_project(&d.project);
        }
        let found = self.tabs.iter().position(|t| t.project == d.project && t.ident.id() == id);
        if d.answer {
            match found {
                // Num terminal comum o texto seria executado como comando (a ponte já evita isso).
                Some(i) if self.tabs[i].alive && self.tabs[i].kind != "shell" => {
                    self.tabs[i].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, answer: true })
                }
                _ => {
                    log::warn!("resposta atrasada #{} para \"{id}\" descartada: a aba não está aberta", d.request);
                    self.core.answer_lost(&d.project, d.request, "a aba não estava aberta");
                }
            }
            return;
        }
        // Aba nova ou processo encerrado: o CLI sobe já com o prompt inicial da tarefa (linha de comando), em vez de o
        // texto ser digitado num terminal que ainda está subindo e se perder.
        let boots = d.boot.as_deref().filter(|_| found.is_none_or(|i| !self.tabs[i].alive));
        if let Some(line) = boots {
            self.core.set_boot_prompt(&d.project, &id, line.to_owned());
        }
        let idx = match found {
            Some(i) => i,
            None => {
                // A aba de uma tarefa não conta no limite das abas abertas por outros agentes: o usuário a pediu.
                let task = self.core.task_of_instance(&d.project, &id);
                let reason = if !self.agents.knows(&d.ident.kind) {
                    Some(format!("o agente \"{}\" não está disponível", d.ident.kind))
                } else if task.is_none() && self.tabs.iter().filter(|t| t.auto).count() >= MAX_AUTO_TABS {
                    Some(format!("limite de {MAX_AUTO_TABS} abas abertas automaticamente; feche alguma"))
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
                    Some(t) => self.system_in(&d.project, format!("Abri a aba de {label} para a tarefa {}: {}.", t.id, t.title)),
                    None => self.system_in(&d.project, format!("Abri a aba de {label} para a conversa com outro agente.")),
                }
                self.tabs.len() - 1
            }
        };
        if !self.tabs[idx].alive {
            self.start(idx); // o processo tinha encerrado: sobe de novo para receber a pergunta
        }
        // O prompt inicial é de uma abertura só: sobrou (CLI não o aceitou, ou a aba já estava viva) = digita o texto.
        let booted = boots.is_some() && self.core.take_boot_prompt(&d.project, &id).is_none() && self.tabs[idx].alive;
        if !booted {
            self.tabs[idx].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, answer: false });
        }
    }

    /// Mensagem da própria interface (aba GitHub) para uma instância do projeto ativo: abre a aba
    /// se ela não existir (ou, com `force_new`, abre outra do mesmo cargo) e enfileira o texto,
    /// que é digitado quando a aba fica livre (mesma regra de `ask-agent`, em `delivery`).
    /// Devolve o rótulo da aba de destino ou o motivo da recusa.
    pub fn send_to_instance(&mut self, want: &Ident, force_new: bool, prompt: String) -> Result<String, String> {
        let project = self.active_project.clone();
        if project.is_empty() {
            return Err("Abra uma pasta como projeto antes de enviar para um agente.".into());
        }
        if !self.agents.knows(&want.kind) || want.kind == "shell" {
            return Err(format!("O agente \"{}\" não está disponível.", want.kind));
        }
        self.load_project(&project);
        let ident = match self.core.resolve_instance(&project, want, force_new) {
            Ok((ident, _)) => ident,
            Err(options) => return Err(format!("\"{}\" é ambíguo: {}. Escolha uma delas.", want.id(), options.join(", "))),
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
        self.tabs[idx].queue.push(Instant::now(), Queued { request: 0, prompt: prompt.clone(), answer: false });
        self.add_feed(FeedEntry::new(FeedKind::User, Some("você"), Some(&id), prompt));
        Ok(self.tabs[idx].label.clone())
    }

    /// Mostra a aba do agente de uma tarefa. Se ela foi fechada e a tarefa ainda está em andamento, em
    /// revisão ou aguardando validação, registra a instância de novo e abre a aba: o terminal abre na worktree da tarefa
    /// (`spawn_spec_for`) com o modelo da tarefa. Devolve o motivo da recusa.
    pub fn open_task_tab(&mut self, task: &agentdeck_core::Task) -> Result<(), String> {
        use agentdeck_core::TaskStatus;
        let project = self.active_project.clone();
        self.show_agents();
        if let Some(i) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == task.instance) {
            self.activate(i);
            return Ok(());
        }
        if !matches!(task.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating) {
            return Err(format!("A tarefa {} está {}: não há agente rodando.", task.id, task.status.label()));
        }
        let Some(ident) = Ident::parse(&task.instance).filter(|i| self.agents.knows(&i.kind)) else {
            return Err(format!("O agente \"{}\" desta tarefa não está disponível.", task.kind));
        };
        self.core.restore_instance(&project, &task.instance);
        self.core.assign_model(&project, &ident, task.model.as_deref());
        self.add_tab(project, ident, false, false);
        Ok(())
    }

    /// O que a regra de entrega precisa saber da aba.
    pub(super) fn tab_state(&self, idx: usize) -> TabState {
        let tab = &self.tabs[idx];
        TabState {
            alive: tab.alive,
            // Um envio cujo Enter ainda não saiu também ocupa a aba.
            busy: tab.busy || !tab.pending_enter.is_empty(),
            hooked: tab.hooked,
            spawned_at: tab.spawned_at,
            last_output: *tab.session.shared.last_output.lock(),
            last_key: tab.last_key,
            typing: tab.typing,
        }
    }

    /// Hook do agente (`ask-agent --signal`): início ou fim de turno.
    pub(crate) fn on_agent_signal(&mut self, project: &str, id: &str, busy: bool) {
        let Some((idx, tab)) = self.tabs.iter_mut().enumerate().find(|(_, t)| t.project == project && t.ident.id() == id) else {
            return;
        };
        tab.hooked = true;
        tab.turn_confirmed = busy;
        // Ocupada: se o aviso de fim de turno se perder, a aba é liberada após HOOK_STALE.
        tab.idle_deadline = busy.then(|| Instant::now() + HOOK_STALE);
        // Turno confirmado pelo hook: se o aviso de fim se perder, a liberação ainda conta como fim.
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

    /// Cria a aba da instância e inicia o processo. Uma aba `auto` fica em segundo plano (só ganha
    /// o foco se não houver nenhuma outra).
    fn add_tab(&mut self, project: String, ident: Ident, restoring: bool, auto: bool) {
        let kind = ident.kind.clone();
        let label = ident.label(&self.agents.name(&kind));
        // Pedido > gravado > padrão do cargo: grava para aparecer na barra lateral e voltar com o app.
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
        };
        self.next_uid += 1;
        // A aba de outro projeto roda escondida: não mexe nas janelas nem no foco.
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

    /// `start(tab)`: reinicia o terminal e o processo.
    pub fn start(&mut self, idx: usize) {
        let wake = self.wake.clone();
        let grid = self.current_grid(&self.tabs[idx].kind);
        let ident = self.tabs[idx].ident.clone();
        let spec = match self.core.spawn_spec_for(&self.tabs[idx].project, &ident) {
            Ok(spec) => Ok(spec),
            Err(e) if self.debug_fake_shell => {
                log::warn!("AGENTDECK_DEBUG_FAKE_SHELL: usando shell local no lugar de '{}' ({e})", self.tabs[idx].kind);
                let cwd = self.core.project_path(&self.tabs[idx].project).unwrap_or_else(|| self.workspace.clone());
                Ok(debug_fake_spec(&self.tabs[idx].kind, &cwd))
            }
            Err(e) => Err(e),
        };
        let tab = &mut self.tabs[idx];
        tab.session.reset();
        // Processo novo: os hooks precisam se apresentar de novo, e nada do anterior vale.
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
                tab.model = spec.model.clone();
                tab.spec = Some(spec);
            }
            Err(msg) => {
                tab.alive = false;
                tab.spec = None;
                let msg = msg.replace("\r\n", "\n").replace('\n', "\r\n");
                tab.session.print(&format!("\x1b[33m{msg}\x1b[0m\r\n\r\n\x1b[90m[pressione Enter para tentar de novo]\x1b[0m\r\n"));
            }
        }
    }

    pub fn activate(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        // Aba de outro projeto (ex.: clique na árvore da barra lateral): o projeto dela vem para a tela.
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
        // No Destaque a janela grande segue a aba ativa: a troca muda a largura das janelas, e o PTY
        // espera mais para o CLI terminar de reimprimir (ver `Deck::fit_tab`).
        if self.organization == Organization::Highlight {
            self.highlight_switch_until = Some(Instant::now() + HIGHLIGHT_RESIZE_DEBOUNCE);
        }
        // Lendo documentos, a aba ativada continua fora da vista: o selo da chave segue valendo.
        if self.docs.view == CenterView::Agents {
            self.tabs[idx].done = false;
        }
        self.set_term_focus(true);
        self.persist_tabs();
    }

    /// Mantém `group_order` igual aos grupos abertos: um grupo novo entra no fim (vira uma janela
    /// nova) e sai quando a última aba dele fecha.
    pub(super) fn sync_groups(&mut self) {
        let kinds: Vec<String> = self.visible().into_iter().map(|i| self.tabs[i].kind.clone()).collect();
        split::sync_order(&mut self.group_order, kinds.iter().map(String::as_str));
    }

    /// Janelas deste quadro na área `area`: a grade automática com um grupo por janela (Dividida), a
    /// janela do CLI selecionado grande com as outras empilhadas ao lado (Destaque) ou uma janela só
    /// (Abas). Guarda o resultado em `panes` para os atalhos. No Destaque, a janela 0 é a grande.
    pub fn plan_panes(&mut self, area: egui::Rect) -> (Vec<usize>, Vec<PanePlan>) {
        let many = self.group_order.len() > 1;
        let (shape, rects, groups) = if self.organization == Organization::Split && many {
            let shape = split::grid_shape(self.group_order.len(), area.width(), area.height());
            // Só ajusta para desenhar: as proporções salvas mudam apenas quando o usuário arrasta.
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
                // Uma janela só: todas as abas na ordem de abertura (como sem divisão).
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

    /// A aba ativada mais recentemente entre `tabs` (empate: a primeira).
    fn most_recent(&self, tabs: impl Iterator<Item = usize>) -> Option<usize> {
        let shown: Vec<u64> = self.tabs.iter().map(|t| t.shown).collect();
        split::most_recent(&shown, tabs)
    }

    /// Janela (do último quadro) que tem a aba ativa.
    pub(super) fn focused_slot(&self) -> Option<usize> {
        let uid = self.active_tab()?.uid;
        self.panes.iter().position(|p| p.uids.contains(&uid))
    }

    fn slot_tabs(&self, slot: usize) -> Vec<usize> {
        self.panes[slot].uids.iter().filter_map(|u| self.tabs.iter().position(|t| t.uid == *u)).collect()
    }

    /// Abas da janela focada (as do projeto, se ainda não houve quadro).
    pub(super) fn focused_pane_tabs(&self) -> Vec<usize> {
        match self.focused_slot() {
            Some(s) => self.slot_tabs(s),
            None => self.visible(),
        }
    }

    /// Foco na janela vizinha (Ctrl+Alt+setas): ativa a aba visível dela.
    pub fn focus_pane(&mut self, dx: i32, dy: i32) {
        let Some(from) = self.focused_slot() else { return };
        let rects: Vec<egui::Rect> = self.panes.iter().map(|p| p.rect).collect();
        if let Some(to) = split::neighbor(&rects, from, dx, dy) {
            if let Some(i) = self.most_recent(self.slot_tabs(to).into_iter()) {
                self.activate(i);
            }
        }
    }

    /// Troca duas janelas de lugar (arrastando a alça de uma sobre a outra).
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

    /// Muda como as janelas se arrumam (Abas, Dividida ou Destaque).
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
                log::warn!("resposta atrasada #{} perdida: a aba foi fechada antes de recebê-la", q.request);
                self.core.answer_lost(&tab.project, q.request, "a aba foi fechada antes de recebê-la");
            } else if q.request != 0 {
                self.core.fail_delivery(q.request, "a aba foi fechada antes de receber a mensagem");
            }
        }
        // Esquece a instância e libera quem esperava resposta dela.
        self.core.close_instance(&tab.project, &tab.ident.id());
        // Índices guardados entre quadros (modal de renomear, menu de contexto) seguem a aba deles.
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
                // Fica na mesma janela (a aba do grupo usada por último); sem nenhuma, a última usada.
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

    /// Pasta das gravações de diagnóstico do terminal (`dados/pty`).
    pub fn pty_dir(&self) -> PathBuf {
        self.core.paths().data_dir.join("pty")
    }

    /// Liga ou desliga a gravação de diagnóstico do terminal da aba `idx` (item do menu da aba) e avisa onde ficam os arquivos.
    pub fn toggle_terminal_recording(&mut self, idx: usize) {
        let dir = self.pty_dir();
        let Some(tab) = self.tabs.get_mut(idx) else { return };
        let id = tab.ident.id();
        if tab.session.recording() {
            tab.session.set_recording(None);
            self.system(format!("Gravação do terminal de {id} desligada. Os arquivos ficam em {}.", dir.display()));
        } else {
            tab.session.set_recording(Some((dir.clone(), id.clone())));
            self.system(format!(
                "Gravando o terminal de {id} daqui em diante em {} (três arquivos por gravação: .bin, .events.txt e .grid.txt). Quando o defeito aparecer, use \"Salvar foto da grade agora\" no menu da aba (botão direito) e mande esses arquivos.",
                dir.display()
            ));
        }
    }

    /// Salva a grade do terminal da aba `idx` agora e abre a pasta (item do menu da aba).
    pub fn save_terminal_snapshot(&mut self, idx: usize) {
        let dir = self.pty_dir();
        let Some(tab) = self.tabs.get(idx) else { return };
        let id = tab.ident.id();
        match tab.session.save_snapshot(&dir, &id) {
            Ok(path) => {
                self.system(format!("Foto da grade do terminal de {id} salva em {}.", path.display()));
                open_in_file_manager(&dir);
            }
            Err(e) => self.system(format!("Não consegui salvar a foto da grade do terminal de {id}: {e}")),
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
            return Err("aba não encontrada".into());
        }
        let old_id = self.tabs[idx].ident.id();
        let kind = self.tabs[idx].kind.clone();
        let trimmed = new_text.trim();
        // Com "@" o texto já é um id completo (o modelo é conferido abaixo); sem, é só o cargo.
        let full_text = if trimmed == kind || trimmed.contains('@') {
            trimmed.to_owned()
        } else if trimmed.is_empty() {
            kind.clone()
        } else {
            format!("{kind}@{trimmed}")
        };
        let Some(new_ident) = Ident::parse(&full_text) else {
            return Err(format!("formato inválido: \"{trimmed}\". Use modelo@cargo@nome (ex.: {kind}@dev ou {kind}@dev@ana)"));
        };
        if new_ident.kind != kind {
            return Err(format!("o modelo não pode ser alterado de \"{kind}\" para \"{}\"", new_ident.kind));
        }
        let new_id = new_ident.id();
        if new_id != old_id {
            self.core.rename_instance(&self.tabs[idx].project, &old_id, &new_ident)?;
            let new_label = new_ident.label(&self.agents.name(&kind));
            self.tabs[idx].ident = new_ident;
            self.tabs[idx].label = new_label;
            // Atualiza a conversa integrada do projeto da aba: os itens guardam o nome de exibição
            // (`who_name`), e o "pensando" passa para o id novo (a próxima mensagem dele o apaga).
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

    /// Uso dos planos por modelo (o fictício em depuração).
    pub fn usage(&self) -> HashMap<String, agentdeck_core::AgentUsage> {
        match &self.debug_usage {
            Some(u) => u.clone(),
            None => self.core.usage_snapshot(),
        }
    }

    /// Cor de destaque: a da aba em foco; sem aba, um cinza claro neutro. É a única cor forte da
    /// interface (seleção, ação primária, divisor ativo) e nunca é uma cor fixa de marca.
    pub fn accent(&self) -> Color32 {
        self.active_tab().map(|t| t.color).unwrap_or(theme::pal().accent_neutral)
    }

    /// A cor da aba em foco, se houver (alimenta `theme::set_accent`).
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
        // Cada projeto carregado grava as abas dele; os que ainda não abriram nesta sessão ficam
        // como estão (senão perderiam as abas salvas).
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

    /// Fixa na barra de título o grupo `group` do agente `model` (`None` volta ao resumo) e grava.
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
        // O projeto da tela (o do core pode já ter mudado, ex.: ao remover o ativo).
        if self.active_project.is_empty() {
            return;
        }
        self.core.update_project(&self.active_project, |p| {
            p.split_rows = ratios.rows;
            p.split_cols = ratios.cols;
        });
    }

    /// Ajusta a grade de uma aba visível à área dela, quando o tamanho parar (ver [`resize_due`]).
    /// Trocar de aba no Destaque muda o tamanho das janelas sem o usuário arrastar nada, e ali o PTY
    /// ganha mais folga (ver [`HIGHLIGHT_RESIZE_DEBOUNCE`]).
    pub fn fit_tab(&mut self, idx: usize, rect: egui::Rect) {
        let Some(m) = self.metrics.clone() else { return };
        let size = m.grid_size_for(rect);
        let now = Instant::now();
        let delay = if self.highlight_switch_until.is_some_and(|until| now < until) { HIGHLIGHT_RESIZE_DEBOUNCE } else { RESIZE_DEBOUNCE };
        let tab = &mut self.tabs[idx];
        // Voltou ao tamanho que o PTY já tem (A→B→A dentro do prazo): não há o que redimensionar, e o
        // pendente do meio do caminho é cancelado em vez de virar um redimensionamento à toa.
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

/// Troca o nome de exibição de quem fala/para quem nos itens da Conversa (aba renomeada).
fn rename_in_feed(feed: &mut [FeedItem], old_who: &str, new_who: &str) {
    for item in feed {
        for who in [&mut item.who, &mut item.to] {
            if who.as_deref() == Some(old_who) {
                *who = Some(new_who.to_owned());
            }
        }
    }
}

/// O "pensando" de `old_id` no projeto passa a ser de `new_id` (chave `projeto#id`, ver `add_feed`).
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
        // O mesmo id em outro projeto não é dele.
        assert_eq!(thinking.get("q#claude"), Some(&9));
    }
}

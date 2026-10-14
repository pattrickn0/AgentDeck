//! Estado e lógica da interface (porta do `renderer.js`), independente da pintura.
//! A pintura fica em [`crate::ui`]; a janela/loop em [`crate::app`].

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use commanddeck_core::{AgentInfo, Core, CoreEvent, DirEntry, DownloadProgress, DownloadState, FeedEntry, FeedKind, SpawnSpec, TableRequest};
use crossbeam_channel::{Receiver, Sender};
use egui::Color32;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::agents::Agents;
use crate::audio;
use crate::icons::IconCache;
use crate::terminal::input::{self, Mods};
use crate::terminal::view::{Metrics, ViewState};
use crate::terminal::{GridSize, Session, Wake};
use crate::theme;
use crate::util;

/// Mensagens de tarefas em segundo plano para a UI.
pub enum UiMsg {
    Agents(Vec<AgentInfo>),
    Attached(Vec<String>),
    Transcribed(Result<String, String>),
    Engine(String),
    WorkspacePicked(PathBuf),
    TableDone,
}

/// Ações vindas da janela (atalhos globais) aplicadas no próximo quadro.
#[derive(Debug, Clone)]
pub enum Action {
    NewTabShortcut,
    ToggleSidebar,
    CycleTab { back: bool },
    FocusComposer,
    CopyText(String),
}

pub struct Tab {
    pub uid: u64,
    pub kind: String,
    pub label: String,
    pub color: Color32,
    pub session: Session,
    pub view: ViewState,
    pub alive: bool,
    pub busy: bool,
    /// Aberta pelo app a pedido de `ask-agent` (e não pelo usuário).
    pub auto: bool,
    seen_output: u64,
    idle_deadline: Option<Instant>,
    spec: Option<SpawnSpec>,
    pending_resize: Option<(GridSize, Instant)>,
}

pub struct FeedItem {
    pub id: u64,
    pub kind: FeedKind,
    pub who: Option<String>,
    pub to: Option<String>,
    pub text: String,
    pub time: String,
    pub color: Color32,
}

/// Linha achatada da árvore de arquivos.
pub struct TreeRow {
    pub entry: DirEntry,
    pub depth: usize,
    pub expanded: bool,
}

#[derive(Default)]
pub struct TreeState {
    pub expanded: HashSet<String>,
    cache: HashMap<String, Vec<DirEntry>>,
    pub active_file: Option<String>,
    /// Clique simples pendente (debounce de 220 ms para distinguir do duplo clique).
    pending_click: Option<(String, Instant)>,
}

pub struct Deck {
    pub core: Core,
    events: Receiver<CoreEvent>,
    tx: Sender<UiMsg>,
    rx: Receiver<UiMsg>,
    pub wake: Wake,
    pub agents: Agents,
    pub tabs: Vec<Tab>,
    pub active: Option<usize>,
    next_uid: u64,
    pub initialized: bool,
    restoring: bool,
    persist_at: Option<Instant>,
    layout_persist_at: Option<Instant>,
    actions: Vec<Action>,

    // Layout
    pub sidebar_w: f32,
    pub sidebar_collapsed: bool,
    pub bottom_h: f32,
    pub feed_collapsed: bool,
    pub sidebar_dragging: Option<(f32, f32)>,
    pub row_dragging: Option<(f32, f32, bool)>,

    // Árvore
    pub workspace: PathBuf,
    pub tree: TreeState,

    // Conversa integrada
    pub feed: Vec<FeedItem>,
    thinking: HashMap<String, u64>,
    next_feed_id: u64,
    pub unseen: u32,
    /// Download em andamento (barra no cabeçalho) e, se for um erro, quando some.
    pub download: Option<(DownloadProgress, Option<Instant>)>,
    /// SOMENTE DEPURAÇÃO: mantém o estado fictício de `COMMANDDECK_DEBUG_UI=download*`, ignorando o download real.
    pub debug_hold_download: bool,
    pub feed_scroll_to_bottom: bool,
    pub feed_near_bottom: bool,

    // Compositor
    pub input: String,
    pub attachments: Vec<String>,
    pub target: String,
    pub rounds: String,
    pub needs_text: bool,
    pub table_running: bool,
    recorder: Option<audio::Recorder>,
    pub recording_since: Option<Instant>,
    pub transcribing: bool,
    pub send_after_transcribe: bool,
    pub engine_label: String,
    pub focus_composer: bool,
    pub composer_focused: bool,
    pub composer_cursor_end: bool,

    // Menus e diversos
    pub new_tab_menu_open: bool,
    pub settings_open: bool,
    pub target_menu_open: bool,
    pub sound_enabled: bool,
    /// Espelho de `Config.auto_open_agent_tabs` (item do menu de configurações).
    pub auto_open_agent_tabs: bool,
    /// Agentes cuja aba automática o usuário fechou: não reabrem até fechar o app.
    dismissed_agent_tabs: HashSet<String>,
    pub drop_hover: bool,
    pub term_focus: bool,
    /// Texto em composição do IME (pré-edição), mostrado sobre o cursor do terminal.
    pub ime_preedit: String,
    pub metrics: Option<Metrics>,
    pub blink_epoch: Instant,
    pub icons: IconCache,
    pub terminal_rect: Option<egui::Rect>,
    pub window_focused: bool,
    debug_fake_shell: bool,
    /// SOMENTE DEPURAÇÃO: fecha as abas restauradas (captura do estado vazio).
    pub debug_close_all: bool,
}

const PERSIST_DEBOUNCE: Duration = Duration::from_millis(120);
/// Quantas abas o app abre sozinho (a pedido de `ask-agent`) ao mesmo tempo; passou disso, só o feed.
const MAX_AUTO_TABS: usize = 3;
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(60);
const IDLE_CHIME: Duration = Duration::from_millis(1200);
const TREE_CLICK_DELAY: Duration = Duration::from_millis(220);

impl Deck {
    pub fn new(core: Core, events: Receiver<CoreEvent>, wake: Wake) -> Self {
        let cfg = core.config();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut deck = Self {
            workspace: core.workspace(),
            core,
            events,
            tx,
            rx,
            wake,
            agents: Agents::new(),
            tabs: Vec::new(),
            active: None,
            next_uid: 1,
            initialized: false,
            restoring: false,
            persist_at: None,
            layout_persist_at: None,
            actions: Vec::new(),
            sidebar_w: if cfg.sidebar_width > 0.0 { cfg.sidebar_width.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W) } else { theme::SIDEBAR_DEFAULT_W },
            sidebar_collapsed: cfg.sidebar_collapsed,
            bottom_h: if cfg.bottom_height > 0.0 { cfg.bottom_height } else { theme::BOTTOM_DEFAULT_H },
            feed_collapsed: cfg.feed_collapsed,
            sidebar_dragging: None,
            row_dragging: None,
            tree: TreeState { expanded: HashSet::from([String::new()]), ..Default::default() },
            feed: Vec::new(),
            thinking: HashMap::new(),
            next_feed_id: 1,
            unseen: 0,
            download: None,
            debug_hold_download: false,
            feed_scroll_to_bottom: false,
            feed_near_bottom: true,
            input: String::new(),
            attachments: Vec::new(),
            target: "active".into(),
            rounds: "2".into(),
            needs_text: false,
            table_running: false,
            recorder: None,
            recording_since: None,
            transcribing: false,
            send_after_transcribe: false,
            engine_label: String::new(),
            focus_composer: false,
            composer_focused: false,
            composer_cursor_end: false,
            new_tab_menu_open: false,
            settings_open: false,
            target_menu_open: false,
            sound_enabled: cfg.sound_enabled,
            auto_open_agent_tabs: cfg.auto_open_agent_tabs,
            dismissed_agent_tabs: HashSet::new(),
            drop_hover: false,
            term_focus: false,
            ime_preedit: String::new(),
            metrics: None,
            blink_epoch: Instant::now(),
            icons: IconCache::default(),
            terminal_rect: None,
            window_focused: true,
            debug_close_all: false,
            debug_fake_shell: std::env::var("COMMANDDECK_DEBUG_FAKE_SHELL").is_ok_and(|v| v == "1"),
        };
        deck.refresh_agents();
        deck.refresh_engine();
        if std::env::var("COMMANDDECK_DEBUG_FEED").is_ok_and(|v| v == "1") {
            deck.debug_feed();
        }
        deck
    }

    fn spawn_bg(&self, name: &str, f: impl FnOnce(Core) -> Option<UiMsg> + Send + 'static) {
        let core = self.core.clone();
        let tx = self.tx.clone();
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new().name(name.into()).spawn(move || {
            if let Some(msg) = f(core) {
                let _ = tx.send(msg);
                wake();
            }
        });
    }

    /// `api.listAgents()` numa thread (pode demorar ~1,5 s consultando o Ollama).
    pub fn refresh_agents(&self) {
        self.spawn_bg("list-agents", |core| Some(UiMsg::Agents(core.list_agents())));
    }

    fn refresh_engine(&self) {
        self.spawn_bg("audio-engine", |core| Some(UiMsg::Engine(core.transcribe_engine())));
    }

    pub fn push_action(&mut self, a: Action) {
        self.actions.push(a);
    }

    // ------------------------------------------------------------------
    // Quadro
    // ------------------------------------------------------------------

    /// Processa eventos, mensagens e temporizadores antes de pintar.
    pub fn before_frame(&mut self, ctx: &egui::Context) {
        if self.metrics.is_none() {
            self.metrics = Some(Metrics::measure(ctx));
        }
        for a in std::mem::take(&mut self.actions) {
            self.apply_action(ctx, a);
        }
        while let Ok(ev) = self.events.try_recv() {
            self.on_core_event(ev);
        }
        while let Ok(msg) = self.rx.try_recv() {
            self.on_msg(msg);
        }
        self.tick(ctx);
    }

    fn apply_action(&mut self, ctx: &egui::Context, a: Action) {
        match a {
            Action::NewTabShortcut => {
                let kind = self.active.map(|i| self.tabs[i].kind.clone()).unwrap_or_else(|| self.agents.fallback_kind());
                self.open_tab(&kind, false);
            }
            Action::ToggleSidebar => self.toggle_sidebar(),
            Action::CycleTab { back } => {
                let n = self.tabs.len();
                if n > 0 {
                    let cur = self.active.unwrap_or(0) as isize;
                    let next = (cur + if back { -1 } else { 1 } + n as isize).rem_euclid(n as isize) as usize;
                    self.activate(next);
                }
            }
            Action::FocusComposer => self.request_composer_focus(),
            Action::CopyText(t) => ctx.copy_text(t),
        }
    }

    fn on_core_event(&mut self, ev: CoreEvent) {
        match ev {
            CoreEvent::Feed(entry) => {
                let chime = entry.kind == FeedKind::Answer || (entry.kind == FeedKind::System && entry.text.contains("Mesa encerrada"));
                self.add_feed(entry);
                if chime {
                    self.play_sound(false);
                }
            }
            CoreEvent::WorkspaceChanged => self.refresh_tree(),
            CoreEvent::TableFinished => self.finish_table(),
            CoreEvent::AgentsChanged => self.refresh_agents(),
            CoreEvent::Download(_) if self.debug_hold_download => {}
            CoreEvent::EnsureAgentTab { agent } => self.ensure_agent_tab(&agent),
            CoreEvent::Download(p) => self.download = match p.state {
                DownloadState::Done => None,
                // O erro fica visível por alguns segundos (o detalhe também vai para a Conversa integrada).
                DownloadState::Error => Some((p, Some(Instant::now() + Duration::from_secs(8)))),
                _ => Some((p, None)),
            },
        }
    }

    fn on_msg(&mut self, msg: UiMsg) {
        match msg {
            UiMsg::Agents(list) => {
                self.agents.build(&list);
                if !self.agents.has_option(&self.target) {
                    self.target = "active".into();
                }
                for t in &mut self.tabs {
                    t.color = self.agents.tab_color(&t.kind);
                }
                if !self.initialized {
                    self.initialized = true;
                    self.restore_tabs();
                    if self.debug_close_all {
                        while !self.tabs.is_empty() {
                            self.close_tab(0);
                        }
                    }
                }
            }
            UiMsg::Attached(refs) => {
                if !refs.is_empty() {
                    self.attachments.extend(refs);
                    self.request_composer_focus();
                }
            }
            UiMsg::Transcribed(res) => {
                self.transcribing = false;
                let mut ok = false;
                match res {
                    Ok(text) => {
                        let text = text.trim();
                        self.input = if self.input.is_empty() { text.to_owned() } else { format!("{} {text}", self.input) };
                        self.composer_cursor_end = true;
                        ok = true;
                    }
                    Err(e) => self.system(format!("Não foi possível transcrever: {e}.")),
                }
                self.request_composer_focus();
                self.refresh_engine();
                let send = self.send_after_transcribe && ok;
                self.send_after_transcribe = false;
                if send {
                    self.send_message();
                }
            }
            UiMsg::Engine(label) => self.engine_label = label,
            UiMsg::WorkspacePicked(dir) => {
                if let Err(e) = self.core.set_workspace(dir) {
                    self.system(format!("Não foi possível trocar a pasta de trabalho: {e}"));
                }
                self.workspace = self.core.workspace();
                self.refresh_tree();
            }
            UiMsg::TableDone => self.finish_table(),
        }
    }

    fn finish_table(&mut self) {
        self.table_running = false;
    }

    /// Temporizadores (setTimeout do renderer).
    fn tick(&mut self, ctx: &egui::Context) {
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
        if let Some((rel, at)) = self.tree.pending_click.clone() {
            if now >= at + TREE_CLICK_DELAY {
                self.tree.pending_click = None;
                self.tree.active_file = Some(rel.clone());
                self.input = util::insert_ref(&self.input, &rel);
                self.composer_cursor_end = true;
                self.request_composer_focus();
            } else {
                want(at + TREE_CLICK_DELAY);
            }
        }

        let mut chime = false;
        let metrics = self.metrics.clone();
        let mut exits = Vec::new();
        for (i, tab) in self.tabs.iter_mut().enumerate() {
            if let Some(code) = tab.session.take_exit() {
                exits.push((i, code));
            }
            if tab.session.take_bell() {
                chime = true;
            }
            if let Some(text) = tab.session.take_clipboard() {
                ctx.copy_text(text);
            }
            let seq = tab.session.shared.output_seq.load(std::sync::atomic::Ordering::Relaxed);
            if seq != tab.seen_output {
                tab.seen_output = seq;
                if tab.busy {
                    let last = (*tab.session.shared.last_output.lock()).unwrap_or(now);
                    tab.idle_deadline = Some(last + IDLE_CHIME);
                }
            }
            if let Some(at) = tab.idle_deadline {
                if now >= at {
                    tab.idle_deadline = None;
                    if tab.busy {
                        tab.busy = false;
                        chime = true;
                    }
                } else {
                    want(at);
                }
            }
            if let Some((size, at)) = tab.pending_resize {
                if now >= at {
                    tab.pending_resize = None;
                    if let Some(m) = &metrics {
                        tab.session.resize(size, m.cell_w, m.cell_h);
                    }
                } else {
                    want(at);
                }
            }
        }
        for (i, code) in exits {
            let tab = &mut self.tabs[i];
            tab.alive = false;
            tab.session.print(&format!("\r\n\x1b[90m[processo encerrado (código {code}) — pressione Enter para reiniciar]\x1b[0m\r\n"));
        }
        if chime {
            self.play_sound(false);
        }
        if let Some(t) = next {
            ctx.request_repaint_after(t.saturating_duration_since(now));
        }
    }

    pub fn play_sound(&self, force: bool) {
        if self.sound_enabled || force {
            audio::play_chime();
        }
    }

    // ------------------------------------------------------------------
    // Abas
    // ------------------------------------------------------------------

    fn restore_tabs(&mut self) {
        let cfg = self.core.config();
        if !cfg.open_tabs.is_empty() {
            self.restoring = true;
            for kind in &cfg.open_tabs {
                if self.agents.knows(kind) {
                    self.open_tab(kind, true);
                }
            }
            let idx = if cfg.active_tab_index >= 0 && (cfg.active_tab_index as usize) < self.tabs.len() { cfg.active_tab_index as usize } else { 0 };
            if !self.tabs.is_empty() {
                self.activate(idx);
            }
            self.restoring = false;
        } else {
            self.open_tab("claude", false);
            self.open_tab("antigravity", false);
            if !self.tabs.is_empty() {
                self.activate(0);
            }
        }
        self.refresh_tree();
    }

    fn current_grid(&self) -> GridSize {
        match (&self.metrics, self.terminal_rect) {
            (Some(m), Some(r)) => m.grid_size_for(r),
            _ => GridSize { cols: 120, rows: 30 },
        }
    }

    pub fn open_tab(&mut self, kind: &str, restoring: bool) {
        // Abrir à mão devolve ao agente o direito de abrir abas sozinho.
        self.dismissed_agent_tabs.remove(kind);
        self.add_tab(kind, restoring, false);
    }

    /// `ask-agent` chamou `agent`: garante uma aba (terminal normal) para ele, em segundo plano e
    /// sem roubar o foco. A conversa da ponte continua indo pelo modo headless; a aba é só o
    /// terminal do agente, pronto para o usuário usar. Respeita a opção do usuário, abas que ele
    /// fechou e um teto de abas automáticas.
    fn ensure_agent_tab(&mut self, agent: &str) {
        if !self.auto_open_agent_tabs
            || !self.agents.knows(agent)
            || self.tabs.iter().any(|t| t.kind == agent)
            || self.dismissed_agent_tabs.contains(agent)
            || self.tabs.iter().filter(|t| t.auto).count() >= MAX_AUTO_TABS
        {
            return;
        }
        self.add_tab(agent, false, true);
        self.system(format!("Abri a aba de {} para a conversa com outro agente.", self.agents.name(agent)));
    }

    /// Cria a aba e inicia o processo. Uma aba `auto` fica em segundo plano (só ganha o foco se
    /// não houver nenhuma outra).
    fn add_tab(&mut self, kind: &str, restoring: bool, auto: bool) {
        let same_kind = self.tabs.iter().filter(|t| t.kind == kind).count();
        let mut label = self.agents.name(kind);
        if same_kind > 0 {
            label = format!("{label} {}", same_kind + 1);
        }
        let tab = Tab {
            uid: self.next_uid,
            kind: kind.to_owned(),
            label,
            color: self.agents.tab_color(kind),
            session: Session::new(self.current_grid()),
            view: ViewState::default(),
            alive: false,
            busy: false,
            auto,
            seen_output: 0,
            idle_deadline: None,
            spec: None,
            pending_resize: None,
        };
        self.next_uid += 1;
        self.tabs.push(tab);
        let idx = self.tabs.len() - 1;
        if !auto || self.active.is_none() {
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
        let grid = self.current_grid();
        let spec = match self.core.spawn_spec(&self.tabs[idx].kind) {
            Ok(spec) => Ok(spec),
            Err(e) if self.debug_fake_shell => {
                log::warn!("COMMANDDECK_DEBUG_FAKE_SHELL: usando shell local no lugar de '{}' ({e})", self.tabs[idx].kind);
                Ok(debug_fake_spec(&self.tabs[idx].kind, &self.workspace))
            }
            Err(e) => Err(e),
        };
        let tab = &mut self.tabs[idx];
        tab.session.reset();
        if let Some(m) = &self.metrics {
            tab.session.resize(grid, m.cell_w, m.cell_h);
        }
        match spec.and_then(|s| tab.session.spawn(&s, wake).map(|_| s).map_err(|e| e.to_string())) {
            Ok(spec) => {
                tab.alive = true;
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
        self.active = Some(idx);
        self.set_term_focus(true);
        self.persist_tabs();
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        tab.session.kill();
        if tab.auto {
            // Fechou uma aba que o app abriu sozinho: não insiste até o próximo início.
            self.dismissed_agent_tabs.insert(tab.kind.clone());
        }
        match self.active {
            Some(a) if a == idx => {
                if self.tabs.is_empty() {
                    self.active = None;
                } else {
                    self.activate(idx.min(self.tabs.len() - 1));
                }
            }
            Some(a) if a > idx => self.active = Some(a - 1),
            _ => {}
        }
        self.persist_tabs_now();
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    /// `--active-agent-color` (sem aba, volta ao valor do `:root`, a cor do Claude).
    pub fn active_color(&self) -> Color32 {
        self.active_tab().map(|t| t.color).unwrap_or(theme::CLAUDE)
    }

    fn persist_tabs(&mut self) {
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
        let open: Vec<String> = self.tabs.iter().map(|t| t.kind.clone()).collect();
        let idx = self.active.map(|i| i as i32).unwrap_or(-1);
        self.core.update_config(|c| {
            c.open_tabs = open;
            c.active_tab_index = idx;
        });
    }

    pub fn persist_layout(&mut self) {
        self.layout_persist_at = Some(Instant::now() + Duration::from_millis(400));
    }

    pub fn persist_layout_now(&mut self) {
        self.layout_persist_at = None;
        let (sw, sc, bh, fc) = (self.sidebar_w, self.sidebar_collapsed, self.bottom_h, self.feed_collapsed);
        self.core.update_config(|c| {
            c.sidebar_width = sw;
            c.sidebar_collapsed = sc;
            c.bottom_height = bh;
            c.feed_collapsed = fc;
        });
    }

    /// Ajusta a grade da aba ativa à área visível (debounce de 60 ms ao PTY).
    pub fn fit_active(&mut self, rect: egui::Rect) {
        self.terminal_rect = Some(rect);
        let (Some(m), Some(idx)) = (self.metrics.clone(), self.active) else { return };
        let size = m.grid_size_for(rect);
        let tab = &mut self.tabs[idx];
        let current = tab.pending_resize.map(|p| p.0).unwrap_or(tab.session.size());
        if size != current {
            tab.pending_resize = Some((size, Instant::now() + RESIZE_DEBOUNCE));
        }
    }

    pub fn set_term_focus(&mut self, on: bool) {
        self.term_focus = on;
        if on {
            self.blink_epoch = Instant::now();
        }
    }

    // ------------------------------------------------------------------
    // Teclado
    // ------------------------------------------------------------------

    /// IME com o terminal focado (teclados CJK, seletor de emoji `Win+.`): a confirmação vai ao
    /// processo como se tivesse sido digitada; a pré-edição só é desenhada, sem enviar nada.
    pub fn handle_ime(&mut self, ime: &winit::event::Ime) {
        use winit::event::Ime;
        match ime {
            Ime::Preedit(text, _) => self.ime_preedit = text.clone(),
            Ime::Commit(text) => {
                self.ime_preedit.clear();
                if let (false, Some(idx)) = (text.is_empty(), self.active) {
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
        if ev.state != ElementState::Pressed {
            return !egui_has_focus && self.term_focus && self.active.is_some();
        }
        let ctrl = mods.control_key();
        let lower = match &ev.logical_key {
            Key::Character(s) => s.to_lowercase(),
            _ => String::new(),
        };
        if ctrl && !mods.alt_key() {
            match lower.as_str() {
                "t" if !ev.repeat => {
                    self.push_action(Action::NewTabShortcut);
                    return true;
                }
                "b" if !ev.repeat => {
                    self.push_action(Action::ToggleSidebar);
                    return true;
                }
                "l" if !self.composer_focused => {
                    self.push_action(Action::FocusComposer);
                    return true;
                }
                "v" => self.check_clipboard_image(),
                _ => {}
            }
            if ev.logical_key == Key::Named(NamedKey::Tab) {
                self.push_action(Action::CycleTab { back: mods.shift_key() });
                return true;
            }
        }
        if egui_has_focus || !self.term_focus {
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
                        self.tabs[idx].session.write(&input::paste_bytes(&text, bracketed));
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
        let Some(bytes) = input::encode_key(&ev.logical_key, text, m, app_cursor) else { return true };
        self.terminal_data(idx, &bytes);
        true
    }

    /// `term.onData`: envia ao processo ou reinicia com Enter quando morto.
    fn terminal_data(&mut self, idx: usize, bytes: &[u8]) {
        let tab = &mut self.tabs[idx];
        if tab.alive {
            if bytes == b"\r" {
                tab.busy = true;
            }
            {
                let mut emu = tab.session.shared.emu.lock();
                emu.term.scroll_display(Scroll::Bottom);
                emu.term.selection = None;
            }
            tab.session.write(bytes);
        } else if bytes == b"\r" {
            self.start(idx);
        }
    }

    /// Escreve bytes gerados pelo mouse (roda em modo de relatório) no processo.
    pub fn terminal_mouse_bytes(&mut self, idx: usize, bytes: &[u8]) {
        if self.tabs[idx].alive && !bytes.is_empty() {
            self.tabs[idx].session.write(bytes);
        }
    }

    /// Se a área de transferência não tem texto mas tem imagem, anexa como PNG.
    ///
    /// O clipboard é lido aqui, na thread da UI (a mesma em que o egui o usa para colar no
    /// compositor): duas threads nele ao mesmo tempo corrompiam o heap do Windows (`0xC0000374`).
    /// Só a conversão para PNG e o anexo vão para segundo plano, já sem tocar no clipboard.
    fn check_clipboard_image(&self) {
        let Ok(mut cb) = arboard::Clipboard::new() else { return };
        if cb.get_text().is_ok_and(|t| !t.is_empty()) {
            return;
        }
        let Ok(img) = cb.get_image() else { return };
        let (width, height, bytes) = (img.width as u32, img.height as u32, img.bytes.into_owned());
        self.spawn_bg("clipboard-image", move |core| match core.attach_rgba_png(width, height, &bytes) {
            Ok(r) => Some(UiMsg::Attached(vec![r])),
            Err(e) => {
                log::warn!("colar imagem: {e}");
                None
            }
        });
    }

    // ------------------------------------------------------------------
    // Barra lateral e árvore
    // ------------------------------------------------------------------

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

    pub fn tree_file_double_click(&mut self, rel: &str) {
        self.tree.pending_click = None;
        if let Err(e) = self.core.open_path(rel) {
            log::warn!("abrir {rel}: {e}");
        }
    }

    pub fn pick_workspace(&self) {
        let start = self.workspace.clone();
        self.spawn_bg("pick-workspace", move |_| {
            let dir = rfd::FileDialog::new().set_directory(&start).pick_folder()?;
            Some(UiMsg::WorkspacePicked(dir))
        });
    }

    // ------------------------------------------------------------------
    // Conversa integrada
    // ------------------------------------------------------------------

    pub fn add_feed(&mut self, e: FeedEntry) {
        if let Some(from) = &e.from {
            if let Some(id) = self.thinking.remove(from) {
                self.feed.retain(|f| f.id != id);
            }
        }
        let to = e.to.as_ref().map(|t| t.split(" + ").map(|n| self.agents.who_name(n)).collect::<Vec<_>>().join(" + "));
        let item = FeedItem {
            id: self.next_feed_id,
            kind: e.kind,
            who: e.from.as_ref().map(|f| self.agents.who_name(f)),
            to,
            color: self.agents.who_color(e.from.as_deref()),
            time: util::time_pt_br(e.ts),
            text: e.text,
        };
        self.next_feed_id += 1;
        if e.kind == FeedKind::Thinking {
            if let Some(from) = &e.from {
                self.thinking.insert(from.clone(), item.id);
            }
        }
        self.feed.push(item);
        if self.feed_near_bottom {
            self.feed_scroll_to_bottom = true;
        }
        if self.feed_collapsed && e.kind != FeedKind::Thinking {
            self.unseen += 1;
        }
    }

    pub fn system(&mut self, text: impl Into<String>) {
        self.add_feed(FeedEntry { ts: 0, kind: FeedKind::System, from: None, to: None, text: text.into() });
    }

    pub fn clear_feed(&mut self) {
        self.feed.clear();
        self.thinking.clear();
    }

    pub fn toggle_feed(&mut self) {
        self.feed_collapsed = !self.feed_collapsed;
        if !self.feed_collapsed {
            self.unseen = 0;
        }
        self.persist_layout();
    }

    fn debug_feed(&mut self) {
        let mk = |kind, from: Option<&str>, to: Option<&str>, text: &str| FeedEntry::new(kind, from, to, text);
        self.add_feed(mk(FeedKind::User, Some("você"), Some("claude + antigravity"), "Revisem o @README.md e proponham melhorias."));
        self.add_feed(mk(FeedKind::Ask, Some("claude"), Some("antigravity"), "Você pode verificar se o build do Windows ainda usa o ConPTY?"));
        self.add_feed(mk(FeedKind::Answer, Some("antigravity"), Some("claude"), "Sim — portable-pty usa ConPTY no Windows 10+.\nNada a mudar."));
        self.add_feed(mk(FeedKind::System, None, None, "Mesa iniciada: Claude → Antigravity (2 rodadas)."));
        self.add_feed(mk(FeedKind::Error, Some("ollama:qwen3.5:9b"), None, "Falha ao contatar o Ollama em 127.0.0.1:11434."));
        self.add_feed(mk(FeedKind::Thinking, Some("claude"), None, "rodada 1/2…"));
    }

    // ------------------------------------------------------------------
    // Compositor / envio
    // ------------------------------------------------------------------

    pub fn request_composer_focus(&mut self) {
        self.focus_composer = true;
        self.term_focus = false;
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// `ready` do botão Enviar.
    pub fn send_ready(&self) -> bool {
        !self.input.trim().is_empty() || !self.attachments.is_empty() || self.is_recording() || self.transcribing || self.send_after_transcribe
    }

    /// `--send-color`: cor da aba ativa quando o alvo é "Aba ativa" e não é um shell.
    pub fn send_color(&self) -> Color32 {
        if self.target == "active" {
            if let Some(t) = self.active_tab() {
                if t.kind != "shell" {
                    if let Some(c) = self.agents.color(&t.kind) {
                        return c;
                    }
                }
            }
        }
        theme::SEND_DEFAULT
    }

    pub fn send_label(&self) -> &'static str {
        if self.send_after_transcribe && (self.is_recording() || self.transcribing) {
            "Enviando…"
        } else if self.table_running {
            "Parar"
        } else {
            "Enviar"
        }
    }

    pub fn set_target(&mut self, value: String) {
        self.target = value;
    }

    pub fn pick_attachments(&self) {
        self.spawn_bg("pick-attachments", |core| {
            let files = rfd::FileDialog::new().pick_files()?;
            Some(UiMsg::Attached(core.attach_files(&files)))
        });
    }

    pub fn attach_paths(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        self.spawn_bg("attach-files", move |core| Some(UiMsg::Attached(core.attach_files(&paths))));
    }

    /// Clique no microfone.
    pub fn toggle_mic(&mut self) {
        if self.recorder.is_some() {
            self.stop_recording();
            return;
        }
        match audio::Recorder::start() {
            Ok(r) => {
                self.recorder = Some(r);
                self.recording_since = Some(Instant::now());
            }
            Err(e) => self.system(format!("Microfone indisponível: {e}")),
        }
    }

    /// `recorder.stop()` → transcreve numa thread.
    fn stop_recording(&mut self) {
        let Some(rec) = self.recorder.take() else { return };
        self.recording_since = None;
        self.transcribing = true;
        self.spawn_bg("transcribe", move |core| {
            let pcm = rec.stop();
            let res = if pcm.is_empty() { Err("nenhum áudio capturado".to_string()) } else { core.transcribe_pcm16k(&pcm).map(|t| t.text).map_err(|e| e.to_string()) };
            Some(UiMsg::Transcribed(res))
        });
    }

    pub fn cancel_recording(&mut self) {
        if let Some(rec) = self.recorder.take() {
            rec.cancel();
        }
        self.recording_since = None;
        self.send_after_transcribe = false;
        self.request_composer_focus();
    }

    /// Porta de `sendMessage()`.
    pub fn send_message(&mut self) {
        if self.table_running {
            self.core.stop_table();
            return;
        }
        if self.recorder.is_some() || self.transcribing {
            self.send_after_transcribe = true;
            if self.recorder.is_some() {
                self.stop_recording();
            }
            return;
        }
        let text = self.input.trim().to_owned();
        if text.is_empty() && self.attachments.is_empty() {
            return;
        }
        // Anexo sozinho não vai: o agente leria o arquivo sem saber o que fazer, gastando tokens à toa.
        if text.is_empty() {
            self.system("Escreva uma instrução junto com os anexos (ex.: \"analisa essas imagens\").");
            self.needs_text = true;
            self.request_composer_focus();
            return;
        }
        let full = util::compose_full(&text, &self.attachments);
        let mode = self.target.clone();

        if mode == "active" {
            let Some(idx) = self.active.filter(|&i| self.tabs[i].alive) else {
                self.system("A aba ativa não tem um processo rodando.");
                return;
            };
            self.submit(idx, &full);
            self.set_term_focus(true);
        } else if mode == "all" {
            let idxs: Vec<usize> = (0..self.tabs.len()).filter(|&i| self.tabs[i].alive && self.tabs[i].kind != "shell").collect();
            if idxs.is_empty() {
                self.system("Nenhum agente aberto.");
                return;
            }
            for &i in &idxs {
                self.submit(i, &full);
            }
            let to = idxs.iter().map(|&i| self.tabs[i].kind.clone()).collect::<Vec<_>>().join(" + ");
            self.add_feed(FeedEntry::new(FeedKind::User, Some("você"), Some(&to), full.clone()));
        } else if let Some(ids) = mode.strip_prefix("table:") {
            let agents: Vec<String> = ids.split(',').map(str::to_owned).collect();
            let rounds = util::clamp_rounds(&self.rounds);
            let files = std::mem::take(&mut self.attachments);
            self.input.clear();
            if self.feed_collapsed {
                self.toggle_feed();
            }
            self.table_running = true;
            let req = TableRequest { task: text, agents, rounds, attachments: files };
            self.spawn_bg("table", move |core| {
                core.run_table(req);
                Some(UiMsg::TableDone)
            });
            return;
        }
        self.input.clear();
        self.attachments.clear();
    }

    /// `pty:submit`: cola entre colchetes e confirma com Enter após 250 ms (e de novo após
    /// 600 ms quando há menção `@` em CLIs com autocomplete de arquivo).
    fn submit(&mut self, idx: usize, text: &str) {
        let tab = &mut self.tabs[idx];
        tab.busy = true;
        tab.session.write(&input::submit_paste(text));
        let writer = tab.session.writer_handle();
        let double = text.contains('@') && tab.spec.as_ref().is_some_and(|s| s.double_enter_on_mention);
        let _ = std::thread::Builder::new().name("submit".into()).spawn(move || {
            std::thread::sleep(Duration::from_millis(250));
            writer(b"\r");
            if double {
                std::thread::sleep(Duration::from_millis(350));
                writer(b"\r");
            }
        });
    }

    /// Mantém o processo de cada aba encerrado ao fechar a janela.
    pub fn shutdown(&mut self) {
        self.persist_tabs_now();
        self.persist_layout_now();
        for t in &mut self.tabs {
            t.session.kill();
        }
    }
}

/// Lê texto da área de transferência do sistema. Só da thread da UI (ver `check_clipboard_image`).
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_FAKE_SHELL=1`): abre o shell do sistema quando o
/// backend ainda não sabe montar o comando do agente, para exercitar o renderizador do terminal.
fn debug_fake_spec(kind: &str, cwd: &std::path::Path) -> SpawnSpec {
    #[cfg(windows)]
    let program = PathBuf::from("powershell.exe");
    #[cfg(not(windows))]
    let program = PathBuf::from(std::env::var("SHELL").unwrap_or_else(|_| "/bin/bash".into()));
    let cwd = if cwd.is_dir() { cwd.to_path_buf() } else { std::env::current_dir().unwrap_or_default() };
    SpawnSpec {
        program,
        args: Vec::new(),
        cwd,
        env: std::env::vars().collect(),
        label: kind.to_owned(),
        color: commanddeck_core::agents::COLOR_SHELL.into(),
        kind: kind.to_owned(),
        double_enter_on_mention: false,
    }
}

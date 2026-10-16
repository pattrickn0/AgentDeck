//! Estado e lógica da interface (porta do `renderer.js`), independente da pintura.
//! A pintura fica em [`crate::ui`]; a janela/loop em [`crate::app`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use commanddeck_core::delivery::{self, Gate, Mailbox, TabState};
use commanddeck_core::{
    AgentInfo, Core, CoreEvent, Delivery, DirEntry, DownloadProgress, DownloadState, FeedEntry, FeedKind, Ident, SpawnSpec,
};
use crossbeam_channel::{Receiver, Sender};
use egui::Color32;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::agent_map::{self, AnswerInFlight, Link, LinkState, NodeState, Recent, Snapshot};
use crate::agents::Agents;
use crate::audio;
use crate::icons::IconCache;
use crate::split;
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
    /// Resposta do Git, com a geração do projeto em que foi pedida (ver `GitUi::generation`).
    Git(u64, crate::git_ui::GitReply),
}

/// Ações vindas da janela (atalhos globais) aplicadas no próximo quadro.
#[derive(Debug, Clone)]
pub enum Action {
    NewTabShortcut,
    ToggleSidebar,
    CycleTab {
        back: bool,
    },
    /// Ctrl+Alt+setas: foco para a janela vizinha da tela dividida.
    FocusPane {
        dx: i32,
        dy: i32,
    },
    FocusComposer,
    CopyText(String),
}

/// Mensagem de outro agente esperando para ser digitada no terminal da aba.
struct Queued {
    request: u64,
    prompt: String,
    /// Quem mandou (quem perguntou; ou quem respondeu, se `answer`).
    from: String,
    /// É uma resposta atrasada a uma pergunta desta aba, não uma pergunta nova.
    answer: bool,
}

/// O que a barra lateral direita mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightView {
    Conversa,
    Mapa,
    /// Repositório git do workspace (ver `git_ui.rs`).
    Git,
}

pub struct Tab {
    pub uid: u64,
    pub kind: String,
    /// Identidade da instância (`claude`, `claude@dev`, `claude@dev@ana`).
    pub ident: Ident,
    /// Id do projeto da instância (as instâncias do core são por projeto).
    pub project: String,
    pub label: String,
    pub color: Color32,
    pub session: Session,
    pub view: ViewState,
    pub alive: bool,
    pub busy: bool,
    /// Teve turno concluído enquanto não estava ativa (limpa ao focar a aba).
    pub done: bool,
    /// Quando a aba foi ativada pela última vez (contador): em cada janela da tela dividida
    /// aparece a mais recente.
    pub shown: u64,
    /// Aberta pelo app a pedido de `ask-agent` (e não pelo usuário).
    pub auto: bool,
    /// Perguntas de outros agentes aguardando a aba ficar pronta (uma por vez).
    queue: Mailbox<Queued>,
    /// Última tecla do usuário nesta aba e se ela deixou texto começado na linha.
    last_key: Option<Instant>,
    typing: bool,
    /// O agente avisa início/fim de turno por hook (`ask-agent --signal`): `busy` vem dele.
    hooked: bool,
    /// O hook confirmou que o turno atual começou: só o aviso de fim de turno libera a aba. Sem
    /// confirmação (ex.: Enter num menu, que não abre turno), vale a heurística da saída.
    turn_confirmed: bool,
    /// Enters agendados de um envio (`submit`), em ordem.
    pending_enter: VecDeque<Instant>,
    /// Teclas do usuário que chegaram enquanto um envio esperava o Enter: vão depois dele, para
    /// não entrar no meio da mensagem.
    held_keys: Vec<u8>,
    seen_output: u64,
    idle_deadline: Option<Instant>,
    /// Houve saída desde que a aba ficou ocupada (só então o fim do turno toca e marca "concluído").
    busy_output: bool,
    spec: Option<SpawnSpec>,
    pending_resize: Option<(GridSize, Instant)>,
}

/// Uma janela da tela dividida, como ficou no último quadro.
pub struct PaneSlot {
    /// Janela inteira (cabeçalho de abas + terminal).
    pub rect: egui::Rect,
    /// Área do terminal.
    pub body: egui::Rect,
    pub uids: Vec<u64>,
    /// Grupos da janela (mais de um quando não cabem todos e os que sobram dividem a última).
    pub groups: Vec<String>,
}

/// Um agente (aba) de um projeto, para a lista de projetos da barra lateral.
#[derive(Debug, Clone)]
pub struct AgentBadge {
    /// Id da instância (`claude@dev`).
    pub id: String,
    /// Chefe da instância (`None` = responde ao usuário). Vem do core (`Core::set_boss`).
    pub parent: Option<String>,
    /// Modelo (`claude`, `antigravity`, `ollama:…`): escolhe o logo.
    pub kind: String,
    /// Nome mostrado (`Claude · Dev`).
    pub label: String,
    pub color: Color32,
    /// Trabalhando agora.
    pub busy: bool,
    /// Terminou um turno sem ser vista.
    pub done: bool,
    /// O processo continua rodando.
    pub alive: bool,
    /// Índice em `Deck::tabs` (clicar ativa a aba); `None` se o projeto ainda não foi carregado.
    pub tab: Option<usize>,
}

/// Um projeto na barra lateral.
#[derive(Debug, Clone)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub active: bool,
    /// A lista de agentes aparece embaixo do projeto.
    pub expanded: bool,
    pub agents: Vec<AgentBadge>,
}

/// O que a Deck lembra de um projeto enquanto ele não está na tela.
#[derive(Default)]
struct ProjectState {
    /// As abas salvas já foram abertas nesta sessão (até lá, só estão registradas no core).
    loaded: bool,
    /// A aba ativa quando o projeto saiu da tela.
    active_uid: Option<u64>,
    group_order: Vec<String>,
    split_ratios: split::Ratios,
    /// Conversa integrada e conversas recentes do mapa (as do projeto ativo ficam em `Deck`).
    feed: Vec<FeedItem>,
    recent: VecDeque<Recent>,
}

/// O que a tela dividida mostra neste quadro.
pub struct PanePlan {
    pub rect: egui::Rect,
    /// Índices das abas da janela, na ordem do cabeçalho.
    pub tabs: Vec<usize>,
    /// A aba visível (a ativada mais recentemente).
    pub shown: usize,
    pub groups: Vec<String>,
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
    pub sidebar_dragging: Option<(f32, f32)>,
    /// Largura pedida da barra direita (a efetiva depende da janela; ver `ui::right_width`).
    pub right_w: f32,
    pub right_dragging: Option<(f32, f32)>,
    pub right_view: RightView,

    // Tela dividida (ver `split.rs`)
    pub split_enabled: bool,
    /// Ordem das janelas (grupos de `split::group_key`).
    pub group_order: Vec<String>,
    pub split_ratios: split::Ratios,
    /// Janelas do último quadro (atalhos de teclado e tamanho das abas novas).
    pub panes: Vec<PaneSlot>,
    /// Janela sendo arrastada pela alça para trocar de lugar com outra.
    pub pane_drag: Option<usize>,
    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=split-drag`): janela-alvo fictícia do arrasto.
    pub debug_pane_drop: Option<usize>,
    show_seq: u64,

    // Mapa de conversas
    /// Conversas concluídas há pouco (o "Limpar" da conversa não apaga).
    recent: VecDeque<Recent>,
    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=map-demo`): mapa fictício para capturas de tela.
    pub debug_map: Option<Snapshot>,
    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=usage-demo`): uso fictício na barra de título.
    pub debug_usage: Option<HashMap<String, commanddeck_core::AgentUsage>>,
    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=projects-demo`): projetos fictícios na barra lateral.
    pub debug_projects: Option<Vec<ProjectView>>,
    /// SOMENTE DEPURAÇÃO (`COMMANDDECK_DEBUG_UI=project:<id>`): troca para este projeto logo depois de restaurar as abas.
    pub debug_switch_project: Option<String>,
    /// O botão de fechar da barra de título foi clicado (o `App` encerra depois do quadro).
    pub quit_requested: bool,

    /// Aba Git (ver `git_ui.rs`).
    pub git: crate::git_ui::GitUi,

    // Árvore e Projetos
    /// Pasta do projeto ativo (vazia sem projeto).
    pub workspace: PathBuf,
    /// Id do projeto ativo (vazio = nenhum). Só as abas dele aparecem.
    pub active_project: String,
    projects: HashMap<String, ProjectState>,
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
    pub needs_text: bool,
    recorder: Option<audio::Recorder>,
    pub recording_since: Option<Instant>,
    pub transcribing: bool,
    pub send_after_transcribe: bool,
    /// Aba ativa quando o Enviar foi clicado durante a gravação: a transcrição só sai sozinha se
    /// ela ainda for a ativa (senão a mensagem iria para outro agente ou projeto).
    send_target: Option<u64>,
    pub engine_label: String,
    pub focus_composer: bool,
    pub composer_focused: bool,
    pub composer_cursor_end: bool,

    // Menus e diversos
    pub new_tab_menu_open: bool,
    pub settings_open: bool,
    /// Aba sendo renomeada pelo usuário (índice, buffer de edição, erro opcional).
    pub rename_tab: Option<(usize, String, Option<String>)>,
    /// Menu de contexto aberto em uma aba (índice da aba, posição do clique).
    pub tab_context_menu: Option<(usize, egui::Pos2)>,
    pub sound_enabled: bool,
    /// Espelho de `Config.auto_open_agent_tabs` (item do menu de configurações).
    pub auto_open_agent_tabs: bool,
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
/// Quantas abas o app abre sozinho (a pedido de `ask-agent`) ao mesmo tempo.
const MAX_AUTO_TABS: usize = 8;
/// Depois de colar um envio, o Enter vai após este tempo (e de novo após `SECOND_ENTER` quando há
/// menção `@` em CLIs com autocomplete de arquivo).
const FIRST_ENTER: Duration = Duration::from_millis(250);
const SECOND_ENTER: Duration = Duration::from_millis(600);
/// Com hooks, a aba só deixa de estar ocupada pelo aviso de fim de turno. Se o aviso se perder,
/// ela é liberada depois deste tempo sem nenhuma saída.
const HOOK_STALE: Duration = Duration::from_secs(600);
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(60);
const IDLE_CHIME: Duration = Duration::from_millis(1200);
/// Enter (ou envio) que não produziu saída nenhuma (prompt vazio, menu): a aba é liberada depois
/// deste tempo, em silêncio. Sem isso ela ficava "trabalhando" para sempre e prendia a fila da ponte.
const NO_OUTPUT_IDLE: Duration = Duration::from_secs(8);
/// Mensagens guardadas na Conversa integrada de cada projeto (as mais antigas saem): a lista é
/// redesenhada a cada quadro e, sem limite, uma sessão longa deixava a interface lenta.
const FEED_MAX: usize = 1000;

fn cap_feed(feed: &mut Vec<FeedItem>) {
    if feed.len() > FEED_MAX {
        feed.drain(..feed.len() - FEED_MAX);
    }
}
const TREE_CLICK_DELAY: Duration = Duration::from_millis(220);

impl Deck {
    pub fn new(core: Core, events: Receiver<CoreEvent>, wake: Wake) -> Self {
        let cfg = core.config();
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut deck = Self {
            workspace: core.workspace(),
            active_project: core.active_project_id(),
            projects: HashMap::new(),
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
            sidebar_w: if cfg.sidebar_width > 0.0 {
                cfg.sidebar_width.clamp(theme::SIDEBAR_MIN_W, theme::SIDEBAR_MAX_W)
            } else {
                theme::SIDEBAR_DEFAULT_W
            },
            sidebar_collapsed: cfg.sidebar_collapsed,
            sidebar_dragging: None,
            right_w: if cfg.right_panel_width > 0.0 && cfg.right_panel_width != theme::RIGHT_OLD_DEFAULT_W {
                cfg.right_panel_width
            } else {
                theme::RIGHT_DEFAULT_W
            },
            right_dragging: None,
            right_view: RightView::Conversa,
            split_enabled: cfg.split_enabled,
            group_order: Vec::new(),
            split_ratios: cfg
                .active()
                .map(|p| split::Ratios { rows: p.split_rows.clone(), cols: p.split_cols.clone() })
                .unwrap_or(split::Ratios { rows: Vec::new(), cols: Vec::new() }),
            panes: Vec::new(),
            pane_drag: None,
            debug_pane_drop: None,
            show_seq: 0,
            recent: VecDeque::new(),
            debug_map: None,
            debug_usage: None,
            debug_projects: None,
            debug_switch_project: None,
            quit_requested: false,
            git: crate::git_ui::GitUi::default(),
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
            needs_text: false,
            recorder: None,
            recording_since: None,
            transcribing: false,
            send_after_transcribe: false,
            send_target: None,
            engine_label: String::new(),
            focus_composer: false,
            composer_focused: false,
            composer_cursor_end: false,
            new_tab_menu_open: false,
            settings_open: false,
            rename_tab: None,
            tab_context_menu: None,
            sound_enabled: cfg.sound_enabled,
            auto_open_agent_tabs: cfg.auto_open_agent_tabs,
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
        // A ponte passa a digitar as perguntas dos agentes nas abas (em vez de rodar o agente à parte).
        deck.core.enable_tab_delivery();
        if std::env::var("COMMANDDECK_DEBUG_FEED").is_ok_and(|v| v == "1") {
            deck.debug_feed();
        }
        // O chip da branch na topbar aparece já ao abrir.
        deck.git_refresh();
        deck
    }

    pub(crate) fn spawn_bg(&self, name: &str, f: impl FnOnce(Core) -> Option<UiMsg> + Send + 'static) {
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
            Action::FocusComposer => self.request_composer_focus(),
            Action::CopyText(t) => ctx.copy_text(t),
        }
    }

    fn on_core_event(&mut self, ev: CoreEvent) {
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
                self.git_refresh_soon();
            }
            CoreEvent::GitChanged => self.git_refresh_soon(),
            CoreEvent::AgentsChanged => self.refresh_agents(),
            CoreEvent::Download(_) if self.debug_hold_download => {}
            CoreEvent::Deliver(d) => self.on_deliver(d),
            CoreEvent::AgentSignal { project, id, busy } => self.on_agent_signal(&project, &id, busy),
            CoreEvent::UsageChanged => {}
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
                let target = self.send_target.take();
                if send && target == self.active_tab().map(|t| t.uid) {
                    self.send_message();
                } else if send {
                    self.system("A aba mudou durante a transcrição: o texto ficou na caixa de mensagem para você conferir e enviar.");
                }
            }
            UiMsg::Engine(label) => self.engine_label = label,
            UiMsg::WorkspacePicked(dir) => {
                self.add_project(dir);
            }
            UiMsg::Git(generation, reply) => self.on_git(generation, reply),
        }
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
        let active = self.active;
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
                            if active != Some(i) {
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
            tab.busy = false;
            tab.done = false;
            for q in tab.queue.drain() {
                self.core.fail_delivery(q.request, "a aba encerrou antes de receber a mensagem");
            }
            self.core.instance_ended(&tab.project, &tab.ident.id());
            tab.session.print(&format!("\r\n\x1b[90m[processo encerrado (código {code}) — pressione Enter para reiniciar]\x1b[0m\r\n"));
        }
        // Mensagens de outros agentes: digitadas no terminal quando a aba fica livre (uma por vez;
        // regra em `commanddeck_core::delivery`).
        for i in 0..self.tabs.len() {
            let Some(since) = self.tabs[i].queue.front_since() else { continue };
            match delivery::gate(&self.tab_state(i), since, now) {
                Gate::Fail => {
                    for q in self.tabs[i].queue.drain() {
                        self.core.fail_delivery(q.request, "a aba não está rodando");
                    }
                }
                Gate::Ready => {
                    if let Some(q) = self.tabs[i].queue.pop() {
                        // Pergunta cancelada na fila (expirou, quem perguntou fechou): não faz o agente
                        // trabalhar à toa nem pedir um `--reply` que daria 404.
                        if !q.answer && !self.core.question_pending(q.request) {
                            log::info!("pergunta #{} descartada antes de ser digitada: não espera mais resposta", q.request);
                        } else {
                            self.submit(i, &q.prompt);
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

    /// Ao iniciar: registra no core as abas salvas de todos os projetos (a ponte não cria
    /// duplicatas delas) e abre só as do projeto ativo; as outras abrem quando o projeto é
    /// selecionado (ou quando um agente chama alguém de lá).
    fn restore_tabs(&mut self) {
        for p in self.core.projects() {
            if p.id != self.active_project {
                for id in &p.open_tabs {
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
    fn load_project(&mut self, id: &str) {
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
    fn show_project_tab(&mut self, uid: Option<u64>) {
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
        let project = self.active_project.clone();
        if project.is_empty() {
            self.system("Abra uma pasta como projeto antes de abrir um agente.");
            self.new_project_dialog();
            return;
        }
        let ident = self.core.register_instance(&project, kind);
        self.add_tab(project, ident, restoring, false);
    }

    /// Pergunta de outro agente (`ask-agent`) para uma instância: abre a aba dela, se ainda não
    /// existe (em segundo plano, sem roubar o foco), e coloca a pergunta na fila para ser
    /// digitada no terminal assim que ele estiver pronto. A resposta volta quando o agente da
    /// aba rodar `ask-agent --reply <n> "resposta"`.
    ///
    /// Uma resposta atrasada (`d.answer`) vai para a aba de quem perguntou só se ela ainda está
    /// aberta e rodando: não reabre nada (a resposta já está na Conversa integrada).
    fn on_deliver(&mut self, d: Delivery) {
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
                    self.tabs[i].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, from: d.from, answer: true })
                }
                _ => log::info!("resposta atrasada para \"{id}\" descartada: a aba não está aberta"),
            }
            return;
        }
        let idx = match found {
            Some(i) => i,
            None => {
                let reason = if !self.agents.knows(&d.ident.kind) {
                    Some(format!("o agente \"{}\" não está disponível", d.ident.kind))
                } else if self.tabs.iter().filter(|t| t.auto).count() >= MAX_AUTO_TABS {
                    Some(format!("limite de {MAX_AUTO_TABS} abas abertas automaticamente; feche alguma"))
                } else {
                    None
                };
                if let Some(reason) = reason {
                    self.core.fail_delivery(d.request, &reason);
                    self.core.close_instance(&d.project, &id);
                    return;
                }
                self.add_tab(d.project.clone(), d.ident.clone(), false, true);
                let label = self.tabs.last().map(|t| t.label.clone()).unwrap_or_else(|| id.clone());
                self.system_in(&d.project, format!("Abri a aba de {label} para a conversa com outro agente."));
                self.tabs.len() - 1
            }
        };
        if !self.tabs[idx].alive {
            self.start(idx); // o processo tinha encerrado: sobe de novo para receber a pergunta
        }
        self.tabs[idx].queue.push(Instant::now(), Queued { request: d.request, prompt: d.prompt, from: d.from, answer: false });
    }

    /// O que a regra de entrega precisa saber da aba.
    fn tab_state(&self, idx: usize) -> TabState {
        let tab = &self.tabs[idx];
        TabState {
            alive: tab.alive,
            // Um envio cujo Enter ainda não saiu também ocupa a aba.
            busy: tab.busy || !tab.pending_enter.is_empty(),
            hooked: tab.hooked,
            last_output: *tab.session.shared.last_output.lock(),
            last_key: tab.last_key,
            typing: tab.typing,
        }
    }

    /// Hook do agente (`ask-agent --signal`): início ou fim de turno.
    fn on_agent_signal(&mut self, project: &str, id: &str, busy: bool) {
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
            if self.active != Some(idx) {
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
        let tab = Tab {
            uid: self.next_uid,
            kind: kind.clone(),
            ident,
            project,
            label,
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
                log::warn!("COMMANDDECK_DEBUG_FAKE_SHELL: usando shell local no lugar de '{}' ({e})", self.tabs[idx].kind);
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
        // Aba de outro projeto (ex.: clique no mapa): o projeto dela vem para a tela.
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
        self.tabs[idx].done = false;
        self.set_term_focus(true);
        self.persist_tabs();
    }

    /// Mantém `group_order` igual aos grupos abertos: um grupo novo entra no fim (vira uma janela
    /// nova) e sai quando a última aba dele fecha.
    fn sync_groups(&mut self) {
        let kinds: Vec<String> = self.visible().into_iter().map(|i| self.tabs[i].kind.clone()).collect();
        split::sync_order(&mut self.group_order, kinds.iter().map(String::as_str));
    }

    /// Janelas deste quadro na área `area`: a grade automática com um grupo por janela (ou uma
    /// janela só, com a divisão desligada). Guarda o resultado em `panes` para os atalhos.
    pub fn plan_panes(&mut self, area: egui::Rect) -> (Vec<usize>, Vec<PanePlan>) {
        let (shape, rects, groups) = if self.split_enabled && self.group_order.len() > 1 {
            let shape = split::grid_shape(self.group_order.len(), area.width(), area.height());
            self.split_ratios.conform(&shape);
            let rects = split::layout(area, &shape, &self.split_ratios);
            let groups = split::assign(&self.group_order, rects.len());
            (shape, rects, groups)
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
    fn focused_slot(&self) -> Option<usize> {
        let uid = self.active_tab()?.uid;
        self.panes.iter().position(|p| p.uids.contains(&uid))
    }

    fn slot_tabs(&self, slot: usize) -> Vec<usize> {
        self.panes[slot].uids.iter().filter_map(|u| self.tabs.iter().position(|t| t.uid == *u)).collect()
    }

    /// Abas da janela focada (as do projeto, se ainda não houve quadro).
    fn focused_pane_tabs(&self) -> Vec<usize> {
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

    /// Liga/desliga a tela dividida.
    pub fn toggle_split(&mut self) {
        self.split_enabled = !self.split_enabled;
        self.persist_layout();
    }

    pub fn close_tab(&mut self, idx: usize) {
        if idx >= self.tabs.len() {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        tab.session.kill();
        for q in tab.queue.drain() {
            self.core.fail_delivery(q.request, "a aba foi fechada antes de receber a mensagem");
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
            // Atualiza itens existentes na conversa integrada
            for item in &mut self.feed {
                if item.who.as_deref() == Some(&old_id) {
                    item.who = Some(new_id.clone());
                }
                if item.to.as_deref() == Some(&old_id) {
                    item.to = Some(new_id.clone());
                }
            }
            // Atualiza histórico recente do mapa
            for r in &mut self.recent {
                if r.from == old_id {
                    r.from = new_id.clone();
                }
                if r.to == old_id {
                    r.to = new_id.clone();
                }
            }
            self.persist_tabs();
        }
        self.rename_tab = None;
        Ok(())
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    /// Modelos das abas abertas, sem repetir e na ordem das abas (`ollama:*` vira `ollama`; sem o terminal comum).
    pub fn open_models(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for t in &self.tabs {
            let key = split::group_key(&t.kind);
            if key != "shell" && !out.iter().any(|k| k == key) {
                out.push(key.to_owned());
            }
        }
        // Capturas de tela da demonstração sem abas abertas: os modelos fictícios + um sem dados.
        if out.is_empty() && self.debug_usage.is_some() {
            out = ["claude", "antigravity", "ollama", "codex"].map(String::from).to_vec();
        }
        out
    }

    /// Uso dos planos por modelo (o fictício em depuração).
    pub fn usage(&self) -> HashMap<String, commanddeck_core::AgentUsage> {
        match &self.debug_usage {
            Some(u) => u.clone(),
            None => self.core.usage_snapshot(),
        }
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

    pub fn persist_layout(&mut self) {
        self.layout_persist_at = Some(Instant::now() + Duration::from_millis(400));
    }

    pub fn persist_layout_now(&mut self) {
        self.layout_persist_at = None;
        let (sw, sc, rw) = (self.sidebar_w, self.sidebar_collapsed, self.right_w);
        let (split_on, ratios) = (self.split_enabled, self.split_ratios.clone());
        self.core.update_config(|c| {
            c.sidebar_width = sw;
            c.sidebar_collapsed = sc;
            c.right_panel_width = rw;
            c.split_enabled = split_on;
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

    /// Ajusta a grade de uma aba visível à área dela (debounce de 60 ms ao PTY).
    pub fn fit_tab(&mut self, idx: usize, rect: egui::Rect) {
        let Some(m) = self.metrics.clone() else { return };
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
            if let Some(i) = self.active {
                if let Some(tab) = self.tabs.get_mut(i) {
                    tab.done = false;
                }
            }
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
    fn user_input(&mut self, idx: usize, bytes: &[u8], typing: bool) {
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
        let start = if self.workspace.as_os_str().is_empty() { dirs_home() } else { self.workspace.clone() };
        self.spawn_bg("pick-workspace", move |_| {
            let dir = rfd::FileDialog::new().set_directory(&start).pick_folder()?;
            Some(UiMsg::WorkspacePicked(dir))
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
        self.pane_drag = None;
        if !old.is_empty() {
            let uid = self.active.and_then(|i| self.tabs.get(i)).map(|t| t.uid);
            let state = self.projects.entry(old).or_default();
            state.active_uid = uid;
            state.group_order = std::mem::take(&mut self.group_order);
            state.split_ratios = std::mem::take(&mut self.split_ratios);
            state.feed = std::mem::take(&mut self.feed);
            state.recent = std::mem::take(&mut self.recent);
        }
        self.active = None;
        self.unseen = 0;
        self.feed_scroll_to_bottom = true;
        match self.projects.get_mut(id) {
            Some(state) if state.loaded => {
                self.group_order = std::mem::take(&mut state.group_order);
                self.split_ratios = std::mem::take(&mut state.split_ratios);
                self.feed = std::mem::take(&mut state.feed);
                self.recent = std::mem::take(&mut state.recent);
                let uid = state.active_uid;
                self.sync_groups();
                self.show_project_tab(uid);
            }
            Some(state) => {
                self.feed = std::mem::take(&mut state.feed);
                self.recent = std::mem::take(&mut state.recent);
                self.load_project(id);
            }
            None => self.load_project(id),
        }
        self.after_project_change();
    }

    /// O projeto da tela mudou: pasta, árvore (do zero) e git.
    fn after_project_change(&mut self) {
        self.workspace = self.core.workspace();
        self.tree = TreeState { expanded: HashSet::from([String::new()]), ..Default::default() };
        self.refresh_tree();
        self.git_reset_for_project();
        self.git_refresh();
        self.persist_tabs();
    }

    // ------------------------------------------------------------------
    // Projetos (API da barra lateral)
    // ------------------------------------------------------------------

    /// Projetos para a barra lateral, na ordem salva, com os agentes de cada um (de um projeto
    /// ainda não aberto nesta sessão, os salvos, sem aba).
    pub fn project_views(&self) -> Vec<ProjectView> {
        if let Some(demo) = &self.debug_projects {
            return demo.clone();
        }
        self.core
            .projects()
            .into_iter()
            .map(|p| {
                let loaded = self.projects.get(&p.id).is_some_and(|s| s.loaded);
                let agents = if loaded {
                    (0..self.tabs.len())
                        .filter(|&i| self.tabs[i].project == p.id)
                        .map(|i| {
                            let t = &self.tabs[i];
                            AgentBadge {
                                id: t.ident.id(),
                                parent: p.tab_parents.get(&t.ident.id()).cloned(),
                                kind: t.kind.clone(),
                                label: t.label.clone(),
                                color: t.color,
                                busy: t.busy,
                                done: t.done,
                                alive: t.alive,
                                tab: Some(i),
                            }
                        })
                        .collect()
                } else {
                    p.open_tabs
                        .iter()
                        .filter_map(|id| Ident::parse(id))
                        .map(|ident| AgentBadge {
                            id: ident.id(),
                            parent: p.tab_parents.get(&ident.id()).cloned(),
                            label: ident.label(&self.agents.name(&ident.kind)),
                            color: self.agents.tab_color(&ident.kind),
                            kind: ident.kind,
                            busy: false,
                            done: false,
                            alive: false,
                            tab: None,
                        })
                        .collect()
                };
                ProjectView { active: p.id == self.active_project, expanded: !p.collapsed, id: p.id, name: p.name, path: p.path, agents }
            })
            .collect()
    }

    /// Mostra ou esconde a lista de agentes do projeto `id` na barra lateral (gravado na config).
    pub fn toggle_project_expanded(&mut self, id: &str) {
        self.core.update_project(id, |p| p.collapsed = !p.collapsed);
    }

    /// Mostra (`true`) ou esconde a lista de agentes de todos os projetos.
    pub fn set_all_projects_expanded(&mut self, expanded: bool) {
        for p in self.core.projects() {
            self.core.update_project(&p.id, |p| p.collapsed = !expanded);
        }
    }

    /// Agente arrastado na barra lateral: passa a responder a `boss` (`None` = ao usuário). O core
    /// recusa ciclos; o aviso de equipe vai na próxima mensagem dos afetados.
    pub fn set_agent_boss(&mut self, project: &str, id: &str, boss: Option<&str>) {
        if let Err(e) = self.core.set_boss(project, id, boss) {
            self.system(format!("Não foi possível mudar a equipe: {e}."));
        }
    }

    /// Clique num agente da barra lateral: traz o projeto dele para a tela (abrindo as abas salvas,
    /// se ainda não abriram) e ativa a aba.
    pub fn focus_agent(&mut self, project: &str, id: &str) {
        self.switch_project(project);
        if let Some(i) = self.tabs.iter().position(|t| t.project == project && t.ident.id() == id) {
            self.activate(i);
        }
    }

    /// "+" da seção PROJETOS: escolhe a pasta do projeto novo.
    pub fn new_project_dialog(&self) {
        self.pick_workspace();
    }

    /// Dá outro nome ao projeto (não muda a pasta).
    pub fn rename_project(&mut self, id: &str, name: &str) {
        let name = name.trim();
        if !name.is_empty() {
            self.core.rename_project(id, name);
        }
    }

    /// Tira o projeto da lista e fecha as abas dele (a pasta fica). Se era o da tela, o core ativa
    /// o primeiro que sobrar (ou nenhum).
    pub fn remove_project_by_id(&mut self, id: &str) {
        while let Some(i) = self.tabs.iter().position(|t| t.project == id) {
            self.close_tab(i);
        }
        self.projects.remove(id);
        let was_active = id == self.active_project;
        self.core.remove_project(id);
        if was_active {
            self.active_project.clear();
            self.active = None;
            self.group_order.clear();
            self.split_ratios = split::Ratios::default();
            self.feed.clear();
            self.recent.clear();
            let next = self.core.active_project_id();
            if next.is_empty() {
                self.after_project_change();
            } else {
                self.switch_project(&next);
            }
        }
    }

    // ------------------------------------------------------------------
    // Conversa integrada
    // ------------------------------------------------------------------

    pub fn add_feed(&mut self, e: FeedEntry) {
        // Entradas de outro projeto vão para a conversa dele (guardada até ele voltar à tela);
        // as globais (sem projeto, ex.: o Whisper) e as da interface ficam na do projeto ativo.
        let elsewhere = e.project.clone().filter(|p| *p != self.active_project && !self.active_project.is_empty());
        let thinking_key = |from: &str| format!("{}#{from}", e.project.as_deref().unwrap_or(&self.active_project));
        let thinking_prev = e.from.as_deref().map(thinking_key).and_then(|k| self.thinking.remove(&k));
        let thinking_new = (e.kind == FeedKind::Thinking).then(|| e.from.as_deref().map(thinking_key)).flatten();
        let to = e.to.as_deref().map(|t| self.agents.who_name(t));
        let item = FeedItem {
            id: self.next_feed_id,
            kind: e.kind,
            who: e.from.as_ref().map(|f| self.agents.who_name(f)),
            to,
            color: self.agents.who_color(e.from.as_deref()),
            time: util::time_pt_br(e.ts),
            text: e.text.clone(),
        };
        self.next_feed_id += 1;
        if let Some(key) = thinking_new {
            self.thinking.insert(key, item.id);
        }
        if let Some(project) = elsewhere {
            let state = self.projects.entry(project).or_default();
            track_recent(&mut state.recent, &e);
            if let Some(id) = thinking_prev {
                state.feed.retain(|f| f.id != id);
            }
            state.feed.push(item);
            cap_feed(&mut state.feed);
            return;
        }
        track_recent(&mut self.recent, &e);
        if let Some(id) = thinking_prev {
            self.feed.retain(|f| f.id != id);
        }
        self.feed.push(item);
        cap_feed(&mut self.feed);
        if self.feed_near_bottom {
            self.feed_scroll_to_bottom = true;
        }
        if self.right_view != RightView::Conversa && e.kind != FeedKind::Thinking {
            self.unseen += 1;
        }
    }

    /// Troca o que a barra direita mostra (voltar à Conversa zera as não vistas).
    pub fn set_right_view(&mut self, view: RightView) {
        let opening_git = view == RightView::Git && self.right_view != RightView::Git;
        self.right_view = view;
        if view == RightView::Conversa {
            self.unseen = 0;
        }
        if opening_git {
            self.git_refresh();
        }
    }

    /// Quem está conversando com quem agora, para o mapa: o core sabe as perguntas pendentes; a
    /// interface sabe as filas das abas, quem está ocupado, e as conversas recentes.
    pub fn map_snapshot(&self) -> Snapshot {
        if let Some(demo) = &self.debug_map {
            return demo.clone();
        }
        let cm = self.core.conversation_map(&self.active_project);
        let visible: Vec<&Tab> = self.visible().into_iter().map(|i| &self.tabs[i]).collect();
        let nodes = visible
            .iter()
            .map(|t| {
                let state = if !t.alive {
                    NodeState::Ended
                } else if t.busy || !t.pending_enter.is_empty() {
                    NodeState::Busy
                } else {
                    NodeState::Idle
                };
                (t.ident.id(), state)
            })
            .collect();
        let queued = |target: &str, request: u64| -> Option<usize> {
            let tab = visible.iter().find(|t| t.ident.id() == target)?;
            tab.queue.iter().filter(|q| !q.answer).position(|q| q.request == request).map(|i| i + 1)
        };
        let mut links: Vec<Link> = cm
            .pending
            .iter()
            .map(|p| {
                let state =
                    if p.detached { LinkState::Detached } else { queued(&p.target, p.id).map_or(LinkState::Typed, LinkState::Queued) };
                Link { id: Some(p.id), from: p.asker.clone(), to: p.target.clone(), since_ms: p.since_ms, state }
            })
            .collect();
        links.extend(cm.headless.iter().map(|h| Link {
            id: None,
            from: h.asker.clone(),
            to: h.target.clone(),
            since_ms: h.since_ms,
            state: LinkState::Headless,
        }));
        let answers = visible
            .iter()
            .flat_map(|t| {
                let to = t.ident.id();
                t.queue
                    .iter()
                    .filter(|q| q.answer)
                    .map(move |q| AnswerInFlight { request: q.request, from: q.from.clone(), to: to.clone() })
                    .collect::<Vec<_>>()
            })
            .collect();
        Snapshot { nodes, links, answers, recent: self.recent.iter().cloned().collect() }
    }

    pub fn system(&mut self, text: impl Into<String>) {
        self.add_feed(FeedEntry { ts: 0, kind: FeedKind::System, from: None, to: None, text: text.into(), project: None });
    }

    /// Aviso na conversa do projeto `project` (que pode não ser o da tela).
    fn system_in(&mut self, project: &str, text: impl Into<String>) {
        self.add_feed(FeedEntry::new(FeedKind::System, None, None, text).in_project(project));
    }

    /// "Limpar": só a conversa do projeto da tela.
    pub fn clear_feed(&mut self) {
        self.feed.clear();
        let prefix = format!("{}#", self.active_project);
        self.thinking.retain(|k, _| !k.starts_with(&prefix));
    }

    fn debug_feed(&mut self) {
        let mk = |kind, from: Option<&str>, to: Option<&str>, text: &str| FeedEntry::new(kind, from, to, text);
        self.add_feed(mk(FeedKind::User, Some("você"), Some("claude"), "Revise o @README.md e proponha melhorias."));
        self.add_feed(mk(
            FeedKind::Ask,
            Some("claude"),
            Some("antigravity"),
            "Você pode verificar se o build do Windows ainda usa o ConPTY?",
        ));
        self.add_feed(mk(
            FeedKind::Answer,
            Some("antigravity"),
            Some("claude"),
            "Sim — portable-pty usa ConPTY no Windows 10+.\nNada a mudar.",
        ));
        self.add_feed(mk(FeedKind::System, None, None, "Projeto ativo alterado para: workspace"));
        self.add_feed(mk(FeedKind::Error, Some("ollama:qwen3.5:9b"), None, "Falha ao contatar o Ollama em 127.0.0.1:11434."));
        self.add_feed(mk(FeedKind::Thinking, Some("claude"), None, "⚙️ [read_file] README.md"));
    }

    // ------------------------------------------------------------------
    // Compositor / envio
    // ------------------------------------------------------------------

    pub fn request_composer_focus(&mut self) {
        // Na aba Git o compositor fica escondido: volta para a Conversa.
        if self.right_view == RightView::Git {
            self.set_right_view(RightView::Conversa);
        }
        self.focus_composer = true;
        self.term_focus = false;
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// `ready` do botão Enviar.
    pub fn send_ready(&self) -> bool {
        !self.input.trim().is_empty()
            || !self.attachments.is_empty()
            || self.is_recording()
            || self.transcribing
            || self.send_after_transcribe
    }

    /// `--send-color`: cor da aba ativa, se não for um shell.
    pub fn send_color(&self) -> Color32 {
        if let Some(t) = self.active_tab() {
            if t.kind != "shell" {
                if let Some(c) = self.agents.color(&t.kind) {
                    return c;
                }
            }
        }
        theme::SEND_DEFAULT
    }

    pub fn send_label(&self) -> &'static str {
        if self.send_after_transcribe && (self.is_recording() || self.transcribing) {
            "Enviando…"
        } else {
            "Enviar"
        }
    }

    pub fn pick_attachments(&self) {
        self.spawn_bg("pick-attachments", |core| {
            let files = rfd::FileDialog::new().pick_files()?;
            Some(UiMsg::Attached(core.attach_files(&files)))
        });
    }

    pub fn attach_paths(&mut self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        if self.right_view == RightView::Git {
            self.set_right_view(RightView::Conversa);
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
            let res = if pcm.is_empty() {
                Err("nenhum áudio capturado".to_string())
            } else {
                core.transcribe_pcm16k(&pcm).map(|t| t.text).map_err(|e| e.to_string())
            };
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

    /// Porta de `sendMessage()`: o compositor envia sempre para a aba ativa.
    pub fn send_message(&mut self) {
        if self.recorder.is_some() || self.transcribing {
            self.send_after_transcribe = true;
            self.send_target = self.active_tab().map(|t| t.uid);
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
        let Some(idx) = self.active.filter(|&i| self.tabs[i].alive) else {
            self.system("A aba ativa não tem um processo rodando.");
            return;
        };
        self.submit(idx, &full);
        self.set_term_focus(true);
        let to = self.tabs[idx].ident.id();
        self.add_feed(FeedEntry::new(FeedKind::User, Some("você"), Some(&to), full.clone()));
        self.input.clear();
        self.attachments.clear();
    }

    /// `pty:submit`: cola entre colchetes e confirma com Enter após 250 ms (e de novo após
    /// 600 ms quando há menção `@` em CLIs com autocomplete de arquivo).
    ///
    /// Os Enters saem da thread da interface (em `tick`), na ordem certa em relação ao que o
    /// usuário digitar nesse meio-tempo; uma thread só acorda a interface na hora.
    fn submit(&mut self, idx: usize, text: &str) {
        let now = Instant::now();
        let tab = &mut self.tabs[idx];
        // Um envio anterior que ainda esperava o Enter é confirmado antes desta colagem.
        for _ in tab.pending_enter.drain(..) {
            tab.session.write(b"\r");
        }
        if !tab.held_keys.is_empty() {
            let held = std::mem::take(&mut tab.held_keys);
            tab.session.write(&held);
        }
        if !tab.busy {
            tab.busy_output = false;
            tab.idle_deadline = Some(now + NO_OUTPUT_IDLE);
        }
        tab.busy = true;
        tab.turn_confirmed = false;
        tab.typing = false;
        tab.done = false;
        tab.session.write(&input::submit_paste(text));
        let double = text.contains('@') && tab.spec.as_ref().is_some_and(|s| s.double_enter_on_mention);
        let mut times = vec![now + FIRST_ENTER];
        if double {
            times.push(now + SECOND_ENTER);
        }
        tab.pending_enter.extend(times.iter().copied());
        let wake = self.wake.clone();
        let _ = std::thread::Builder::new().name("submit-wake".into()).spawn(move || {
            for at in times {
                std::thread::sleep(at.saturating_duration_since(Instant::now()) + Duration::from_millis(2));
                wake();
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
/// Alimenta as conversas recentes do mapa com a resposta (ou falha) de uma pergunta entre dois agentes.
fn track_recent(recent: &mut VecDeque<Recent>, e: &FeedEntry) {
    if !matches!(e.kind, FeedKind::Answer | FeedKind::Error) {
        return;
    }
    if let (Some(from), Some(to)) = (&e.from, &e.to) {
        let at_ms = commanddeck_core::util::now_ms();
        recent.push_back(Recent { from: from.clone(), to: to.clone(), ok: e.kind == FeedKind::Answer, at_ms });
        while recent.len() > 50 || recent.front().is_some_and(|r| at_ms.saturating_sub(r.at_ms) > agent_map::RECENT_MS) {
            recent.pop_front();
        }
    }
}

/// Pasta pessoal (início do seletor de pasta quando não há projeto).
fn dirs_home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

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

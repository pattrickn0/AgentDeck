//! Estado e lógica da interface (porta do `renderer.js`), independente da pintura.
//! A pintura fica em [`crate::ui`]; a janela/loop em [`crate::app`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentdeck_core::delivery::{self, Gate, Mailbox, TabState};
use agentdeck_core::{
    AgentInfo, Core, CoreEvent, Delivery, DirEntry, DownloadProgress, DownloadState, FeedEntry, FeedKind, Ident, SpawnSpec,
};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use crossbeam_channel::{Receiver, Sender};
use egui::Color32;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::agents::Agents;
use crate::audio;
use crate::dictation::{self, MicPhase, MicTarget, TabDelivery, ToggleOutcome};
use crate::docs::CenterView;
use crate::icons::IconCache;
use crate::split;
use crate::terminal::input::{self, Mods};
use crate::terminal::view::{Metrics, ViewState};
use crate::terminal::{GridSize, Session, Wake};
use crate::theme;
use crate::util;

/// Destino de arquivos anexados pelo botão de anexo, arrastar e soltar ou Ctrl+V de imagem.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    /// Digita os caminhos anexados no PTY da aba `uid` (sem Enter).
    Tab(u64),
    /// Adiciona os anexos ao formulário "Nova tarefa" do projeto `project_id`.
    TaskForm(String),
}

/// Área destacada enquanto arquivos são arrastados sobre a janela (`ui/overlay.rs`).
#[derive(Debug, Clone)]
pub struct DropZone {
    pub rect: egui::Rect,
    pub target: AttachTarget,
    pub title: String,
    pub subtitle: String,
}

/// Mensagens de tarefas em segundo plano para a UI.
pub enum UiMsg {
    Agents(Vec<AgentInfo>),
    /// Anexos copiados (caminhos absolutos em `dados/anexos`) para uma aba ou formulário de tarefa.
    Attached(AttachTarget, Vec<PathBuf>),
    Transcribed(Result<String, String>),
    Engine(String),
    WorkspacePicked(PathBuf),
    /// Resultado da abertura do microfone (feita fora da thread da interface).
    MicOpened(Result<audio::Recorder, String>),
    /// O usuário confirmou remover o projeto (id) da lista.
    RemoveProject(String),
    /// Resposta do Git, com a geração do projeto em que foi pedida (ver `GitUi::generation`).
    Git(u64, crate::git_ui::GitReply),
    /// Modelos conhecidos de um agente (menu de novo agente); vazio = só o padrão.
    Models(String, Vec<String>),
    /// Resposta da mesa de tarefas, com a geração do projeto em que foi pedida (ver `TasksUi::generation`).
    Tasks(u64, crate::tasks_ui::TaskReply),
    /// Um documento do modo Documentos terminou de carregar (uid do documento).
    Doc(u64, crate::docs::Loaded),
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
    /// Ctrl+Shift+M: inicia/para o ditado na aba de CLI focada (ou na que já está gravando).
    ToggleTabMic,
    CopyText(String),
    /// Ctrl+Shift+D: alterna o centro entre Agentes e Documentos.
    ToggleCenter,
}

/// Mensagem de outro agente (ou da UI) esperando para ser digitada no terminal da aba.
struct Queued {
    /// Número da pergunta no core; 0 = mensagem da própria interface (ex.: aba GitHub), sem resposta a devolver.
    request: u64,
    prompt: String,
    /// É uma resposta atrasada a uma pergunta desta aba, não uma pergunta nova.
    answer: bool,
}

/// O que a barra lateral direita mostra.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightView {
    /// Mesa de tarefas (ver `tasks_ui.rs`).
    Tarefas,
    Conversa,
    /// Repositório git do workspace (ver `git_ui.rs`).
    Git,
    /// PRs, issues e Actions do GitHub (ver `github_ui.rs`).
    GitHub,
}

pub struct Tab {
    pub uid: u64,
    pub kind: String,
    /// Identidade da instância (`claude`, `claude@dev`, `claude@dev@ana`).
    pub ident: Ident,
    /// Id do projeto da instância (as instâncias do core são por projeto).
    pub project: String,
    pub label: String,
    /// Modelo de LLM da instância (`sonnet`); `None` = o padrão do CLI ou um agente que não escolhe.
    pub model: Option<String>,
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
    /// Quando o processo atual foi iniciado (a regra de entrega dá tempo de a TUI subir).
    spawned_at: Option<Instant>,
}

impl Tab {
    /// Um redimensionamento do PTY espera o tamanho parar (ver `Deck::fit_tab`).
    #[cfg(test)]
    pub(crate) fn resize_pending(&self) -> bool {
        self.pending_resize.is_some()
    }
}

/// Uma janela da tela dividida, como ficou no último quadro.
pub struct PaneSlot {
    /// Janela inteira (cabeçalho de abas + terminal).
    pub rect: egui::Rect,
    /// Área do terminal.
    pub body: egui::Rect,
    pub uids: Vec<u64>,
    /// `uid` da aba visível (`plan.shown`) desta janela.
    pub shown_uid: u64,
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
    /// Modelo de LLM da instância, quando tem um.
    pub model: Option<String>,
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
    /// Conversa integrada (a do projeto ativo fica em `Deck`).
    feed: Vec<FeedItem>,
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
    pub right_panel_rect: Option<egui::Rect>,

    // Tela dividida (ver `split.rs`)
    pub split_enabled: bool,
    /// Ordem das janelas (grupos de `split::group_key`).
    pub group_order: Vec<String>,
    pub split_ratios: split::Ratios,
    /// Janelas do último quadro (atalhos de teclado e tamanho das abas novas).
    pub panes: Vec<PaneSlot>,
    /// Janela sendo arrastada pela alça para trocar de lugar com outra.
    pub pane_drag: Option<usize>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=split-drag`): janela-alvo fictícia do arrasto.
    pub debug_pane_drop: Option<usize>,
    show_seq: u64,

    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=usage-demo`): uso fictício na barra de título.
    pub debug_usage: Option<HashMap<String, agentdeck_core::AgentUsage>>,
    /// SOMENTE DEPURAÇÃO: o painel de uso aberto sem o mouse (`AGENTDECK_DEBUG_UI=usage-panel`).
    pub debug_usage_panel: bool,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=projects-demo`): projetos fictícios na barra lateral.
    pub debug_projects: Option<Vec<ProjectView>>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=project:<id>`): troca para este projeto logo depois de restaurar as abas.
    pub debug_switch_project: Option<String>,
    /// O botão de fechar da barra de título foi clicado (o `App` encerra depois do quadro).
    pub quit_requested: bool,

    /// Aba Git (ver `git_ui.rs`).
    pub git: crate::git_ui::GitUi,
    /// Aba GitHub (ver `github_ui.rs`).
    pub github: crate::github_ui::GithubUi,
    /// Aba Tarefas (ver `tasks_ui.rs`).
    pub tasks: crate::tasks_ui::TasksUi,
    /// Modo Documentos do centro (ver `docs.rs`).
    pub docs: crate::docs::Docs,
    /// O que o centro mostrava no quadro anterior (para perceber a troca; ver `Deck::center_changed`).
    pub(crate) center_seen: crate::docs::CenterView,

    // Árvore e Projetos
    /// Pasta do projeto ativo (vazia sem projeto).
    pub workspace: PathBuf,
    /// Id do projeto ativo (vazio = nenhum). Só as abas dele aparecem.
    pub active_project: String,
    projects: HashMap<String, ProjectState>,
    pub tree: TreeState,

    // Conversa integrada (somente leitura)
    pub feed: Vec<FeedItem>,
    thinking: HashMap<String, u64>,
    next_feed_id: u64,
    pub unseen: u32,
    /// Download em andamento (barra no cabeçalho) e, se for um erro, quando some.
    pub download: Option<(DownloadProgress, Option<Instant>)>,
    /// SOMENTE DEPURAÇÃO: mantém o estado fictício de `AGENTDECK_DEBUG_UI=download*`, ignorando o download real.
    pub debug_hold_download: bool,
    pub feed_scroll_to_bottom: bool,
    pub feed_near_bottom: bool,

    // Ditado por voz (preso ao `uid` da aba onde começou)
    recorder: Option<audio::Recorder>,
    pub recording_since: Option<Instant>,
    /// Estado global do microfone e seu destino (`Tab(uid)`).
    pub mic_phase: MicPhase,
    pub engine_label: String,

    // Menus e diversos
    pub new_tab_menu_open: bool,
    /// Agente cujo segundo passo do menu (escolher o modelo) está aberto.
    pub new_tab_models_for: Option<String>,
    /// Provedor escolhido no menu do modelo (`opencode`, `opencode-go`…): a lista mostra só os
    /// modelos dele. `None` = o passo dos provedores.
    pub new_tab_provider: Option<String>,
    /// Modelos conhecidos de cada agente e quando chegaram (buscados fora da thread da interface).
    model_lists: HashMap<String, (Vec<String>, Instant)>,
    /// Agentes cuja lista de modelos está sendo buscada agora.
    models_loading: HashSet<String>,
    pub settings_open: bool,
    /// Aba sendo renomeada pelo usuário (índice, buffer de edição, erro opcional).
    pub rename_tab: Option<(usize, String, Option<String>)>,
    /// Menu de contexto aberto em uma aba (índice da aba, posição do clique).
    pub tab_context_menu: Option<(usize, egui::Pos2)>,
    pub sound_enabled: bool,
    /// Espelho de `Config.auto_open_agent_tabs` (item do menu de configurações).
    pub auto_open_agent_tabs: bool,
    /// Espelho de `Config.app_shortcuts_first` (item do menu de configurações).
    pub app_shortcuts_first: bool,
    /// Há um modal aberto (do último quadro): o teclado não vai ao terminal nem aos atalhos.
    pub modal_open: bool,
    /// Ctrl+/ pedido pelo teclado (abre/fecha o modal de atalhos no próximo quadro).
    pub toggle_shortcuts: bool,
    pub drop_hover: bool,
    pub last_drop_target: Option<AttachTarget>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=drop-pane:<n>`): destaca este painel no arrastar e soltar.
    pub debug_drop_slot: Option<usize>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=drop-task`): destaca o formulário "Nova tarefa" no arrastar e soltar.
    pub debug_drop_task_form: bool,
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
/// Quanto tempo a lista de modelos recebida de um agente vale antes de pedir de novo.
const MODEL_LIST_TTL: Duration = Duration::from_secs(300);
/// Depois de colar um envio, o Enter vai após este tempo (e de novo após `SECOND_ENTER` quando há
/// menção `@` em CLIs com autocomplete de arquivo).
const FIRST_ENTER: Duration = Duration::from_millis(250);
const SECOND_ENTER: Duration = Duration::from_millis(600);
/// Com hooks, a aba só deixa de estar ocupada pelo aviso de fim de turno. Se o aviso se perder,
/// ela é liberada depois deste tempo sem nenhuma saída.
const HOOK_STALE: Duration = Duration::from_secs(600);
/// Cada largura nova faz o Claude Code reimprimir a conversa, e no Windows sem `ESC[3J`: a cópia
/// anterior fica no histórico. Por isso o PTY só muda com o tamanho parado e o mouse solto
/// (ver [`resize_due`]): um arrasto do divisor ou da borda vira um redimensionamento só.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
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

/// Fecha o microfone numa thread (esperar a captura terminar não é coisa da thread da interface).
fn discard_recorder(rec: audio::Recorder) {
    let _ = std::thread::Builder::new().name("mic-close".into()).spawn(move || rec.cancel());
}

/// Caminhos absolutos dos anexos a partir das referências do core (relativas ao workspace da hora
/// da cópia, ou absolutas). Todo anexo fica em `dados/anexos` com nome único, então o nome basta
/// e não depende de qual projeto estava ativo.
fn attachment_paths(core: &Core, refs: Vec<String>) -> Vec<PathBuf> {
    let dir = core.paths().data_dir.join("anexos");
    refs.iter().filter_map(|r| Path::new(r).file_name()).map(|name| dir.join(name)).collect()
}

/// O arquivo é uma cópia nossa em `dados/anexos` (só essas o "×" apaga).
fn in_attachments_dir(data_dir: &Path, path: &Path) -> bool {
    path.parent().is_some_and(|p| p == data_dir.join("anexos"))
}

/// O redimensionamento pendente do PTY já pode ser aplicado: tamanho parado e mouse solto.
fn resize_due(now: Instant, at: Instant, dragging: bool) -> bool {
    now >= at && !dragging
}

/// Ctrl+<tecla> é atalho do app? Ctrl+T sempre; Ctrl+B (prefixo do tmux) vai ao terminal quando
/// ele tem o foco, a não ser que o usuário dê prioridade ao app.
fn app_claims_ctrl(key: &str, term_focused: bool, app_first: bool) -> bool {
    match key {
        "b" => !term_focused || app_first,
        _ => true,
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
            // A mesa de tarefas é a primeira aba e a que abre: o app não grava a aba escolhida.
            right_view: RightView::Tarefas,
            right_panel_rect: None,
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
            debug_usage: None,
            debug_usage_panel: false,
            debug_projects: None,
            debug_switch_project: None,
            quit_requested: false,
            git: crate::git_ui::GitUi::default(),
            github: crate::github_ui::GithubUi::default(),
            tasks: crate::tasks_ui::TasksUi::default(),
            docs: crate::docs::Docs::default(),
            center_seen: crate::docs::CenterView::Agents,
            tree: TreeState { expanded: HashSet::from([String::new()]), ..Default::default() },
            feed: Vec::new(),
            thinking: HashMap::new(),
            next_feed_id: 1,
            unseen: 0,
            download: None,
            debug_hold_download: false,
            feed_scroll_to_bottom: false,
            feed_near_bottom: true,
            recorder: None,
            recording_since: None,
            mic_phase: MicPhase::Idle,
            engine_label: String::new(),
            new_tab_menu_open: false,
            new_tab_models_for: None,
            new_tab_provider: None,
            model_lists: HashMap::new(),
            models_loading: HashSet::new(),
            settings_open: false,
            rename_tab: None,
            tab_context_menu: None,
            sound_enabled: cfg.sound_enabled,
            auto_open_agent_tabs: cfg.auto_open_agent_tabs,
            app_shortcuts_first: cfg.app_shortcuts_first,
            modal_open: false,
            toggle_shortcuts: false,
            drop_hover: false,
            last_drop_target: None,
            debug_drop_slot: None,
            debug_drop_task_form: false,
            term_focus: false,
            ime_preedit: String::new(),
            metrics: None,
            blink_epoch: Instant::now(),
            icons: IconCache::default(),
            terminal_rect: None,
            window_focused: true,
            debug_close_all: false,
            debug_fake_shell: agentdeck_core::env_var("DEBUG_FAKE_SHELL").is_some_and(|v| v == "1"),
        };
        deck.refresh_agents();
        deck.refresh_engine();
        // A ponte passa a digitar as perguntas dos agentes nas abas (em vez de rodar o agente à parte).
        deck.core.enable_tab_delivery();
        if agentdeck_core::env_var("DEBUG_FEED").is_some_and(|v| v == "1") {
            deck.debug_feed();
        }
        // O chip da branch na topbar aparece já ao abrir, e o número de tarefas em revisão no ícone da aba.
        deck.git_refresh();
        deck.tasks_refresh();
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

    /// Pede os modelos conhecidos de `kind` (numa thread; o core guarda em cache). Não repete enquanto
    /// há um pedido em andamento nem enquanto a lista recebida é recente.
    pub fn request_models(&mut self, kind: &str) {
        let fresh = self.model_lists.get(kind).is_some_and(|(_, at)| at.elapsed() < MODEL_LIST_TTL);
        if fresh || !self.models_loading.insert(kind.to_owned()) {
            return;
        }
        let kind = kind.to_owned();
        self.spawn_bg("list-models", move |core| {
            let models = core.known_models(&kind);
            Some(UiMsg::Models(kind, models))
        });
    }

    /// Lista de modelos de `kind` para o menu e se ainda está sendo buscada.
    pub fn models_of(&self, kind: &str) -> (&[String], bool) {
        (self.model_lists.get(kind).map_or(&[][..], |(l, _)| l.as_slice()), self.models_loading.contains(kind))
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
        if self.docs.view != self.center_seen {
            self.center_changed();
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
            Action::ToggleCenter => self.docs.toggle(&self.active_project),
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
                self.docs_reload_files();
                self.git_refresh_soon();
            }
            CoreEvent::GitChanged => self.git_refresh_soon(),
            CoreEvent::AgentsChanged => self.refresh_agents(),
            CoreEvent::Download(_) if self.debug_hold_download => {}
            CoreEvent::Deliver(d) => self.on_deliver(d),
            CoreEvent::AgentSignal { project, id, busy } => self.on_agent_signal(&project, &id, busy),
            CoreEvent::UsageChanged => {}
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
                        match dictation::resolve_tab_delivery(&pairs, uid) {
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
                let Some(MicTarget::Tab(uid)) = dictation::finish_transcribe(&mut self.mic_phase) else {
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
                match dictation::resolve_tab_delivery(&pairs, uid) {
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
        let dragging = ctx.input(|i| i.pointer.primary_down());
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
            // OSC 52 das outras abas é descartado.
            if let Some(text) = tab.session.take_clipboard() {
                if crate::terminal::clipboard_allowed(i, active, &tab.project, &self.active_project) {
                    ctx.copy_text(text);
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
            if let Some((size, at)) = tab.pending_resize {
                if resize_due(now, at, dragging) {
                    tab.pending_resize = None;
                    if let Some(m) = &metrics {
                        tab.session.resize(size, m.cell_w, m.cell_h);
                    }
                } else if now < at {
                    want(at);
                } else {
                    // Mouse ainda pressionado: soltar já pede um quadro; isto é só a garantia.
                    want(now + RESIZE_DEBOUNCE);
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
    fn tab_state(&self, idx: usize) -> TabState {
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
        // Lendo documentos, a aba ativada continua fora da vista: o selo da chave segue valendo.
        if self.docs.view == CenterView::Agents {
            self.tabs[idx].done = false;
        }
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
            // Só ajusta para desenhar: as proporções salvas mudam apenas quando o usuário arrasta.
            let rects = split::layout(area, &shape, &self.split_ratios.fitted(&shape));
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

    /// Pergunta (numa thread, sem congelar o quadro) se o projeto sai da lista.
    pub fn confirm_remove_project(&self, id: String, name: String) {
        self.spawn_bg("confirm-remove-project", move |_| {
            let ok = rfd::MessageDialog::new()
                .set_title("Remover projeto")
                .set_description(format!("Deseja remover \"{name}\" da lista de projetos?"))
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
    fn after_project_change(&mut self) {
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
                                model: p.tab_models.get(&t.ident.id()).cloned(),
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
                            model: p.tab_models.get(&ident.id()).cloned(),
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
        self.show_agents();
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
        self.docs.drop_project(id);
        let was_active = id == self.active_project;
        self.core.remove_project(id);
        if was_active {
            self.active_project.clear();
            self.active = None;
            self.group_order.clear();
            self.split_ratios = split::Ratios::default();
            self.feed.clear();
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
            if let Some(id) = thinking_prev {
                state.feed.retain(|f| f.id != id);
            }
            state.feed.push(item);
            cap_feed(&mut state.feed);
            return;
        }
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
        let opening_github = view == RightView::GitHub && self.right_view != RightView::GitHub;
        let opening_tasks = view == RightView::Tarefas && self.right_view != RightView::Tarefas;
        self.right_view = view;
        if view == RightView::Conversa {
            self.unseen = 0;
        }
        if opening_git {
            self.git_refresh();
        }
        if opening_github {
            self.github_refresh();
        }
        if opening_tasks {
            self.tasks_on_open();
        }
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
    // Anexos, arrastar e soltar e ditado por voz
    // ------------------------------------------------------------------

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

    /// Aciona o microfone para `target` (`Tab(uid)`). Abrir o dispositivo pode levar
    /// até 5 s: vai para uma thread, e um segundo clique enquanto isso desiste.
    fn toggle_mic_for(&mut self, target: MicTarget) {
        match dictation::toggle(&mut self.mic_phase, target) {
            ToggleOutcome::StartOpening(_) => {
                self.spawn_bg("mic-open", |_| Some(UiMsg::MicOpened(audio::Recorder::start())));
            }
            ToggleOutcome::CancelOpening => {}
            ToggleOutcome::StopAndTranscribe => self.stop_recording(),
            ToggleOutcome::Busy => {}
        }
    }

    fn on_mic_opened(&mut self, res: Result<audio::Recorder, String>) {
        let was_opening = self.mic_phase.is_opening();
        match res {
            Ok(r) if self.recorder.is_none() && dictation::on_opened(&mut self.mic_phase, true) => {
                self.recorder = Some(r);
                self.recording_since = Some(Instant::now());
            }
            Ok(r) => {
                dictation::on_opened(&mut self.mic_phase, false);
                discard_recorder(r);
            }
            Err(e) => {
                dictation::on_opened(&mut self.mic_phase, false);
                if was_opening {
                    self.system(format!("Microfone indisponível: {e}"));
                }
            }
        }
    }

    /// `recorder.stop()` → transcreve numa thread.
    fn stop_recording(&mut self) {
        let Some(rec) = self.recorder.take() else { return };
        self.recording_since = None;
        dictation::begin_transcribe(&mut self.mic_phase);
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
        color: agentdeck_core::agents::COLOR_SHELL.into(),
        kind: kind.to_owned(),
        double_enter_on_mention: false,
        model: None,
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
    fn ctrl_b_goes_to_the_focused_terminal_unless_the_app_comes_first() {
        // Terminal focado: Ctrl+B é do tmux; Ctrl+T continua do app.
        assert!(!app_claims_ctrl("b", true, false));
        assert!(app_claims_ctrl("t", true, false));
        // Fora do terminal, ou com a opção ligada, todos são do app.
        assert!(app_claims_ctrl("b", false, false));
        assert!(app_claims_ctrl("b", true, true));
    }

    #[test]
    fn a_drag_resizes_the_pty_once_when_the_mouse_is_released() {
        let at = Instant::now();
        let later = at + RESIZE_DEBOUNCE;
        assert!(!resize_due(at - Duration::from_millis(1), at, false), "tamanho ainda mudando");
        assert!(!resize_due(later, at, true), "arrastando: espera soltar o mouse");
        assert!(resize_due(later, at, false), "parado e solto: aplica");
    }

    #[test]
    fn only_our_copies_in_the_attachments_folder_are_deleted() {
        let data = Path::new("/dados");
        assert!(in_attachments_dir(data, &data.join("anexos").join("s_x_1-foto.png")));
        assert!(!in_attachments_dir(data, &data.join("anexos").join("sub").join("a.png")));
        assert!(!in_attachments_dir(data, Path::new("/home/ana/foto.png")));
    }

    #[test]
    fn renaming_a_tab_renames_its_feed_items_and_thinking_entry() {
        let mut feed = vec![item(1, "Claude", Some("Antigravity")), item(2, "Antigravity", Some("Claude")), item(3, "Ollama", None)];
        rename_in_feed(&mut feed, "Claude", "Claude@dev");
        let names: Vec<_> = feed.iter().map(|f| (f.who.as_deref().unwrap(), f.to.as_deref())).collect();
        assert_eq!(names, [("Claude@dev", Some("Antigravity")), ("Antigravity", Some("Claude@dev")), ("Ollama", None)]);

        let mut thinking = HashMap::from([("p#claude".to_owned(), 1), ("q#claude".to_owned(), 9)]);
        rename_thinking(&mut thinking, "p", "claude", "claude@dev");
        assert_eq!(thinking.get("p#claude@dev"), Some(&1));
        assert!(!thinking.contains_key("p#claude"));
        // O mesmo id em outro projeto não é dele.
        assert_eq!(thinking.get("q#claude"), Some(&9));
    }
}

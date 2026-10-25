//! Estado e lógica da interface (porta do `renderer.js`), independente da pintura.
//! A pintura fica em [`crate::ui`]; a janela/loop em [`crate::app`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentdeck_core::delivery::{self, Gate, Mailbox, TabState};
use agentdeck_core::{
    AgentInfo, Core, CoreEvent, Delivery, DictationChoice, DirEntry, DownloadProgress, DownloadState, FeedEntry, FeedKind, Ident,
    Organization, SpawnSpec,
};
use alacritty_terminal::grid::Scroll;
use alacritty_terminal::term::TermMode;
use crossbeam_channel::{Receiver, Sender};
use egui::Color32;
use winit::event::{ElementState, KeyEvent};
use winit::keyboard::{Key, ModifiersState, NamedKey};

use crate::agents::Agents;
use crate::audio;
use crate::dictation::{MicPhase, MicTarget, Setup, TabDelivery, ToggleOutcome};
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
    /// A gravação de um arquivo editado terminou: o documento, o texto gravado e o resultado.
    DocSaved(u64, String, Result<(), String>),
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
    /// Ctrl+S no modo Documentos: grava o arquivo em edição.
    SaveDoc,
    /// Ctrl+F no modo Documentos: abre a busca do documento.
    FindInDoc,
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
    /// Quando o PTY desta aba foi redimensionado por último: o próximo só sai depois que o CLI
    /// terminar de reimprimir a conversa (ver [`Tab::resize_blocked_until`]).
    last_resize: Option<Instant>,
    /// Quando o processo atual foi iniciado (a regra de entrega dá tempo de a TUI subir).
    spawned_at: Option<Instant>,
}

impl Tab {
    /// Um redimensionamento do PTY espera o tamanho parar (ver `Deck::fit_tab`).
    #[cfg(test)]
    pub(crate) fn resize_pending(&self) -> bool {
        self.pending_resize.is_some()
    }

    /// O redimensionamento pendente ainda não pode sair porque o CLI está reimprimindo a conversa do
    /// anterior: só depois da saída parada por [`RESIZE_SETTLE`], com teto de [`RESIZE_SETTLE_MAX`]
    /// para um agente que não para de escrever. `None` = pode sair agora.
    fn resize_blocked_until(&self, now: Instant) -> Option<Instant> {
        let last = self.last_resize?;
        let quiet = (*self.session.shared.last_output.lock()).map_or(last, |at| at.max(last));
        let ready = (quiet + RESIZE_SETTLE).min(last + RESIZE_SETTLE_MAX);
        (now < ready).then_some(ready)
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
    /// Modelo (`claude`, `antigravity`…): escolhe o logo.
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
    pub organization: Organization,
    /// Destaque: fração da largura do centro que a janela grande ocupa (ver `split::highlight_ratio`).
    pub highlight_ratio: f32,
    /// O grupo de modelos fixado na barra de título, por agente (cópia de `Config::usage_pins`).
    pub usage_pins: std::collections::BTreeMap<String, String>,
    /// Ordem das janelas (grupos de `split::group_key`).
    pub group_order: Vec<String>,
    pub split_ratios: split::Ratios,
    /// Janelas do último quadro (atalhos de teclado e tamanho das abas novas).
    pub panes: Vec<PaneSlot>,
    /// Janela sendo arrastada pela alça para trocar de lugar com outra.
    pub pane_drag: Option<usize>,
    /// Até quando o tamanho das janelas mudou por uma troca de aba no Destaque (ver `Deck::activate`):
    /// nesse prazo o PTY espera `HIGHLIGHT_RESIZE_DEBOUNCE` em vez de `RESIZE_DEBOUNCE`.
    highlight_switch_until: Option<Instant>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=split-drag`): janela-alvo fictícia do arrasto.
    pub debug_pane_drop: Option<usize>,
    show_seq: u64,

    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=usage-demo`): uso fictício na barra de título.
    pub debug_usage: Option<HashMap<String, agentdeck_core::AgentUsage>>,
    /// SOMENTE DEPURAÇÃO: o painel de uso aberto sem o mouse, do agente dado ou, vazio, do primeiro
    /// (`AGENTDECK_DEBUG_UI=usage-panel` ou `usage-panel:<agente>`).
    pub debug_usage_panel: Option<String>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=projects-demo`): projetos fictícios na barra lateral.
    pub debug_projects: Option<Vec<ProjectView>>,
    /// SOMENTE DEPURAÇÃO (`AGENTDECK_DEBUG_UI=project:<id>`): troca para este projeto logo depois de restaurar as abas.
    pub debug_switch_project: Option<String>,
    /// O app deve encerrar (o `App` encerra depois do quadro, ou do trabalho sem quadro com a janela escondida).
    pub quit_requested: bool,
    /// O botão de fechar da barra de título foi clicado: o mesmo que o fechar do sistema (o `App` esconde na bandeja
    /// ou minimiza, depois do quadro).
    pub close_requested: bool,
    /// Uma segunda abertura do app pediu a janela (`CoreEvent::Activate`): o `App` a mostra e foca.
    pub activate_requested: bool,

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
    /// A pergunta do ditado por voz (baixar o Whisper?) está aberta.
    pub dictation_prompt: bool,
    /// O modelo e um motor do whisper estão no disco (lido do disco na abertura e quando o core avisa).
    dictation_installed: bool,
    /// Configurações: o botão "Remover" já foi clicado uma vez e pede confirmação.
    pub dictation_confirm_remove: bool,
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
    /// `AGENTDECK_DEBUG_PTY=1`: grava a saída crua de cada aba em `dados/pty/` (ver `terminal::record`).
    debug_pty: bool,
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
/// Trocar de aba no Destaque muda a largura das janelas sem o usuário arrastar nada; o CLI pode
/// ainda estar reimprimindo a conversa da troca anterior, então o PTY espera mais.
const HIGHLIGHT_RESIZE_DEBOUNCE: Duration = Duration::from_millis(600);
/// Depois de um redimensionamento aplicado, o próximo da mesma aba espera a saída dela ficar parada
/// por este tempo (o CLI acabou de reimprimir a conversa)…
const RESIZE_SETTLE: Duration = Duration::from_millis(300);
/// …mas um agente que não para de escrever não pode segurar o redimensionamento para sempre.
const RESIZE_SETTLE_MAX: Duration = Duration::from_secs(3);
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

/// Abre uma pasta no explorador de arquivos do sistema.
fn open_in_file_manager(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
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
        let dictation_installed = core.dictation_installed();
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
            organization: cfg.organization(),
            highlight_ratio: split::highlight_ratio(cfg.highlight_ratio),
            usage_pins: cfg.usage_pins.clone(),
            group_order: Vec::new(),
            split_ratios: cfg
                .active()
                .map(|p| split::Ratios { rows: p.split_rows.clone(), cols: p.split_cols.clone() })
                .unwrap_or(split::Ratios { rows: Vec::new(), cols: Vec::new() }),
            panes: Vec::new(),
            pane_drag: None,
            highlight_switch_until: None,
            debug_pane_drop: None,
            show_seq: 0,
            debug_usage: None,
            debug_usage_panel: None,
            debug_projects: None,
            debug_switch_project: None,
            quit_requested: false,
            close_requested: false,
            activate_requested: false,
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
            dictation_prompt: false,
            dictation_installed,
            dictation_confirm_remove: false,
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
            debug_pty: agentdeck_core::env_var("DEBUG_PTY").is_some_and(|v| v == "1"),
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

    /// `api.listAgents()` numa thread (procura os CLIs no disco).
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
}

// Cada seção em um arquivo (filhos deste módulo: enxergam os campos privados de `Deck`).
mod attachments;
mod dictation;
mod feed;
mod frame;
mod keyboard;
mod projects;
mod sidebar;
mod tabs;

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

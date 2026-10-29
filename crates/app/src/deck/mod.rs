//! Interface state and logic (port of `renderer.js`), independent of painting.
//! Painting lives in [`crate::ui`]; the window/loop in [`crate::app`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use agentdeck_core::delivery::{self, Gate, Mailbox, TabState};
use agentdeck_core::t;
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

/// Destination of files attached by the attach button, drag and drop or Ctrl+V of an image.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    /// Types the attached paths into the tab's PTY (`uid`) (no Enter).
    Tab(u64),
    /// Adds the attachments to the "New task" form of project `project_id`.
    TaskForm(String),
}

/// Area highlighted while files are dragged over the window (`ui/overlay.rs`).
#[derive(Debug, Clone)]
pub struct DropZone {
    pub rect: egui::Rect,
    pub target: AttachTarget,
    pub title: String,
    pub subtitle: String,
}

/// Task messages from background work to the UI.
pub enum UiMsg {
    Agents(Vec<AgentInfo>),
    /// Attachments copied (absolute paths under `data/attachments`) for a tab or a task form.
    Attached(AttachTarget, Vec<PathBuf>),
    Transcribed(Result<String, String>),
    Engine(String),
    WorkspacePicked(PathBuf),
    /// Result of opening the microphone (done outside the interface thread).
    MicOpened(Result<audio::Recorder, String>),
    /// The user confirmed removing the project (id) from the list.
    RemoveProject(String),
    /// Git reply, with the generation of the project it was asked for (see `GitUi::generation`).
    Git(u64, crate::git_ui::GitReply),
    /// Known models of an agent (new-agent menu); empty = default only.
    Models(String, Vec<String>),
    /// Tasks desk reply, with the generation of the project it was asked for (see `TasksUi::generation`).
    Tasks(u64, crate::tasks_ui::TaskReply),
    /// A document of the Documents mode finished loading (document uid).
    Doc(u64, crate::docs::Loaded),
    /// The save of an edited file finished: the document, the saved text and the result.
    DocSaved(u64, String, Result<(), String>),
}

/// Actions from the window (global shortcuts) applied on the next frame.
#[derive(Debug, Clone)]
pub enum Action {
    NewTabShortcut,
    ToggleSidebar,
    CycleTab {
        back: bool,
    },
    /// Ctrl+Alt+arrows: focus to the neighboring window of the split view.
    FocusPane {
        dx: i32,
        dy: i32,
    },
    /// Ctrl+Shift+M: starts/stops dictation on the focused CLI tab (or on the one already recording).
    ToggleTabMic,
    CopyText(String),
    /// Ctrl+Shift+D: switches the center between Agents and Documents.
    ToggleCenter,
    /// Ctrl+S in Documents mode: saves the file being edited.
    SaveDoc,
    /// Ctrl+F in Documents mode: opens the document search.
    FindInDoc,
}

/// Message from another agent (or from the UI) waiting to be typed into the tab's terminal.
struct Queued {
    /// Question number in the core; 0 = message from the interface itself (e.g. GitHub tab), with no answer to return.
    request: u64,
    prompt: String,
    /// It is a late answer to a question of this tab, not a new question.
    answer: bool,
}

/// What the right bar shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RightView {
    /// Tasks desk (see `tasks_ui.rs`).
    Tasks,
    Chat,
    /// The workspace git repository (see `git_ui.rs`).
    Git,
    /// GitHub PRs, issues and Actions (see `github_ui.rs`).
    GitHub,
}

pub struct Tab {
    pub uid: u64,
    pub kind: String,
    /// Instance identity (`claude`, `claude@dev`, `claude@dev@ana`).
    pub ident: Ident,
    /// Project id of the instance (the core instances are per project).
    pub project: String,
    pub label: String,
    /// Instance LLM model (`sonnet`); `None` = the CLI default or an agent that does not choose.
    pub model: Option<String>,
    pub color: Color32,
    pub session: Session,
    pub view: ViewState,
    pub alive: bool,
    pub busy: bool,
    /// A turn finished while it was not active (cleared when the tab is focused).
    pub done: bool,
    /// When the tab was last activated (counter): each window of the split view
    /// shows the most recent one.
    pub shown: u64,
    /// Opened by the app at the request of `ask-agent` (and not by the user).
    pub auto: bool,
    /// Questions from other agents waiting for the tab to be ready (one at a time).
    queue: Mailbox<Queued>,
    /// Last user key in this tab and whether it left text started on the line.
    last_key: Option<Instant>,
    typing: bool,
    /// The agent reports the start/end of a turn through a hook (`ask-agent --signal`): `busy` comes from it.
    hooked: bool,
    /// The hook confirmed that the current turn started: only the end-of-turn notice frees the tab. Without
    /// confirmation (e.g. Enter on a menu, which does not open a turn), the output heuristic applies.
    turn_confirmed: bool,
    /// Scheduled Enters of a submit (`submit`), in order.
    pending_enter: VecDeque<Instant>,
    /// User keys that arrived while a submit was waiting for the Enter: they go after it, so they do
    /// not land in the middle of the message.
    held_keys: Vec<u8>,
    seen_output: u64,
    idle_deadline: Option<Instant>,
    /// There has been output since the tab became busy (only then the end of the turn rings and marks "done").
    busy_output: bool,
    spec: Option<SpawnSpec>,
    pending_resize: Option<(GridSize, Instant)>,
    /// When this tab's PTY was last resized: the next one only goes out after the CLI
    /// finishes reprinting the conversation (see [`Tab::resize_blocked_until`]).
    last_resize: Option<Instant>,
    /// When the current process was started (the delivery rule gives the TUI time to come up).
    spawned_at: Option<Instant>,
}

impl Tab {
    /// A PTY resize waits for the size to stop (see `Deck::fit_tab`).
    #[cfg(test)]
    pub(crate) fn resize_pending(&self) -> bool {
        self.pending_resize.is_some()
    }

    /// The pending resize cannot go out yet because the CLI is reprinting the previous conversation:
    /// only after the output has been quiet for [`RESIZE_SETTLE`], with a ceiling of [`RESIZE_SETTLE_MAX`]
    /// for an agent that never stops writing. `None` = it can go out now.
    fn resize_blocked_until(&self, now: Instant) -> Option<Instant> {
        let last = self.last_resize?;
        let quiet = (*self.session.shared.last_output.lock()).map_or(last, |at| at.max(last));
        let ready = (quiet + RESIZE_SETTLE).min(last + RESIZE_SETTLE_MAX);
        (now < ready).then_some(ready)
    }
}

/// One window of the split view, as it was on the last frame.
pub struct PaneSlot {
    /// Whole window (tab header + terminal).
    pub rect: egui::Rect,
    /// Terminal area.
    pub body: egui::Rect,
    pub uids: Vec<u64>,
    /// `uid` of the visible tab (`plan.shown`) of this window.
    pub shown_uid: u64,
    /// Window groups (more than one when they all do not fit and the remaining ones share the last).
    pub groups: Vec<String>,
}

/// An agent (tab) of a project, for the sidebar project list.
#[derive(Debug, Clone)]
pub struct AgentBadge {
    /// Instance id (`claude@dev`).
    pub id: String,
    /// Instance boss (`None` = answers the user). Comes from the core (`Core::set_boss`).
    pub parent: Option<String>,
    /// Model (`claude`, `antigravity`…): chooses the logo.
    pub kind: String,
    /// Label shown (`Claude · Dev`).
    pub label: String,
    /// Instance LLM model, when it has one.
    pub model: Option<String>,
    pub color: Color32,
    /// Working right now.
    pub busy: bool,
    /// Finished a turn without being seen.
    pub done: bool,
    /// The process is still running.
    pub alive: bool,
    /// Index in `Deck::tabs` (clicking activates the tab); `None` if the project has not been loaded yet.
    pub tab: Option<usize>,
}

/// A project in the sidebar.
#[derive(Debug, Clone)]
pub struct ProjectView {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    pub active: bool,
    /// The agent list shows under the project.
    pub expanded: bool,
    pub agents: Vec<AgentBadge>,
}

/// What the Deck remembers about a project while it is not on screen.
#[derive(Default)]
struct ProjectState {
    /// The saved tabs were already opened in this session (until then, they are only registered in the core).
    loaded: bool,
    /// The active tab when the project left the screen.
    active_uid: Option<u64>,
    group_order: Vec<String>,
    split_ratios: split::Ratios,
    /// Integrated conversation (the active project's lives in `Deck`).
    feed: Vec<FeedItem>,
}

/// What the split view shows on this frame.
pub struct PanePlan {
    pub rect: egui::Rect,
    /// Indices of the window's tabs, in header order.
    pub tabs: Vec<usize>,
    /// The visible tab (the most recently activated one).
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

/// Flattened row of the file tree.
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
    /// Pending single click (220 ms debounce to tell it apart from a double click).
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
    /// Requested right bar width (the effective one depends on the window; see `ui::right_width`).
    pub right_w: f32,
    pub right_dragging: Option<(f32, f32)>,
    pub right_view: RightView,
    pub right_panel_rect: Option<egui::Rect>,

    // Split view (see `split.rs`)
    pub organization: Organization,
    /// Highlight: fraction of the center width the large window takes (see `split::highlight_ratio`).
    pub highlight_ratio: f32,
    /// The model group pinned in the title bar, per agent (a copy of `Config::usage_pins`).
    pub usage_pins: std::collections::BTreeMap<String, String>,
    /// Order of the windows (`split::group_key` groups).
    pub group_order: Vec<String>,
    pub split_ratios: split::Ratios,
    /// Windows of the last frame (keyboard shortcuts and size of new tabs).
    pub panes: Vec<PaneSlot>,
    /// Window being dragged by the handle to swap places with another.
    pub pane_drag: Option<usize>,
    /// Until when the window size changed due to a tab switch in the Highlight (see `Deck::activate`):
    /// within that period the PTY waits `HIGHLIGHT_RESIZE_DEBOUNCE` instead of `RESIZE_DEBOUNCE`.
    highlight_switch_until: Option<Instant>,
    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=split-drag`): fake target window of the drag.
    pub debug_pane_drop: Option<usize>,
    show_seq: u64,

    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=usage-demo`): fake usage in the title bar.
    pub debug_usage: Option<HashMap<String, agentdeck_core::AgentUsage>>,
    /// DEBUG ONLY: the usage panel opened without the mouse, for the given agent or, empty, the first
    /// (`AGENTDECK_DEBUG_UI=usage-panel` or `usage-panel:<agent>`).
    pub debug_usage_panel: Option<String>,
    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=projects-demo`): fake projects in the sidebar.
    pub debug_projects: Option<Vec<ProjectView>>,
    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=project:<id>`): switches to this project right after restoring the tabs.
    pub debug_switch_project: Option<String>,
    /// The app must quit (the `App` quits after the frame, or after the frame-less work with the window hidden).
    pub quit_requested: bool,
    /// The title bar's close button was clicked: the same as the system close (the `App` hides in the tray
    /// or minimizes, after the frame).
    pub close_requested: bool,
    /// A second launch of the app asked for the window (`CoreEvent::Activate`): the `App` shows and focuses it.
    pub activate_requested: bool,

    /// Git tab (see `git_ui.rs`).
    pub git: crate::git_ui::GitUi,
    /// GitHub tab (see `github_ui.rs`).
    pub github: crate::github_ui::GithubUi,
    /// Tasks tab (see `tasks_ui.rs`).
    pub tasks: crate::tasks_ui::TasksUi,
    /// Center Documents mode (see `docs.rs`).
    pub docs: crate::docs::Docs,
    /// What the center showed on the previous frame (to notice the switch; see `Deck::center_changed`).
    pub(crate) center_seen: crate::docs::CenterView,

    // Tree and Projects
    /// Folder of the active project (empty with no project).
    pub workspace: PathBuf,
    /// Id of the active project (empty = none). Only its tabs appear.
    pub active_project: String,
    projects: HashMap<String, ProjectState>,
    pub tree: TreeState,

    // Integrated conversation (read only)
    pub feed: Vec<FeedItem>,
    thinking: HashMap<String, u64>,
    next_feed_id: u64,
    pub unseen: u32,
    /// Download in progress (bar in the header) and, for an error, when it disappears.
    pub download: Option<(DownloadProgress, Option<Instant>)>,
    /// DEBUG ONLY: keeps the fake state of `AGENTDECK_DEBUG_UI=download*`, ignoring the real download.
    pub debug_hold_download: bool,
    pub feed_scroll_to_bottom: bool,
    pub feed_near_bottom: bool,

    // Voice dictation (pinned to the `uid` of the tab where it started)
    recorder: Option<audio::Recorder>,
    pub recording_since: Option<Instant>,
    /// Global microphone state and its destination (`Tab(uid)`).
    pub mic_phase: MicPhase,
    pub engine_label: String,

    // Menus and misc
    pub new_tab_menu_open: bool,
    /// Agent whose second menu step (choose the model) is open.
    pub new_tab_models_for: Option<String>,
    /// Provider chosen in the model menu (`opencode`, `opencode-go`…): the list shows only its
    /// models. `None` = the providers step.
    pub new_tab_provider: Option<String>,
    /// Known models of each agent and when they arrived (fetched outside the interface thread).
    model_lists: HashMap<String, (Vec<String>, Instant)>,
    /// Agents whose model list is being fetched right now.
    models_loading: HashSet<String>,
    pub settings_open: bool,
    /// The voice dictation question (download Whisper?) is open.
    pub dictation_prompt: bool,
    /// The whisper model and an engine are on disk (read from disk at startup and when the core notifies).
    dictation_installed: bool,
    /// Settings: the "Remove" button was already clicked once and asks for confirmation.
    pub dictation_confirm_remove: bool,
    /// Tab being renamed by the user (index, edit buffer, optional error).
    pub rename_tab: Option<(usize, String, Option<String>)>,
    /// Context menu open on a tab (tab index, click position).
    pub tab_context_menu: Option<(usize, egui::Pos2)>,
    pub sound_enabled: bool,
    /// Mirror of `Config.auto_open_agent_tabs` (settings menu item).
    pub auto_open_agent_tabs: bool,
    /// Mirror of `Config.app_shortcuts_first` (settings menu item).
    pub app_shortcuts_first: bool,
    /// A modal is open (from the last frame): the keyboard goes neither to the terminal nor to the shortcuts.
    pub modal_open: bool,
    /// Ctrl+/ requested by the keyboard (opens/closes the shortcuts modal on the next frame).
    pub toggle_shortcuts: bool,
    pub drop_hover: bool,
    pub last_drop_target: Option<AttachTarget>,
    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=drop-pane:<n>`): highlights this panel on drag and drop.
    pub debug_drop_slot: Option<usize>,
    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=drop-task`): highlights the "New task" form on drag and drop.
    pub debug_drop_task_form: bool,
    pub term_focus: bool,
    /// IME composition text (pre-edit), shown over the terminal cursor.
    pub ime_preedit: String,
    pub metrics: Option<Metrics>,
    pub blink_epoch: Instant,
    pub icons: IconCache,
    pub terminal_rect: Option<egui::Rect>,
    pub window_focused: bool,
    debug_fake_shell: bool,
    /// `AGENTDECK_DEBUG_PTY=1`: records each tab's raw output under `data/pty/` (see `terminal::record`).
    debug_pty: bool,
    /// DEBUG ONLY: closes the restored tabs (empty-state screenshot).
    pub debug_close_all: bool,
}

const PERSIST_DEBOUNCE: Duration = Duration::from_millis(120);
/// How many tabs the app opens by itself (at the request of `ask-agent`) at the same time.
const MAX_AUTO_TABS: usize = 8;
/// How long a model list received from an agent is valid before asking again.
const MODEL_LIST_TTL: Duration = Duration::from_secs(300);
/// After pasting a submit, the Enter goes out after this time (and again after `SECOND_ENTER` when there is
/// an `@` mention in CLIs with file autocomplete).
const FIRST_ENTER: Duration = Duration::from_millis(250);
const SECOND_ENTER: Duration = Duration::from_millis(600);
/// With hooks, the tab only stops being busy through the end-of-turn notice. If the notice is lost,
/// it is freed after this time with no output.
const HOOK_STALE: Duration = Duration::from_secs(600);
/// Every new width makes Claude Code reprint the conversation, and on Windows without `ESC[3J`: the previous
/// copy stays in the history. That is why the PTY only changes with the size stopped and the mouse released
/// (see [`resize_due`]): a drag of the divider or the edge becomes a single resize.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(150);
/// Switching tabs in the Highlight changes the window widths without the user dragging anything; the CLI may
/// still be reprinting the previous switch's conversation, so the PTY waits longer.
const HIGHLIGHT_RESIZE_DEBOUNCE: Duration = Duration::from_millis(600);
/// After an applied resize, the next one of the same tab waits for its output to stay quiet
/// for this time (the CLI just reprinted the conversation)…
const RESIZE_SETTLE: Duration = Duration::from_millis(300);
/// …but an agent that never stops writing cannot hold the resize forever.
const RESIZE_SETTLE_MAX: Duration = Duration::from_secs(3);
const IDLE_CHIME: Duration = Duration::from_millis(1200);
/// An Enter (or submit) that produced no output at all (empty prompt, menu): the tab is freed after
/// this time, silently. Without it the tab stayed "working" forever and held the bridge queue.
const NO_OUTPUT_IDLE: Duration = Duration::from_secs(8);
/// Messages kept in each project's Integrated conversation (the oldest leave): the list is
/// redrawn every frame and, with no limit, a long session made the interface slow.
const FEED_MAX: usize = 1000;

fn cap_feed(feed: &mut Vec<FeedItem>) {
    if feed.len() > FEED_MAX {
        feed.drain(..feed.len() - FEED_MAX);
    }
}

/// Closes the microphone on a thread (waiting for the capture to finish is not the interface thread's job).
fn discard_recorder(rec: audio::Recorder) {
    let _ = std::thread::Builder::new().name("mic-close".into()).spawn(move || rec.cancel());
}

/// Absolute paths of the attachments from the core references (relative to the workspace at copy time,
/// or absolute). Every attachment lives in `data/attachments` with a unique name, so the name is enough
/// and does not depend on which project was active.
fn attachment_paths(core: &Core, refs: Vec<String>) -> Vec<PathBuf> {
    let dir = core.paths().data_dir.join("attachments");
    refs.iter().filter_map(|r| Path::new(r).file_name()).map(|name| dir.join(name)).collect()
}

/// Opens a folder in the system file manager.
fn open_in_file_manager(path: &Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(path).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(path).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(path).spawn();
}

/// The pending PTY resize can be applied: size stopped and mouse released.
fn resize_due(now: Instant, at: Instant, dragging: bool) -> bool {
    now >= at && !dragging
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
            // The tasks desk is the first tab and the one that opens: the app does not save the chosen tab.
            right_view: RightView::Tasks,
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
        // The bridge now types the agents' questions into the tabs (instead of running the agent separately).
        deck.core.enable_tab_delivery();
        if agentdeck_core::env_var("DEBUG_FEED").is_some_and(|v| v == "1") {
            deck.debug_feed();
        }
        // The branch chip in the top bar shows right away, and so does the number of tasks under review on the tab icon.
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

    /// `api.listAgents()` on a thread (looks for the CLIs on disk).
    pub fn refresh_agents(&self) {
        self.spawn_bg("list-agents", |core| Some(UiMsg::Agents(core.list_agents())));
    }

    /// Asks for the known models of `kind` (on a thread; the core caches them). Does not repeat while
    /// a request is in flight nor while the received list is recent.
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

    /// Model list of `kind` for the menu and whether it is still being fetched.
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

// Each section in a file (children of this module: they see `Deck`'s private fields).
mod attachments;
mod dictation;
mod feed;
mod frame;
mod keyboard;
mod projects;
mod sidebar;
mod tabs;

/// Reads text from the system clipboard. Only from the UI thread (see `check_clipboard_image`).
fn clipboard_text() -> Option<String> {
    arboard::Clipboard::new().ok()?.get_text().ok()
}

/// Home folder (start of the folder picker when there is no project).
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

    #[test]
    fn a_drag_resizes_the_pty_once_when_the_mouse_is_released() {
        let at = Instant::now();
        let later = at + RESIZE_DEBOUNCE;
        assert!(!resize_due(at - Duration::from_millis(1), at, false), "size still changing");
        assert!(!resize_due(later, at, true), "dragging: waits for the mouse to be released");
        assert!(resize_due(later, at, false), "stopped and released: applies");
    }
}

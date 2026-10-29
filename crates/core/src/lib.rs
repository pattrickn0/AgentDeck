//! Holt — the core (backend) without graphical interface dependencies.
//!
//! This crate is the Rust equivalent of `main.js` of the Electron version.
//! The interface (crate `holt`) talks to it exclusively through [`Core`]
//! and the [`CoreEvent`] events — the same role the IPC/preload had in Electron.
//!
//! STABLE CONTRACT: the public types and signatures of this file and of `events.rs`,
//! `agents.rs` (types `AgentInfo`/`SpawnSpec`), `config.rs` (`Config`) and
//! `workspace.rs` (`DirEntry`) are consumed by the UI. Do not change them without aligning with the lead.

pub mod activation;
pub mod activity;
pub mod agents;
pub mod attachments;
pub mod bridge;
pub mod capability;
pub mod config;
pub mod context;
pub mod delivery;
pub mod download;
pub mod env;
pub mod events;
pub mod exchange;
pub(crate) mod file_lock;
pub mod git;
pub mod github;
pub mod headless;
pub mod highlight;
pub mod i18n;
pub mod inbox;
pub mod instances;
pub mod learnings;
pub mod models;
pub mod outline;
pub mod paths;
pub(crate) mod process;
pub mod resume;
pub mod scope;
pub mod sessions;
pub mod situation;
pub mod tasks;
pub mod team;
pub(crate) mod trust;
pub mod usage;
pub mod util;
pub mod whisper;
pub mod workspace;

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::{Mutex, RwLock};

pub use agents::{AgentInfo, SpawnSpec};
pub use config::{Config, DictationChoice, Organization, ProjectCfg};
pub use env::{env_var, env_var_os};
pub use events::{CoreEvent, Delivery, DownloadProgress, DownloadState, FeedEntry, FeedKind};
pub use exchange::PendingInfo;
pub use headless::HeadlessOptions;
pub use instances::Ident;
pub use tasks::{MergeBlock, MergeOutcome, MergeRefusal, Task, TaskError, TaskEvent, TaskMessage, TaskSpec, TaskStatus};
pub use team::Team;
pub use usage::{AgentUsage, UsageGroup, UsageWindow};
pub use workspace::{DirEntry, Eol, TextFile};

/// Function called whenever a [`CoreEvent`] is emitted, to wake the UI loop
/// (e.g. `EventLoopProxy::send_event`). It can be called from any thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Result of a transcription.
#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    /// Label of the engine used (e.g. "GPU (Vulkan)").
    pub engine: String,
}

/// Startup options for tests and for embedding the core. The default is the
/// normal behavior of the app.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// PATH used to find the agent CLIs (`None` = the process PATH).
    pub search_path: Option<OsString>,
    /// Home folder used in the agent resolution and in the rule "never write AGENTS.md in the
    /// user's folder" (`None` = the system one).
    pub home_dir: Option<PathBuf>,
    /// Watch the workspace and emit [`CoreEvent::WorkspaceChanged`].
    pub watch_workspace: bool,
    /// Configuration to use instead of reading `paths.config_file` (not saved until `update_config`).
    pub config: Option<Config>,
    /// Also look for CLIs in the system folders (`APPDATA`, `LOCALAPPDATA`, `ProgramFiles`).
    /// The tests turn it off, so they do not depend on what is installed on the machine.
    pub system_dirs: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self { search_path: None, home_dir: None, watch_workspace: true, config: None, system_dirs: true }
    }
}

/// Main backend handle. Cheap to clone; safe between threads.
#[derive(Clone)]
pub struct Core {
    pub(crate) inner: Arc<CoreInner>,
}

pub(crate) struct CoreInner {
    pub paths: paths::AppPaths,
    pub config: RwLock<Config>,
    pub events_tx: Sender<CoreEvent>,
    pub waker: RwLock<Option<Waker>>,
    pub state: state::SharedState,
    pub home: PathBuf,
    pub watch_workspace: bool,
    pub agents: agents::Registry,
    pub providers: context::Providers,
    pub processes: process::ProcessRegistry,
    /// Key of the pair (with the project scope) → id of the headless session/conversation.
    pub headless_sessions: Mutex<HashMap<String, String>>,
    /// Agent instances (one per tab) of each project (project id → instances).
    pub instances: Mutex<HashMap<String, instances::Instances>>,
    /// Questions waiting for an answer between tabs (scoped keys: `instances::scoped`).
    pub exchange: exchange::Exchange,
    /// Answers received by each instance that asked (`ask-agent --inbox`).
    pub inbox: inbox::Inbox,
    /// Last activity of each tab (scoped key); a question's deadline counts from it.
    pub activity: activity::Activity,
    /// Projects other than the active one whose AGENTS.md was already written in this session (in their first tab).
    pub context_written: Mutex<HashSet<String>>,
    /// The UI knows how to deliver messages in the tabs (otherwise the bridge answers in headless mode).
    pub tab_delivery: std::sync::atomic::AtomicBool,
    /// Token of each agent process → who it is (the bridge does not trust the `from` of the body).
    pub callers: Mutex<HashMap<String, Caller>>,
    /// Instances (scoped keys) whose position in the team changed since the last message they
    /// received: the next one carries the notice (see `team::notice`).
    pub team_changed: Mutex<HashSet<String>>,
    /// Serializes the config saves (see `Core::update_config`).
    pub config_save: Mutex<()>,
    /// The config changes stay only in memory (screenshots and debug states).
    pub config_read_only: std::sync::atomic::AtomicBool,
    pub bridge_queues: bridge::PairQueues,
    /// Known models of each agent (new agent menu), cached.
    pub model_lists: models::ModelLists,
    pub bridge_server: Mutex<Option<Arc<tiny_http::Server>>>,
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub whisper: whisper::WhisperState,
    pub git: git::GitState,
    pub github: github::GithubState,
    /// Usage of the agents' plans (title bar).
    pub usage: usage::UsageStore,
    /// Tasks (task desk) of each project, in `data/tasks/<project>.json`.
    pub tasks: tasks::TaskStore,
    /// Lock of the data folder (one instance of the app per `data/`); released on shutdown.
    data_lock: Mutex<Option<std::fs::File>>,
}

/// Who called the bridge, by the token it received in the environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// Instance id (`claude`, `claude@dev`…), without the project.
    pub id: String,
    /// Project of the instance: the bridge only sees the instances of it.
    pub project: String,
    /// It is the process of the tab (interactive). `false` = headless run (bridge without tabs).
    pub tab: bool,
}

/// Result of authenticating a request to the bridge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// Master token (tests and debugging tools): the `from` of the body is accepted, validated.
    Master,
    /// Token of an agent process: the `from` is its own, always.
    Agent(Caller),
}

/// Token of a headless process; forgotten when the process ends.
pub(crate) struct TokenGuard {
    inner: Arc<CoreInner>,
    pub token: String,
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        self.inner.callers.lock().remove(&self.token);
    }
}

/// Comparison in constant time (the token does not leak through the response time).
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Lock file of the data folder: one instance of the app per folder.
const LOCK_FILE: &str = ".holt.lock";

/// Locks `data/.holt.lock`. Error if another instance of the app already uses the folder.
fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(LOCK_FILE);
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
    let busy = match file.try_lock() {
        Ok(()) => false,
        Err(std::fs::TryLockError::WouldBlock) => true,
        Err(std::fs::TryLockError::Error(e)) => {
            log::warn!("could not lock {}: {e}", path.display());
            false
        }
    };
    if busy {
        anyhow::bail!(crate::t!(
            "Another Holt window is already using the data folder {dir}.\n\nClose it, or set HOLT_HOME to another \
             folder to open a separate instance.",
            dir = data_dir.display()
        ));
    }
    Ok(file)
}

pub(crate) mod state {
    //! Simple shared mutable state.
    #[derive(Default)]
    pub struct SharedState {
        pub bridge_port: parking_lot::Mutex<u16>,
        pub bridge_token: String,
        pub session_id: String,
        /// Shutdown signal (interrupts the headless runs of the bridge).
        pub shutdown: std::sync::atomic::AtomicBool,
    }
}

/// Random 128-bit token in hexadecimal.
fn random_token() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        // Unlikely fallback: UUID v4 (122 random bits).
        buf = *uuid::Uuid::new_v4().as_bytes();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

impl Core {
    /// Initializes the backend: loads the config, prepares the workspace (AGENTS.md + watcher),
    /// registers the session/retention cleanup and starts the HTTP bridge on 127.0.0.1:0.
    /// Returns the handle and the event receiver for the UI.
    pub fn start(paths: paths::AppPaths) -> anyhow::Result<(Core, Receiver<CoreEvent>)> {
        Self::start_with(paths, StartOptions::default())
    }

    /// Like [`Core::start`], with options (tests / embedded use).
    pub fn start_with(paths: paths::AppPaths, opts: StartOptions) -> anyhow::Result<(Core, Receiver<CoreEvent>)> {
        let data_lock = lock_data_dir(&paths.data_dir)?;
        // Whoever takes the lock is the only instance: what was left by one that crashed means nothing.
        activation::forget(&paths.data_dir);
        let (tx, rx) = crossbeam_channel::unbounded();
        let home = opts.home_dir.clone().unwrap_or_else(util::home_dir);
        let mut config = match opts.config {
            Some(c) => c,
            None => Config::load_from(&config::config_candidates(&paths), &paths, &home),
        };
        config.fix_workspace(&paths, &home);
        // Before the first frame: the UI reads the language on every `t!`.
        i18n::set_lang(config.language);
        let providers = context::Providers::default();
        let inner = CoreInner {
            config: RwLock::new(config),
            events_tx: tx,
            waker: RwLock::new(None),
            state: state::SharedState { bridge_token: random_token(), session_id: sessions::new_session_id(), ..Default::default() },
            agents: agents::Registry::new(agents::Discovery {
                search_path: opts.search_path,
                home: home.clone(),
                system_dirs: opts.system_dirs,
            }),
            home,
            watch_workspace: opts.watch_workspace,
            providers,
            processes: process::ProcessRegistry::default(),
            headless_sessions: Mutex::new(HashMap::new()),
            instances: Mutex::new(HashMap::new()),
            exchange: exchange::Exchange::default(),
            inbox: inbox::Inbox::default(),
            activity: activity::Activity::default(),
            context_written: Mutex::new(HashSet::new()),
            tab_delivery: std::sync::atomic::AtomicBool::new(false),
            callers: Mutex::new(HashMap::new()),
            team_changed: Mutex::new(HashSet::new()),
            config_save: Mutex::new(()),
            config_read_only: std::sync::atomic::AtomicBool::new(false),
            bridge_queues: bridge::PairQueues::default(),
            model_lists: models::ModelLists::default(),
            bridge_server: Mutex::new(None),
            watcher: Mutex::new(None),
            whisper: whisper::WhisperState::default(),
            git: git::GitState::default(),
            github: github::GithubState::default(),
            usage: usage::UsageStore::load(paths.data_dir.join(usage::USAGE_FILE)),
            tasks: tasks::TaskStore::default(),
            data_lock: Mutex::new(Some(data_lock)),
            paths,
        };
        let core = Core { inner: Arc::new(inner) };

        core.prepare_workspace();
        sessions::register_and_cleanup(&core.paths().data_dir, &core.inner.state.session_id);
        let (server, port) = bridge::start(&core)?;
        *core.inner.bridge_server.lock() = Some(server);
        *core.inner.state.bridge_port.lock() = port;
        activation::publish(&core.paths().data_dir, port, core.bridge_token());

        usage::start_poller(&core);

        Ok((core, rx))
    }

    /// Sets the function that wakes the UI on every emitted event.
    pub fn set_waker(&self, waker: Waker) {
        *self.inner.waker.write() = Some(waker);
    }

    /// Tab `id` of project `project` is working now (the UI calls it while it
    /// produces output or is in a turn).
    pub fn touch_activity(&self, project: &str, id: &str, at: std::time::Instant) {
        self.inner.activity.touch(&instances::scoped(project, id), at);
    }

    /// Emits an event to the UI (and wakes the loop).
    pub fn emit(&self, ev: CoreEvent) {
        let _ = self.inner.events_tx.send(ev);
        if let Some(w) = self.inner.waker.read().as_ref() {
            w();
        }
    }

    /// Shortcut to emit an entry in the "integrated conversation".
    pub fn feed(&self, entry: FeedEntry) {
        self.record_task_message(&entry);
        self.emit(CoreEvent::Feed(entry));
    }

    pub fn paths(&self) -> &paths::AppPaths {
        &self.inner.paths
    }

    /// Id of this app session (base36 of the start instant).
    pub fn session_id(&self) -> &str {
        &self.inner.state.session_id
    }

    /// Registers a context provider (extension point for RAG; none by default).
    pub fn register_context_provider(&self, provider: Arc<dyn context::ContextProvider>) {
        self.inner.providers.write().push(provider);
    }

    // ---------- Configuration (config:get / config:save) ----------

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    /// Applies a change and persists it to disk.
    /// From now on the configuration changes only in memory, without writing the file (debugging).
    pub fn disable_config_writes(&self) {
        self.inner.config_read_only.store(true, Ordering::SeqCst);
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        // One save at a time and in the order of the changes (the bridge and the UI save from different
        // threads): without this an old copy could land on top of the new one.
        let _saving = self.inner.config_save.lock();
        let snapshot = {
            let mut cfg = self.inner.config.write();
            f(&mut cfg);
            // The language switches the moment the config changes (the UI is redrawn every frame).
            i18n::set_lang(cfg.language);
            cfg.clone()
        };
        if self.inner.config_read_only.load(Ordering::SeqCst) {
            return;
        }
        if let Err(e) = snapshot.save(&self.paths().config_file) {
            log::warn!("failed to save the config at {}: {e}", self.paths().config_file.display());
        }
    }

    // ---------- Agents (agents:list / pty:spawn) ----------

    /// List of agents for menus (same semantics as `agents:list`). It may block (searches for the
    /// CLIs on disk and queries `gh`): call it outside the UI thread.
    pub fn list_agents(&self) -> Vec<AgentInfo> {
        self.inner.agents.list()
    }

    /// Builds the command to open, in the folder of project `project`, an interactive terminal of agent
    /// `kind` (`claude`, `antigravity`, `shell`, ...).
    /// Error = friendly message ("not installed", unknown project) to be written in the terminal.
    pub fn spawn_spec(&self, project: &str, kind: &str) -> Result<SpawnSpec, String> {
        agents::spawn_spec(self, project, kind)
    }

    /// Like [`Core::spawn_spec`] for an instance (model + role + name): its id goes in
    /// `HOLT_AGENT` and the role in `HOLT_ROLE`. The tab token belongs to the project + id pair.
    pub fn spawn_spec_for(&self, project: &str, ident: &Ident) -> Result<SpawnSpec, String> {
        agents::spawn_spec_for(self, project, ident)
    }

    // ---------- Instances (tabs) and questions between them ----------

    /// The UI starts delivering the `ask-agent` questions by typing in the tabs (instead of running
    /// the agent separately, in headless mode). Only applies with `Config.auto_open_agent_tabs`.
    pub fn enable_tab_delivery(&self) {
        self.inner.tab_delivery.store(true, Ordering::SeqCst);
    }

    pub(crate) fn tab_delivery(&self) -> bool {
        self.inner.tab_delivery.load(Ordering::SeqCst) && self.config().auto_open_agent_tabs
    }

    /// Applies `f` to the instances of project `project` (created on first use). A project that does not
    /// exist (removed in the middle of a question) gets no entry: `f` receives a disposable list.
    pub(crate) fn with_instances<R>(&self, project: &str, f: impl FnOnce(&mut instances::Instances) -> R) -> R {
        let exists = self.project_path(project).is_some();
        let mut all = self.inner.instances.lock();
        match all.get_mut(project) {
            Some(list) => f(list),
            None if exists => f(all.entry(project.to_owned()).or_default()),
            None => f(&mut instances::Instances::default()),
        }
    }

    /// New instance opened by the user in the project (the "+" menu): `claude`, and `claude@main@ana` if
    /// the project already has one. Another project can have its own `claude` without a conflict.
    pub fn register_instance(&self, project: &str, kind: &str) -> Ident {
        self.with_instances(project, |list| list.allocate(kind, None, None))
    }

    /// Reopens an instance of the project with the id it had (tab restoration). `None` if the
    /// id is invalid.
    pub fn restore_instance(&self, project: &str, id: &str) -> Option<Ident> {
        let ident = Ident::parse(id)?;
        self.with_instances(project, |list| list.register(ident.clone()));
        Some(ident)
    }

    /// Resolves an instance (model/role/name) in the project, creating it if it does not exist (or if
    /// `force_new`). Returns `(ident, created)` or the list of ambiguous candidates.
    pub fn resolve_instance(&self, project: &str, want: &Ident, force_new: bool) -> Result<(Ident, bool), Vec<String>> {
        self.with_instances(project, |list| list.resolve_or_create(want, force_new))
    }

    /// The instance's tab was closed: forgets the instance, its token and its activity, notifies whoever
    /// was waiting for its answer and discards the questions it asked (nobody reads the answers any more).
    pub fn close_instance(&self, project: &str, id: &str) {
        if let Some(list) = self.inner.instances.lock().get_mut(project) {
            list.remove(id);
        }
        self.inner.callers.lock().retain(|_, c| !(c.tab && c.project == project && c.id == id));
        let key = instances::scoped(project, id);
        self.inner.exchange.fail_target(&key, &format!("the tab of \"{id}\" was closed before answering"));
        self.inner.exchange.fail_asker(&key, &format!("the tab of \"{id}\" was closed"));
        self.inner.activity.forget(&key);
        self.inner.team_changed.lock().remove(&key);
        // The team of whoever closed goes to their boss.
        let mut touched = Vec::new();
        self.update_project(project, |p| {
            touched = team::remove(&mut p.tab_parents, id);
            p.tab_models.remove(id);
            // Closed for good (the tab's ×, a finished task): reopening it later starts a new conversation.
            p.tab_sessions.remove(id);
        });
        self.mark_team_changed(project, &touched);
    }

    // ---------- Hierarchy (boss and team) ----------

    /// Boss and team of instance `id` of the project (only open instances).
    pub fn team(&self, project: &str, id: &str) -> Team {
        let parents = self.inner.config.read().project(project).map(|p| p.tab_parents.clone()).unwrap_or_default();
        team::team_of(&parents, id, &self.project_instance_ids(project))
    }

    /// Puts instance `id` under `boss` (`None` = answers to the user). Refuses instances that are not
    /// open in the project and cycles. Whoever was affected (it, the old boss and the new one) receives the
    /// notice with the next message.
    pub fn set_boss(&self, project: &str, id: &str, boss: Option<&str>) -> Result<(), String> {
        let open = self.project_instance_ids(project);
        for who in std::iter::once(id).chain(boss) {
            if !open.iter().any(|o| o == who) {
                return Err(crate::t!("\"{who}\" is not open in this project", who));
            }
        }
        // Checks and saves under the same lock: a drag and the bridge at the same time do not form a cycle.
        let mut outcome: Result<Option<Option<String>>, String> = Ok(None);
        self.update_project(project, |p| {
            if let Some(b) = boss.filter(|b| team::would_cycle(&p.tab_parents, id, b)) {
                outcome = Err(crate::t!("\"{b}\" is already part of the team of \"{id}\": it cannot be its boss", b, id));
                return;
            }
            let old = p.tab_parents.get(id).cloned();
            if old.as_deref() == boss {
                return;
            }
            match boss {
                Some(b) => p.tab_parents.insert(id.to_owned(), b.to_owned()),
                None => p.tab_parents.remove(id),
            };
            outcome = Ok(Some(old));
        });
        let Some(old) = outcome? else { return Ok(()) };
        let touched: Vec<String> = std::iter::once(id.to_owned()).chain(old).chain(boss.map(str::to_owned)).collect();
        self.mark_team_changed(project, &touched);
        Ok(())
    }

    fn mark_team_changed(&self, project: &str, ids: &[String]) {
        let mut changed = self.inner.team_changed.lock();
        changed.extend(ids.iter().map(|id| instances::scoped(project, id)));
    }

    /// The instance's pending team notice (once: it disappears when read). The UI attaches it to the
    /// message when it types it: a discarded delivery does not spend the notice.
    pub fn take_team_notice(&self, project: &str, id: &str) -> Option<String> {
        let had = self.inner.team_changed.lock().remove(&instances::scoped(project, id));
        had.then(|| team::notice(&self.team(project, id)))
    }

    /// Renames an open instance of the project (updates instances, callers and pending questions).
    pub fn rename_instance(&self, project: &str, old_id: &str, new_ident: &Ident) -> Result<(), String> {
        self.with_instances(project, |list| list.rename(old_id, new_ident.clone()))?;
        let new_id = new_ident.id();
        for caller in self.inner.callers.lock().values_mut() {
            if caller.project == project && caller.id == old_id {
                caller.id = new_id.clone();
            }
        }
        self.inner.exchange.rename(&instances::scoped(project, old_id), &instances::scoped(project, &new_id));
        self.inner.inbox.rename(&instances::scoped(project, old_id), &instances::scoped(project, &new_id));
        self.inner.activity.forget(&instances::scoped(project, old_id));
        self.update_project(project, |p| {
            team::rename(&mut p.tab_parents, old_id, &new_id);
            models::rename(&mut p.tab_models, old_id, &new_id);
            models::rename(&mut p.tab_sessions, old_id, &new_id);
        });
        let mut changed = self.inner.team_changed.lock();
        if changed.remove(&instances::scoped(project, old_id)) {
            changed.insert(instances::scoped(project, &new_id));
        }
        Ok(())
    }

    /// The instance's process ended (the tab stays open): whoever was waiting for its answer is released.
    pub fn instance_ended(&self, project: &str, id: &str) {
        self.inner.exchange.fail_target(&instances::scoped(project, id), &format!("the process of \"{id}\" ended before answering"));
    }

    /// Question number `request` is still waiting for an answer. A question that expired or whose source
    /// tab closed while it was in the queue must not be typed any more.
    pub fn question_pending(&self, request: u64) -> bool {
        self.inner.exchange.is_pending(request)
    }

    /// The delivery could not be made (tab died, did not start…): releases whoever is waiting.
    pub fn fail_delivery(&self, request: u64, reason: &str) {
        self.inner.exchange.fail(request, reason);
    }

    /// The UI typed the late answer of question `request` in the terminal of whoever asked.
    pub fn answer_typed(&self, request: u64) {
        self.inner.inbox.mark_delivered(request);
    }

    /// The answer of question `request` did not reach whoever asked (`why`: the tab closed, the connection was
    /// abandoned…). It stays in `ask-agent --inbox`; the project's integrated conversation warns.
    pub fn answer_lost(&self, project: &str, request: u64, why: &str) {
        let Some((asker, entry)) = self.inner.inbox.find(request) else { return };
        let asker = instances::unscoped(&asker).1;
        let text = crate::t!("The answer #{request} to {asker} was not typed: {why}; it stays in ask-agent --inbox.", request, asker, why);
        self.feed(FeedEntry::new(FeedKind::Error, Some(&entry.from), Some(asker), text).in_project(project));
    }

    /// Ids of the instances open in all projects, without the project and without repetition (the
    /// `claude` of two projects appears once). It serves whoever only looks at the model (plan usage).
    pub fn instance_ids(&self) -> Vec<String> {
        let all = self.inner.instances.lock();
        let mut projects: Vec<&String> = all.keys().collect();
        projects.sort();
        let mut out = Vec::new();
        for id in projects.into_iter().flat_map(|p| all[p].ids()) {
            if !out.contains(&id) {
                out.push(id);
            }
        }
        out
    }

    /// Ids of the instances open in the project.
    pub fn project_instance_ids(&self, project: &str) -> Vec<String> {
        self.inner.instances.lock().get(project).map(instances::Instances::ids).unwrap_or_default()
    }

    /// Ids that can be used with `ask-agent` in the project: the installed agents and the instances
    /// open in it (those of other projects do not appear).
    pub fn peers(&self, project: &str) -> Vec<String> {
        let mut out = self.inner.agents.peers();
        for id in self.project_instance_ids(project) {
            if !out.contains(&id) && Ident::parse(&id).is_some_and(|i| out.contains(&i.kind)) {
                out.push(id);
            }
        }
        out
    }

    /// Bridge instructions for agent `self_id` (`None` = the generic text of AGENTS.md).
    pub fn bridge_instructions(&self, self_id: Option<&str>) -> String {
        self.inner.agents.bridge_instructions(self_id)
    }

    // ---------- Projects ----------

    /// Projects, in the sidebar order.
    pub fn projects(&self) -> Vec<ProjectCfg> {
        self.inner.config.read().project_list.clone()
    }

    /// The active project (`None` = no project).
    pub fn active_project(&self) -> Option<ProjectCfg> {
        self.inner.config.read().active().cloned()
    }

    /// Id of the active project (empty = none).
    pub fn active_project_id(&self) -> String {
        self.inner.config.read().active_project.clone()
    }

    /// Folder of project `id`.
    pub fn project_path(&self, id: &str) -> Option<PathBuf> {
        self.inner.config.read().project(id).map(|p| p.path.clone())
    }

    /// Creates (and saves) a project in folder `path`; without `name`, the name is the folder's. If the folder already
    /// belongs to a project, returns that one. Does not switch the active project, unless there is none:
    /// then the new one becomes active.
    pub fn create_project(&self, path: &Path, name: Option<&str>) -> ProjectCfg {
        let path = util::absolutize(path);
        let mut out = None;
        self.update_config(|c| {
            let existing = c.project_list.iter().find(|p| util::same_path(&p.path, &path)).cloned();
            out = Some(match existing {
                Some(p) => p,
                None => c.add_project(path, name),
            });
        });
        let project = out.expect("update_config runs the closure");
        if self.active_project().is_none() {
            if let Err(e) = self.set_active_project(&project.id) {
                log::warn!("failed to activate project {}: {e}", project.path.display());
            }
        }
        project
    }

    /// Gives the project another name (the folder does not change). `false` if the project does not exist or the name is empty.
    pub fn rename_project(&self, id: &str, name: &str) -> bool {
        let name = name.trim();
        !name.is_empty() && self.update_project(id, |p| p.name = name.to_owned())
    }

    /// Changes a project's data and saves it (open tabs, active tab, order and proportions of the
    /// windows). The id and the folder do not change. `false` if the project does not exist.
    pub fn update_project(&self, id: &str, f: impl FnOnce(&mut ProjectCfg)) -> bool {
        if self.inner.config.read().project(id).is_none() {
            return false;
        }
        self.update_config(|c| {
            if let Some(p) = c.project_list.iter_mut().find(|p| p.id == id) {
                let (id, path) = (p.id.clone(), p.path.clone());
                f(p);
                p.id = id;
                p.path = path;
            }
        });
        true
    }

    /// Takes the project out of the list: closes its instances and fails the pending questions in it. If
    /// it was the active one, activates the first that is left (or none). `false` if the project does not exist.
    /// The folder is not deleted.
    pub fn remove_project(&self, id: &str) -> bool {
        if self.project_path(id).is_none() {
            return false;
        }
        // First the tokens (no new question comes in) and the project (a question that already passed
        // authentication checks that it exists after being registered; see `bridge::ask_tab`).
        self.inner.callers.lock().retain(|_, c| c.project != id);
        let was_active = self.active_project_id() == id;
        self.update_config(|c| {
            c.remove_project(id);
        });
        let ids = self.inner.instances.lock().remove(id).map(|list| list.ids()).unwrap_or_default();
        for instance in &ids {
            self.close_instance(id, instance);
        }
        // Questions that were left in the project (e.g. asked with the master token).
        for p in self.inner.exchange.snapshot().into_iter().filter(|p| p.project == id) {
            self.inner.exchange.fail(p.id, "the project was removed");
        }
        let prefix = instances::scoped(id, "");
        self.inner.headless_sessions.lock().retain(|k, _| !k.starts_with(&prefix));
        self.inner.team_changed.lock().retain(|k| !k.starts_with(&prefix));
        self.inner.activity.forget_prefix(&prefix);
        self.inner.context_written.lock().remove(id);
        if was_active {
            self.prepare_workspace();
        }
        true
    }

    /// Makes `id` the active project (persisted): its folder becomes the workspace (tree, git,
    /// attachments), and the AGENTS.md and the watcher are made again.
    pub fn set_active_project(&self, id: &str) -> anyhow::Result<()> {
        let Some(path) = self.project_path(id) else { anyhow::bail!(crate::t!("unknown project: {id}", id)) };
        std::fs::create_dir_all(&path)?;
        self.update_config(|c| {
            c.active_project = id.to_owned();
            c.workspace = path;
        });
        self.prepare_workspace();
        Ok(())
    }

    /// Writes AGENTS.md in the folder of a project that is not the active one, in the first tab opened in it
    /// (the active one's is kept by [`Core::prepare_workspace`]).
    fn ensure_project_context(&self, project: &str, path: &Path) {
        if project == self.active_project_id() || !self.inner.context_written.lock().insert(project.to_owned()) {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(path) {
            log::warn!("failed to create the project folder {}: {e}", path.display());
        }
        let instructions = self.bridge_instructions(None);
        if let Err(e) = workspace::ensure_agent_context(path, &self.paths().app_dir, &self.inner.home, &instructions, false) {
            log::warn!("failed to update AGENTS.md at {}: {e}", path.display());
        }
    }

    /// For the CLI that only reads the `AGENTS.md` of the folder where it runs (see `agents::reads_only_agents_md`): creates the
    /// file in `dir`, with only the app block, if the folder has none, and warns the user in the Conversation. An `AGENTS.md`
    /// that already exists stays as it is here (its block is kept by `prepare_workspace`/`ensure_project_context`; in a task
    /// worktree the file is the versioned one). Never in the home folder.
    fn ensure_agents_file(&self, project: &str, dir: &Path) {
        if dir.join("AGENTS.md").exists() {
            return;
        }
        let instructions = self.bridge_instructions(None);
        match workspace::ensure_agent_context(dir, &self.paths().app_dir, &self.inner.home, &instructions, true) {
            Ok(workspace::AgentContext::Created) => {
                log::info!("AGENTS.md created at {} (the CLI only reads instructions from there)", dir.display());
                let text = crate::t!(
                    "Holt created an AGENTS.md with only its own instructions in this folder, because this CLI only reads \
                     instructions from there. It shows up as a new file in git: commit it or delete it as \
                     you like."
                );
                self.feed(FeedEntry::system(text).in_project(project));
            }
            Ok(_) => {}
            Err(e) => log::warn!("failed to create AGENTS.md at {}: {e}", dir.display()),
        }
    }

    // ---------- Workspace (the active project's folder) ----------

    /// Folder of the active project (empty = no project).
    pub fn workspace(&self) -> PathBuf {
        self.inner.config.read().workspace.clone()
    }

    /// Shortcut: creates (if needed) the project of folder `dir` and makes it active.
    pub fn set_workspace(&self, dir: PathBuf) -> anyhow::Result<()> {
        let dir = util::absolutize(&dir);
        std::fs::create_dir_all(&dir)?;
        let project = self.create_project(&dir, None);
        self.set_active_project(&project.id)
    }

    /// Creates the folder, updates AGENTS.md and (re)starts the watcher. Equivalent to `prepareWorkspace`.
    /// Without an active project, it only releases the watcher.
    fn prepare_workspace(&self) {
        let ws = self.workspace();
        let mut guard = self.inner.watcher.lock();
        *guard = None; // releases the previous watcher before creating the new one
        if ws.as_os_str().is_empty() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&ws) {
            log::warn!("failed to create the workspace {}: {e}", ws.display());
        }
        self.refresh_agent_context();
        if self.inner.watch_workspace && ws.is_dir() {
            match workspace::start_watcher(&ws, Arc::downgrade(&self.inner)) {
                Ok(w) => *guard = Some(w),
                Err(e) => log::warn!("failed to start the workspace watcher: {e}"),
            }
        }
    }

    /// Rewrites the Holt block in the AGENTS.md of the active project (respecting the rules about
    /// where to create it). The other projects are rewritten in the next tab opened in them.
    fn refresh_agent_context(&self) {
        self.inner.context_written.lock().clear();
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return;
        }
        let instructions = self.bridge_instructions(None);
        if let Err(e) = workspace::ensure_agent_context(&ws, &self.paths().app_dir, &self.inner.home, &instructions, false) {
            log::warn!("failed to update AGENTS.md: {e}");
        }
    }

    /// Lists a folder relative to the workspace (`""` = root), already filtered and sorted
    /// (folders first, case-insensitive name). Same filter as `workspace:readDir`.
    /// Without an active project, nothing.
    pub fn read_dir(&self, rel_dir: &str) -> Vec<DirEntry> {
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return Vec::new();
        }
        workspace::read_dir(&ws, rel_dir)
    }

    /// Reads a text file of the workspace (up to 1 MB) for the Documents mode view; without an
    /// active project, an error. See [`workspace::read_text`].
    pub fn read_text(&self, rel_path: &str) -> Result<workspace::TextFile, String> {
        workspace::read_text(&self.workspace(), rel_path)
    }

    /// Writes a file edited in the Documents mode, in the folder of project `project` (the document may belong to a
    /// project that is not the active one). See [`workspace::write_text`]. BLOCKING: call it on a thread.
    pub fn write_text(&self, project: &str, rel_path: &str, text_lf: &str, eol: Eol, bom: bool) -> Result<(), String> {
        let ws = self.project_path(project).ok_or_else(|| crate::t!("the project no longer exists").to_owned())?;
        workspace::write_text(&ws, rel_path, text_lf, eol, bom)
    }

    /// Opens a workspace file with the system's default app. Accepts paths relative to the
    /// workspace or absolute inside the workspace / the `data` folder (attachments).
    pub fn open_path(&self, rel_path: &str) -> anyhow::Result<()> {
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            anyhow::bail!(crate::t!("no project open"));
        }
        let full = match workspace::resolve_workspace_path(rel_path, &ws) {
            Ok(p) => p,
            Err(e) => {
                let abs = util::normalize_lexical(&ws.join(rel_path));
                if !util::path_within(&abs, &util::absolutize(&self.paths().data_dir)) {
                    anyhow::bail!(e);
                }
                abs
            }
        };
        open::that_detached(&full)
            .map_err(|e| anyhow::anyhow!(crate::t!("failed to open {path}: {error}", path = full.display(), error = e)))
    }

    // ---------- Attachments ----------

    /// Copies files to `data/attachments` (ignores folders). Returns references to cite (`@ref`).
    pub fn attach_files(&self, files: &[PathBuf]) -> Vec<String> {
        attachments::attach_files(&self.paths().data_dir, &self.workspace(), self.session_id(), files)
    }

    /// Saves bytes (e.g. a pasted image) as an attachment. `name` may be empty.
    pub fn attach_bytes(&self, name: &str, bytes: &[u8]) -> anyhow::Result<String> {
        attachments::attach_bytes(&self.paths().data_dir, &self.workspace(), self.session_id(), name, bytes)
    }

    /// Saves an RGBA image (e.g. from the clipboard) as a PNG attachment.
    pub fn attach_rgba_png(&self, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<String> {
        attachments::attach_rgba_png(&self.paths().data_dir, &self.workspace(), self.session_id(), width, height, rgba)
    }

    // ---------- Transcription (audio:*) ----------

    /// Label of the best available engine or "unavailable". On the first call on
    /// Windows it detects the GPUs (≈1 s); prefer calling it outside the UI thread.
    pub fn transcribe_engine(&self) -> String {
        whisper::transcribe_engine(self)
    }

    /// Transcribes mono 16 kHz PCM (f32 -1..1). Writes a temporary WAV, runs whisper.cpp
    /// with engine fallback and deletes the WAV. BLOCKING: call it on a thread.
    pub fn transcribe_pcm16k(&self, samples: &[f32]) -> anyhow::Result<Transcription> {
        whisper::transcribe_pcm16k(self, samples)
    }

    /// Ensures the whisper model/binaries (background download with progress feed).
    pub fn ensure_whisper_async(&self) {
        whisper::ensure_whisper_async(self);
    }

    /// What to do with the voice dictation when the app opens (see [`whisper::Startup`]). Whoever already has Whisper
    /// downloaded and was never asked starts to count as "yes" in the configuration.
    pub fn dictation_startup(&self) -> whisper::Startup {
        let installed = self.dictation_installed();
        let (startup, choice) = whisper::startup(self.config().dictation, installed);
        self.set_dictation_choice(choice);
        startup
    }

    /// Saves the dictation choice (without rewriting the file if it did not change).
    pub fn set_dictation_choice(&self, choice: DictationChoice) {
        if self.config().dictation != choice {
            self.update_config(|c| c.dictation = choice);
        }
    }

    /// The whisper model and an engine are on disk: the dictation can be used.
    pub fn dictation_installed(&self) -> bool {
        whisper::is_installed(&self.paths().whisper_dir)
    }

    /// A dictation download is in progress.
    pub fn dictation_downloading(&self) -> bool {
        whisper::is_downloading(self)
    }

    /// Deletes from disk what the dictation downloaded (model, CUDA engine, half-done packages) and sets the choice to
    /// "no". Refuses while it is downloading. Returns the bytes freed.
    pub fn remove_dictation(&self) -> Result<u64, String> {
        let freed = whisper::remove_downloaded(self)?;
        self.set_dictation_choice(DictationChoice::No);
        self.emit(CoreEvent::DictationChanged);
        Ok(freed)
    }

    // ---------- Bridge ----------

    pub fn bridge_port(&self) -> u16 {
        *self.inner.state.bridge_port.lock()
    }

    /// Master token of the bridge. The agents receive their own tokens (see [`Core::tab_token`]).
    pub fn bridge_token(&self) -> &str {
        &self.inner.state.bridge_token
    }

    /// Token of the tab of instance `id` of project `project` (the same while the instance exists;
    /// the `claude` of each project has its own).
    pub(crate) fn tab_token(&self, project: &str, id: &str) -> String {
        let mut callers = self.inner.callers.lock();
        if let Some((token, _)) = callers.iter().find(|(_, c)| c.tab && c.project == project && c.id == id) {
            return token.clone();
        }
        let token = random_token();
        callers.insert(token.clone(), Caller { id: id.to_owned(), project: project.to_owned(), tab: true });
        token
    }

    /// Token of a headless run of `id` in project `project`, valid while the guard exists.
    pub(crate) fn headless_token(&self, project: &str, id: &str) -> TokenGuard {
        let token = random_token();
        self.inner.callers.lock().insert(token.clone(), Caller { id: id.to_owned(), project: project.to_owned(), tab: false });
        TokenGuard { inner: self.inner.clone(), token }
    }

    /// Who owns the token (`None` = invalid token).
    pub(crate) fn authenticate(&self, token: &str) -> Option<Auth> {
        if same_secret(token, &self.inner.state.bridge_token) {
            return Some(Auth::Master);
        }
        self.inner.callers.lock().get(token).cloned().map(Auth::Agent)
    }

    /// Shuts down the headless child processes (called when the window closes), the bridge and the watcher.
    pub fn shutdown(&self) {
        self.inner.state.shutdown.store(true, Ordering::SeqCst);
        activation::forget(&self.paths().data_dir);
        self.inner.processes.kill_all();
        if let Some(server) = self.inner.bridge_server.lock().take() {
            server.unblock();
        }
        *self.inner.watcher.lock() = None;
        // The lock goes now, not when the last copy of `Core` disappears: background threads (bridge, quota) still
        // hold copies for a moment, and a new opening in the same data folder would refuse in that interval.
        self.inner.data_lock.lock().take();
    }
}

/// Converts an absolute path into the reference used to cite files to the agents
/// (relative to the workspace when inside it, with `/`). Equivalent to `refPath`.
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    util::ref_path(workspace, abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_data_dir_is_locked_by_one_instance() {
        let tmp = tempfile::tempdir().expect("tmp");
        let first = lock_data_dir(tmp.path()).expect("first");
        let err = lock_data_dir(tmp.path()).expect_err("the second one refuses").to_string();
        assert!(err.starts_with("Another Holt window is already using the data folder"), "{err}");
        drop(first);
        lock_data_dir(tmp.path()).expect("free again");
    }

    /// Seen in the Fedora CI: after `shutdown`, a copy of `Core` still alive on a thread held the lock, and
    /// reopening in the same data folder refused.
    #[test]
    fn shutdown_frees_the_data_dir_even_with_a_copy_still_alive() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let paths = paths::AppPaths {
            app_dir: root.join("app"),
            bin_dir: root.join("bin"),
            data_dir: root.join("data"),
            default_workspace: root.join("ws"),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = StartOptions {
            search_path: Some(root.join("vazio").into_os_string()),
            home_dir: Some(root.join("home")),
            watch_workspace: false,
            config: Some(Config::with_project(root.join("ws"))),
            system_dirs: false,
        };
        let (core, _rx) = Core::start_with(paths, opts).expect("start");
        let still_alive = core.clone();
        core.shutdown();
        lock_data_dir(&root.join("data")).expect("the folder is free right after the shutdown");
        drop(still_alive);
    }
}

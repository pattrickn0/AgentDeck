//! AgentDeck — núcleo (backend) sem dependências de interface gráfica.
//!
//! Este crate é o equivalente Rust de `main.js` + `ollamaAgent.js` da versão Electron.
//! A interface (crate `agentdeck`) conversa com ele exclusivamente através de [`Core`]
//! e dos eventos [`CoreEvent`] — o mesmo papel que o IPC/preload tinha no Electron.
//!
//! CONTRATO ESTÁVEL: os tipos e assinaturas públicas deste arquivo e de `events.rs`,
//! `agents.rs` (tipos `AgentInfo`/`SpawnSpec`), `config.rs` (`Config`) e
//! `workspace.rs` (`DirEntry`) são consumidos pela UI. Não mude sem alinhar com o lead.

pub mod activity;
pub mod agents;
pub mod attachments;
pub mod bridge;
pub mod config;
pub mod context;
pub mod delivery;
pub mod download;
pub mod env;
pub mod events;
pub mod exchange;
pub mod git;
pub mod github;
pub mod headless;
pub mod inbox;
pub mod instances;
pub mod learnings;
pub mod models;
pub mod ollama;
pub mod paths;
pub(crate) mod process;
pub mod sessions;
pub mod situation;
pub mod tasks;
pub mod team;
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
pub use config::{Config, Organization, ProjectCfg};
pub use env::{env_var, env_var_os};
pub use events::{CoreEvent, Delivery, DownloadProgress, DownloadState, FeedEntry, FeedKind};
pub use exchange::PendingInfo;
pub use headless::HeadlessOptions;
pub use instances::Ident;
pub use tasks::{Task, TaskError, TaskEvent, TaskSpec, TaskStatus};
pub use team::Team;
pub use usage::{AgentUsage, UsageGroup, UsageWindow};
pub use workspace::{DirEntry, TextFile};

/// Função chamada sempre que um [`CoreEvent`] é emitido, para acordar o loop da UI
/// (ex.: `EventLoopProxy::send_event`). Pode ser chamada de qualquer thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Resultado de uma transcrição.
#[derive(Debug, Clone)]
pub struct Transcription {
    pub text: String,
    /// Rótulo do motor usado (ex.: "GPU (Vulkan)").
    pub engine: String,
}

/// Opções de inicialização para testes e para embutir o núcleo. O padrão é o
/// comportamento normal do app.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// PATH usado para localizar os CLIs dos agentes (`None` = PATH do processo).
    pub search_path: Option<OsString>,
    /// Pasta pessoal usada na resolução de agentes, nos manifests do Ollama e na regra
    /// "nunca escrever AGENTS.md na pasta do usuário" (`None` = a do sistema).
    pub home_dir: Option<PathBuf>,
    /// Descobrir o Ollama (executável, `/api/tags` e manifests em disco).
    pub ollama_discovery: bool,
    /// Observar o workspace e emitir [`CoreEvent::WorkspaceChanged`].
    pub watch_workspace: bool,
    /// Configuração a usar em vez de ler `paths.config_file` (não é gravada até `update_config`).
    pub config: Option<Config>,
    /// Procurar CLIs também nas pastas do sistema (`APPDATA`, `LOCALAPPDATA`, `ProgramFiles`).
    /// Os testes desligam, para não depender do que está instalado na máquina.
    pub system_dirs: bool,
}

impl Default for StartOptions {
    fn default() -> Self {
        Self { search_path: None, home_dir: None, ollama_discovery: true, watch_workspace: true, config: None, system_dirs: true }
    }
}

/// Handle principal do backend. Barato de clonar; seguro entre threads.
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
    pub ollama: ollama::OllamaAgent,
    pub providers: context::Providers,
    pub processes: process::ProcessRegistry,
    /// Chave do par (com o escopo do projeto) → id da sessão/conversa headless.
    pub headless_sessions: Mutex<HashMap<String, String>>,
    /// Instâncias de agentes (uma por aba) de cada projeto (id do projeto → instâncias).
    pub instances: Mutex<HashMap<String, instances::Instances>>,
    /// Perguntas aguardando resposta entre abas (chaves com escopo: `instances::scoped`).
    pub exchange: exchange::Exchange,
    /// Respostas recebidas por cada instância que perguntou (`ask-agent --inbox`).
    pub inbox: inbox::Inbox,
    /// Última atividade de cada aba (chave com escopo); o prazo de uma pergunta conta a partir dela.
    pub activity: activity::Activity,
    /// Projetos fora do ativo cujo AGENTS.md já foi escrito nesta sessão (na primeira aba deles).
    pub context_written: Mutex<HashSet<String>>,
    /// A interface sabe entregar mensagens nas abas (senão a ponte responde em modo headless).
    pub tab_delivery: std::sync::atomic::AtomicBool,
    /// Token de cada processo de agente → quem ele é (a ponte não confia no `from` do corpo).
    pub callers: Mutex<HashMap<String, Caller>>,
    /// Instâncias (chaves com escopo) cuja posição na equipe mudou desde a última mensagem que
    /// receberam: a próxima leva o aviso (ver `team::notice`).
    pub team_changed: Mutex<HashSet<String>>,
    /// Serializa as gravações da configuração (ver `Core::update_config`).
    pub config_save: Mutex<()>,
    /// As alterações da configuração ficam só na memória (capturas de tela e estados de depuração).
    pub config_read_only: std::sync::atomic::AtomicBool,
    pub bridge_queues: bridge::PairQueues,
    /// Modelos conhecidos de cada agente (menu de novo agente), em cache.
    pub model_lists: models::ModelLists,
    pub bridge_server: Mutex<Option<Arc<tiny_http::Server>>>,
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub whisper: whisper::WhisperState,
    pub git: git::GitState,
    pub github: github::GithubState,
    /// Uso dos planos dos agentes (barra de título).
    pub usage: usage::UsageStore,
    /// Tarefas (mesa de tarefas) de cada projeto, em `dados/tarefas/<projeto>.json`.
    pub tasks: tasks::TaskStore,
    /// Trava da pasta de dados (uma instância do app por `dados/`); solta ao encerrar.
    _data_lock: Vec<std::fs::File>,
}

/// Quem chamou a ponte, pelo token que recebeu no ambiente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// Id da instância (`claude`, `claude@dev`…), sem o projeto.
    pub id: String,
    /// Projeto da instância: a ponte só enxerga as instâncias dele.
    pub project: String,
    /// É o processo da aba (interativo). `false` = execução headless (ponte sem abas).
    pub tab: bool,
}

/// Resultado da autenticação de uma requisição à ponte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Auth {
    /// Token mestre (testes e ferramentas de depuração): o `from` do corpo é aceito, validado.
    Master,
    /// Token de um processo de agente: o `from` é o dele, sempre.
    Agent(Caller),
}

/// Token de um processo headless; esquecido quando o processo termina.
pub(crate) struct TokenGuard {
    inner: Arc<CoreInner>,
    pub token: String,
}

impl Drop for TokenGuard {
    fn drop(&mut self) {
        self.inner.callers.lock().remove(&self.token);
    }
}

/// Comparação em tempo constante (o token não vaza pelo tempo de resposta).
fn same_secret(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Travas da pasta de dados: `.agentdeck.lock` e, durante a migração do nome, também o arquivo da
/// versão antiga (CommandDeck), para o app novo não abrir sobre a mesma pasta com o antigo ainda
/// aberto (e vice-versa). Ver `memory/aprendizados/nomes-antigos-aceitos-na-migracao.md`.
const LOCK_FILES: [&str; 2] = [".agentdeck.lock", ".commanddeck.lock"];

/// Trava `dados/.agentdeck.lock` e `dados/.commanddeck.lock`. Erro se outra instância do app, nova ou
/// antiga, já usa a pasta.
fn lock_data_dir(data_dir: &Path) -> anyhow::Result<Vec<std::fs::File>> {
    std::fs::create_dir_all(data_dir)?;
    let mut locks = Vec::with_capacity(LOCK_FILES.len());
    for name in LOCK_FILES {
        let path = data_dir.join(name);
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
        match file.try_lock() {
            Ok(()) => {}
            Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
                "Outra janela do AgentDeck já está usando a pasta de dados {}.\n\nFeche-a, ou defina AGENTDECK_HOME com outra pasta para abrir uma instância separada.",
                data_dir.display()
            ),
            Err(std::fs::TryLockError::Error(e)) => log::warn!("não consegui travar {}: {e}", path.display()),
        }
        locks.push(file);
    }
    Ok(locks)
}

pub(crate) mod state {
    //! Estado mutável simples compartilhado.
    #[derive(Default)]
    pub struct SharedState {
        pub bridge_port: parking_lot::Mutex<u16>,
        pub bridge_token: String,
        pub session_id: String,
        /// Sinal de encerramento (interrompe execuções do Ollama na ponte).
        pub shutdown: std::sync::atomic::AtomicBool,
    }
}

/// Token aleatório de 128 bits em hexadecimal.
fn random_token() -> String {
    let mut buf = [0u8; 16];
    if getrandom::fill(&mut buf).is_err() {
        // Fallback improvável: UUID v4 (122 bits aleatórios).
        buf = *uuid::Uuid::new_v4().as_bytes();
    }
    buf.iter().map(|b| format!("{b:02x}")).collect()
}

impl Core {
    /// Inicializa o backend: carrega config, prepara workspace (AGENTS.md + watcher),
    /// registra a sessão/limpeza de retenção e sobe a ponte HTTP em 127.0.0.1:0.
    /// Retorna o handle e o receptor de eventos para a UI.
    pub fn start(paths: paths::AppPaths) -> anyhow::Result<(Core, Receiver<CoreEvent>)> {
        Self::start_with(paths, StartOptions::default())
    }

    /// Como [`Core::start`], com opções (testes / uso embutido).
    pub fn start_with(paths: paths::AppPaths, opts: StartOptions) -> anyhow::Result<(Core, Receiver<CoreEvent>)> {
        let data_lock = lock_data_dir(&paths.data_dir)?;
        let (tx, rx) = crossbeam_channel::unbounded();
        let home = opts.home_dir.clone().unwrap_or_else(util::home_dir);
        let mut config = match opts.config {
            Some(c) => c,
            None => Config::load_from(&config::config_candidates(&paths), &paths, &home),
        };
        config.fix_workspace(&paths, &home);
        let providers = context::Providers::default();
        let inner = CoreInner {
            ollama: ollama::OllamaAgent::with_providers(config.ollama_host.clone(), providers.clone()),
            config: RwLock::new(config),
            events_tx: tx,
            waker: RwLock::new(None),
            state: state::SharedState { bridge_token: random_token(), session_id: sessions::new_session_id(), ..Default::default() },
            agents: agents::Registry::new(agents::Discovery {
                search_path: opts.search_path,
                home: home.clone(),
                ollama: opts.ollama_discovery,
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
            _data_lock: data_lock,
            paths,
        };
        let core = Core { inner: Arc::new(inner) };

        core.prepare_workspace();
        sessions::register_and_cleanup(&core.paths().data_dir, &core.inner.state.session_id);
        let (server, port) = bridge::start(&core)?;
        *core.inner.bridge_server.lock() = Some(server);
        *core.inner.state.bridge_port.lock() = port;

        usage::start_poller(&core);

        // Atualiza os modelos Ollama em segundo plano (a lista inicial vem do disco).
        if opts.ollama_discovery {
            let weak = Arc::downgrade(&core.inner);
            let _ = std::thread::Builder::new().name("cd-ollama-refresh".into()).spawn(move || {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let Some(inner) = weak.upgrade() else { return };
                let core = Core { inner };
                if core.inner.agents.refresh_ollama_models(&core.config().ollama_host) {
                    core.refresh_agent_context();
                    core.emit(CoreEvent::AgentsChanged);
                }
            });
        }
        Ok((core, rx))
    }

    /// Define a função que acorda a UI a cada evento emitido.
    pub fn set_waker(&self, waker: Waker) {
        *self.inner.waker.write() = Some(waker);
    }

    /// A aba `id` do projeto `project` está trabalhando agora (a interface chama enquanto ela
    /// produz saída ou está num turno).
    pub fn touch_activity(&self, project: &str, id: &str, at: std::time::Instant) {
        self.inner.activity.touch(&instances::scoped(project, id), at);
    }

    /// Emite um evento para a UI (e acorda o loop).
    pub fn emit(&self, ev: CoreEvent) {
        let _ = self.inner.events_tx.send(ev);
        if let Some(w) = self.inner.waker.read().as_ref() {
            w();
        }
    }

    /// Atalho para emitir uma entrada na "Conversa integrada".
    pub fn feed(&self, entry: FeedEntry) {
        self.emit(CoreEvent::Feed(entry));
    }

    pub fn paths(&self) -> &paths::AppPaths {
        &self.inner.paths
    }

    /// Id desta sessão do app (base36 do instante de início).
    pub fn session_id(&self) -> &str {
        &self.inner.state.session_id
    }

    /// Registra um provedor de contexto (ponto de extensão para RAG; nenhum por padrão).
    pub fn register_context_provider(&self, provider: Arc<dyn context::ContextProvider>) {
        self.inner.providers.write().push(provider);
    }

    /// Motor do agente Ollama compartilhado (conversas da ponte).
    pub fn ollama_agent(&self) -> &ollama::OllamaAgent {
        &self.inner.ollama
    }

    // ---------- Configuração (config:get / config:save) ----------

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    /// Aplica uma alteração e persiste em disco.
    /// Daqui em diante a configuração muda só na memória, sem gravar o arquivo (depuração).
    pub fn disable_config_writes(&self) {
        self.inner.config_read_only.store(true, Ordering::SeqCst);
    }

    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        // Uma gravação por vez e na ordem das alterações (a ponte e a interface gravam de threads
        // diferentes): sem isso uma cópia velha podia ficar por cima da nova.
        let _saving = self.inner.config_save.lock();
        let snapshot = {
            let mut cfg = self.inner.config.write();
            f(&mut cfg);
            cfg.clone()
        };
        if self.inner.config_read_only.load(Ordering::SeqCst) {
            self.inner.ollama.set_host(snapshot.ollama_host.clone());
            return;
        }
        self.inner.ollama.set_host(snapshot.ollama_host.clone());
        if let Err(e) = snapshot.save(&self.paths().config_file) {
            log::warn!("falha ao salvar a configuração em {}: {e}", self.paths().config_file.display());
        }
    }

    // ---------- Agentes (agents:list / pty:spawn) ----------

    /// Lista de agentes para menus (mesma semântica de `agents:list`, incluindo
    /// expansão `ollama:<modelo>`). Pode bloquear até ~1,5 s (consulta /api/tags):
    /// chame fora da thread da UI.
    pub fn list_agents(&self) -> Vec<AgentInfo> {
        self.inner.agents.refresh_ollama_models(&self.config().ollama_host);
        self.inner.agents.list()
    }

    /// Monta o comando para abrir, na pasta do projeto `project`, um terminal interativo do agente
    /// `kind` (`claude`, `antigravity`, `ollama`, `ollama:<modelo>`, `shell`, ...).
    /// Erro = mensagem amigável ("não instalado", projeto desconhecido) a ser escrita no terminal.
    pub fn spawn_spec(&self, project: &str, kind: &str) -> Result<SpawnSpec, String> {
        agents::spawn_spec(self, project, kind)
    }

    /// Como [`Core::spawn_spec`] para uma instância (modelo + cargo + nome): o id dela vai em
    /// `AGENTDECK_AGENT` e o cargo em `AGENTDECK_ROLE`. O token da aba é do par projeto + id.
    pub fn spawn_spec_for(&self, project: &str, ident: &Ident) -> Result<SpawnSpec, String> {
        agents::spawn_spec_for(self, project, ident)
    }

    // ---------- Instâncias (abas) e perguntas entre elas ----------

    /// A interface passa a entregar as perguntas do `ask-agent` digitando nas abas (em vez de rodar
    /// o agente à parte, em modo headless). Só vale com `Config.auto_open_agent_tabs`.
    pub fn enable_tab_delivery(&self) {
        self.inner.tab_delivery.store(true, Ordering::SeqCst);
    }

    pub(crate) fn tab_delivery(&self) -> bool {
        self.inner.tab_delivery.load(Ordering::SeqCst) && self.config().auto_open_agent_tabs
    }

    /// Aplica `f` às instâncias do projeto `project` (criadas na primeira vez). Um projeto que não
    /// existe (removido no meio de uma pergunta) não ganha entrada: `f` recebe uma lista descartável.
    pub(crate) fn with_instances<R>(&self, project: &str, f: impl FnOnce(&mut instances::Instances) -> R) -> R {
        let exists = self.project_path(project).is_some();
        let mut all = self.inner.instances.lock();
        match all.get_mut(project) {
            Some(list) => f(list),
            None if exists => f(all.entry(project.to_owned()).or_default()),
            None => f(&mut instances::Instances::default()),
        }
    }

    /// Nova instância aberta pelo usuário no projeto (menu "+"): `claude`, e `claude@main@ana` se
    /// o projeto já tem uma. Outro projeto pode ter o seu `claude` sem conflito.
    pub fn register_instance(&self, project: &str, kind: &str) -> Ident {
        self.with_instances(project, |list| list.allocate(kind, None, None))
    }

    /// Reabre uma instância do projeto com o id que ela tinha (restauração das abas). `None` se o
    /// id é inválido.
    pub fn restore_instance(&self, project: &str, id: &str) -> Option<Ident> {
        let ident = Ident::parse(id)?;
        self.with_instances(project, |list| list.register(ident.clone()));
        Some(ident)
    }

    /// Resolve uma instância (modelo/cargo/nome) no projeto, criando-a se não existir (ou se
    /// `force_new`). Retorna `(ident, created)` ou a lista de candidatos ambíguos.
    pub fn resolve_instance(&self, project: &str, want: &Ident, force_new: bool) -> Result<(Ident, bool), Vec<String>> {
        self.with_instances(project, |list| list.resolve_or_create(want, force_new))
    }

    /// A aba da instância fechou: esquece a instância, o token e a atividade dela, avisa quem
    /// esperava resposta dela e descarta as perguntas que ela fez (ninguém mais lê as respostas).
    pub fn close_instance(&self, project: &str, id: &str) {
        if let Some(list) = self.inner.instances.lock().get_mut(project) {
            list.remove(id);
        }
        self.inner.callers.lock().retain(|_, c| !(c.tab && c.project == project && c.id == id));
        let key = instances::scoped(project, id);
        self.inner.exchange.fail_target(&key, &format!("a aba de \"{id}\" foi fechada antes de responder"));
        self.inner.exchange.fail_asker(&key, &format!("a aba de \"{id}\" foi fechada"));
        self.inner.activity.forget(&key);
        self.inner.team_changed.lock().remove(&key);
        // A equipe de quem fechou passa ao chefe dele.
        let mut touched = Vec::new();
        self.update_project(project, |p| {
            touched = team::remove(&mut p.tab_parents, id);
            p.tab_models.remove(id);
        });
        self.mark_team_changed(project, &touched);
    }

    // ---------- Hierarquia (chefe e equipe) ----------

    /// Chefe e equipe da instância `id` do projeto (só instâncias abertas).
    pub fn team(&self, project: &str, id: &str) -> Team {
        let parents = self.inner.config.read().project(project).map(|p| p.tab_parents.clone()).unwrap_or_default();
        team::team_of(&parents, id, &self.project_instance_ids(project))
    }

    /// Põe a instância `id` sob `boss` (`None` = responde ao usuário). Recusa instâncias que não
    /// estão abertas no projeto e ciclos. Quem foi afetado (ela, o chefe antigo e o novo) recebe o
    /// aviso na próxima mensagem.
    pub fn set_boss(&self, project: &str, id: &str, boss: Option<&str>) -> Result<(), String> {
        let open = self.project_instance_ids(project);
        for who in std::iter::once(id).chain(boss) {
            if !open.iter().any(|o| o == who) {
                return Err(format!("\"{who}\" não está aberto neste projeto"));
            }
        }
        // Confere e grava sob a mesma trava: um arrasto e a ponte ao mesmo tempo não formam ciclo.
        let mut outcome: Result<Option<Option<String>>, String> = Ok(None);
        self.update_project(project, |p| {
            if let Some(b) = boss.filter(|b| team::would_cycle(&p.tab_parents, id, b)) {
                outcome = Err(format!("\"{b}\" já faz parte da equipe de \"{id}\": não pode ser o chefe dele"));
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

    /// O aviso de equipe pendente da instância (uma vez: some ao ser lido). A interface anexa à
    /// mensagem quando a digita: uma entrega descartada não gasta o aviso.
    pub fn take_team_notice(&self, project: &str, id: &str) -> Option<String> {
        let had = self.inner.team_changed.lock().remove(&instances::scoped(project, id));
        had.then(|| team::notice(&self.team(project, id)))
    }

    /// Renomeia uma instância aberta do projeto (atualiza instâncias, callers e perguntas pendentes).
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
        });
        let mut changed = self.inner.team_changed.lock();
        if changed.remove(&instances::scoped(project, old_id)) {
            changed.insert(instances::scoped(project, &new_id));
        }
        Ok(())
    }

    /// O processo da instância terminou (a aba continua aberta): quem esperava a resposta dela é liberado.
    pub fn instance_ended(&self, project: &str, id: &str) {
        self.inner.exchange.fail_target(&instances::scoped(project, id), &format!("o processo de \"{id}\" encerrou antes de responder"));
    }

    /// A pergunta número `request` ainda espera resposta. Uma pergunta que expirou ou cuja aba de
    /// origem fechou enquanto estava na fila não deve mais ser digitada.
    pub fn question_pending(&self, request: u64) -> bool {
        self.inner.exchange.is_pending(request)
    }

    /// A entrega não pôde ser feita (aba morreu, não subiu…): libera quem espera.
    pub fn fail_delivery(&self, request: u64, reason: &str) {
        self.inner.exchange.fail(request, reason);
    }

    /// A interface digitou no terminal de quem perguntou a resposta atrasada da pergunta `request`.
    pub fn answer_typed(&self, request: u64) {
        self.inner.inbox.mark_delivered(request);
    }

    /// A resposta da pergunta `request` não chegou a quem perguntou (`why`: a aba fechou, a conexão foi
    /// abandonada…). Ela continua em `ask-agent --inbox`; a Conversa integrada do projeto avisa.
    pub fn answer_lost(&self, project: &str, request: u64, why: &str) {
        let Some((asker, entry)) = self.inner.inbox.find(request) else { return };
        let asker = instances::unscoped(&asker).1;
        let text = format!("A resposta #{request} para {asker} não foi digitada: {why}; ela fica em ask-agent --inbox.");
        self.feed(FeedEntry::new(FeedKind::Error, Some(&entry.from), Some(asker), text).in_project(project));
    }

    /// Ids das instâncias abertas em todos os projetos, sem o projeto e sem repetição (o
    /// `claude` de dois projetos aparece uma vez). Serve a quem só olha o modelo (uso dos planos).
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

    /// Ids das instâncias abertas no projeto.
    pub fn project_instance_ids(&self, project: &str) -> Vec<String> {
        self.inner.instances.lock().get(project).map(instances::Instances::ids).unwrap_or_default()
    }

    /// Ids que podem ser usados com `ask-agent` no projeto: os agentes instalados e as instâncias
    /// abertas nele (as de outros projetos não aparecem).
    pub fn peers(&self, project: &str) -> Vec<String> {
        let mut out = self.inner.agents.peers();
        for id in self.project_instance_ids(project) {
            if !out.contains(&id) && Ident::parse(&id).is_some_and(|i| out.contains(&i.kind)) {
                out.push(id);
            }
        }
        out
    }

    /// Instruções da ponte para o agente `self_id` (`None` = texto genérico do AGENTS.md).
    pub fn bridge_instructions(&self, self_id: Option<&str>) -> String {
        self.inner.agents.bridge_instructions(self_id)
    }

    // ---------- Projetos ----------

    /// Projetos, na ordem da barra lateral.
    pub fn projects(&self) -> Vec<ProjectCfg> {
        self.inner.config.read().project_list.clone()
    }

    /// O projeto ativo (`None` = nenhum projeto).
    pub fn active_project(&self) -> Option<ProjectCfg> {
        self.inner.config.read().active().cloned()
    }

    /// Id do projeto ativo (vazio = nenhum).
    pub fn active_project_id(&self) -> String {
        self.inner.config.read().active_project.clone()
    }

    /// Pasta do projeto `id`.
    pub fn project_path(&self, id: &str) -> Option<PathBuf> {
        self.inner.config.read().project(id).map(|p| p.path.clone())
    }

    /// Cria (e grava) um projeto na pasta `path`; sem `name`, o nome é o da pasta. Se a pasta já
    /// é de um projeto, devolve esse. Não troca o projeto ativo, a não ser que não haja nenhum:
    /// aí o novo fica ativo.
    pub fn create_project(&self, path: &Path, name: Option<&str>) -> ProjectCfg {
        let path = util::absolutize(path);
        let mut out = None;
        self.update_config(|c| {
            let project = match c.project_list.iter().find(|p| util::same_path(&p.path, &path)) {
                Some(p) => p.clone(),
                None => {
                    let p = ProjectCfg::new(path, name, &c.project_list);
                    c.project_list.push(p.clone());
                    p
                }
            };
            out = Some(project);
        });
        let project = out.expect("update_config roda o fechamento");
        if self.active_project().is_none() {
            if let Err(e) = self.set_active_project(&project.id) {
                log::warn!("falha ao ativar o projeto {}: {e}", project.path.display());
            }
        }
        project
    }

    /// Dá outro nome ao projeto (a pasta não muda). `false` se o projeto não existe ou o nome é vazio.
    pub fn rename_project(&self, id: &str, name: &str) -> bool {
        let name = name.trim();
        !name.is_empty() && self.update_project(id, |p| p.name = name.to_owned())
    }

    /// Altera os dados de um projeto e grava (abas abertas, aba ativa, ordem e proporções das
    /// janelas). O id e a pasta não mudam. `false` se o projeto não existe.
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

    /// Tira o projeto da lista: fecha as instâncias dele e falha as perguntas pendentes nele. Se
    /// era o ativo, ativa o primeiro que sobrar (ou nenhum). `false` se o projeto não existe.
    /// A pasta não é apagada.
    pub fn remove_project(&self, id: &str) -> bool {
        if self.project_path(id).is_none() {
            return false;
        }
        // Primeiro os tokens (nenhuma pergunta nova entra) e o projeto (uma pergunta que já passou
        // da autenticação confere que ele existe depois de registrada; ver `bridge::ask_tab`).
        self.inner.callers.lock().retain(|_, c| c.project != id);
        let was_active = self.active_project_id() == id;
        self.update_config(|c| {
            c.project_list.retain(|p| p.id != id);
            if c.active_project == id {
                c.active_project = c.project_list.first().map(|p| p.id.clone()).unwrap_or_default();
                c.workspace = c.active().map(|p| p.path.clone()).unwrap_or_default();
            }
        });
        let ids = self.inner.instances.lock().remove(id).map(|list| list.ids()).unwrap_or_default();
        for instance in &ids {
            self.close_instance(id, instance);
        }
        // Perguntas que sobraram no projeto (ex.: feitas pelo token mestre).
        for p in self.inner.exchange.snapshot().into_iter().filter(|p| p.project == id) {
            self.inner.exchange.fail(p.id, "o projeto foi removido");
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

    /// Torna `id` o projeto ativo (persistido): a pasta dele vira o workspace (árvore, git,
    /// anexos), e o AGENTS.md e o watcher são refeitos.
    pub fn set_active_project(&self, id: &str) -> anyhow::Result<()> {
        let Some(path) = self.project_path(id) else { anyhow::bail!("projeto desconhecido: {id}") };
        std::fs::create_dir_all(&path)?;
        self.update_config(|c| {
            c.active_project = id.to_owned();
            c.workspace = path;
        });
        self.prepare_workspace();
        Ok(())
    }

    /// Escreve o AGENTS.md na pasta de um projeto que não é o ativo, na primeira aba aberta nele
    /// (o do ativo é mantido por [`Core::prepare_workspace`]).
    fn ensure_project_context(&self, project: &str, path: &Path) {
        if project == self.active_project_id() || !self.inner.context_written.lock().insert(project.to_owned()) {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(path) {
            log::warn!("falha ao criar a pasta do projeto {}: {e}", path.display());
        }
        let instructions = self.bridge_instructions(None);
        if let Err(e) = workspace::ensure_agent_context(path, &self.paths().app_dir, &self.inner.home, &instructions) {
            log::warn!("falha ao atualizar AGENTS.md em {}: {e}", path.display());
        }
    }

    // ---------- Workspace (a pasta do projeto ativo) ----------

    /// Pasta do projeto ativo (vazia = nenhum projeto).
    pub fn workspace(&self) -> PathBuf {
        self.inner.config.read().workspace.clone()
    }

    /// Atalho: cria (se preciso) o projeto da pasta `dir` e o torna ativo.
    pub fn set_workspace(&self, dir: PathBuf) -> anyhow::Result<()> {
        let dir = util::absolutize(&dir);
        std::fs::create_dir_all(&dir)?;
        let project = self.create_project(&dir, None);
        self.set_active_project(&project.id)
    }

    /// Cria a pasta, atualiza o AGENTS.md e (re)inicia o watcher. Equivalente a `prepareWorkspace`.
    /// Sem projeto ativo, só solta o watcher.
    fn prepare_workspace(&self) {
        let ws = self.workspace();
        let mut guard = self.inner.watcher.lock();
        *guard = None; // solta o watcher anterior antes de criar o novo
        if ws.as_os_str().is_empty() {
            return;
        }
        if let Err(e) = std::fs::create_dir_all(&ws) {
            log::warn!("falha ao criar o workspace {}: {e}", ws.display());
        }
        self.refresh_agent_context();
        if self.inner.watch_workspace && ws.is_dir() {
            match workspace::start_watcher(&ws, Arc::downgrade(&self.inner)) {
                Ok(w) => *guard = Some(w),
                Err(e) => log::warn!("falha ao iniciar o watcher do workspace: {e}"),
            }
        }
    }

    /// Reescreve o bloco do AgentDeck no AGENTS.md do projeto ativo (respeitando as regras de
    /// onde criar). Os outros projetos são reescritos na próxima aba aberta neles.
    fn refresh_agent_context(&self) {
        self.inner.context_written.lock().clear();
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return;
        }
        let instructions = self.bridge_instructions(None);
        if let Err(e) = workspace::ensure_agent_context(&ws, &self.paths().app_dir, &self.inner.home, &instructions) {
            log::warn!("falha ao atualizar AGENTS.md: {e}");
        }
    }

    /// Lista uma pasta relativa ao workspace (`""` = raiz), já filtrada e ordenada
    /// (pastas primeiro, nome case-insensitive). Mesmo filtro de `workspace:readDir`.
    /// Sem projeto ativo, nada.
    pub fn read_dir(&self, rel_dir: &str) -> Vec<DirEntry> {
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            return Vec::new();
        }
        workspace::read_dir(&ws, rel_dir)
    }

    /// Lê um arquivo de texto do workspace (até 1 MB) para a visualização do modo Documentos; sem
    /// projeto ativo, erro. Ver [`workspace::read_text`].
    pub fn read_text(&self, rel_path: &str) -> Result<workspace::TextFile, String> {
        workspace::read_text(&self.workspace(), rel_path)
    }

    /// Abre um arquivo do workspace no app padrão do sistema. Aceita caminhos relativos ao
    /// workspace ou absolutos dentro do workspace / da pasta `dados` (anexos).
    pub fn open_path(&self, rel_path: &str) -> anyhow::Result<()> {
        let ws = self.workspace();
        if ws.as_os_str().is_empty() {
            anyhow::bail!("nenhum projeto aberto");
        }
        let full = match ollama::resolve_workspace_path(rel_path, &ws) {
            Ok(p) => p,
            Err(e) => {
                let abs = util::normalize_lexical(&ws.join(rel_path));
                if !util::path_within(&abs, &util::absolutize(&self.paths().data_dir)) {
                    anyhow::bail!(e);
                }
                abs
            }
        };
        open::that_detached(&full).map_err(|e| anyhow::anyhow!("falha ao abrir {}: {e}", full.display()))
    }

    // ---------- Anexos ----------

    /// Copia arquivos para `dados/anexos` (ignora pastas). Retorna referências para citar (`@ref`).
    pub fn attach_files(&self, files: &[PathBuf]) -> Vec<String> {
        attachments::attach_files(&self.paths().data_dir, &self.workspace(), self.session_id(), files)
    }

    /// Grava bytes (ex.: imagem colada) como anexo. `name` pode ser vazio.
    pub fn attach_bytes(&self, name: &str, bytes: &[u8]) -> anyhow::Result<String> {
        attachments::attach_bytes(&self.paths().data_dir, &self.workspace(), self.session_id(), name, bytes)
    }

    /// Grava uma imagem RGBA (ex.: da área de transferência) como PNG anexo.
    pub fn attach_rgba_png(&self, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<String> {
        attachments::attach_rgba_png(&self.paths().data_dir, &self.workspace(), self.session_id(), width, height, rgba)
    }

    // ---------- Transcrição (audio:*) ----------

    /// Rótulo do melhor motor disponível ou "indisponível". Na primeira chamada no
    /// Windows detecta as GPUs (≈1 s); prefira chamar fora da thread da UI.
    pub fn transcribe_engine(&self) -> String {
        whisper::transcribe_engine(self)
    }

    /// Transcreve PCM mono 16 kHz (f32 -1..1). Grava WAV temporário, roda whisper.cpp
    /// com fallback de motores e apaga o WAV. BLOQUEANTE: chame numa thread.
    pub fn transcribe_pcm16k(&self, samples: &[f32]) -> anyhow::Result<Transcription> {
        whisper::transcribe_pcm16k(self, samples)
    }

    /// Garante modelo/binários do whisper (download em background com feed de progresso).
    pub fn ensure_whisper_async(&self) {
        whisper::ensure_whisper_async(self);
    }

    // ---------- Ponte ----------

    pub fn bridge_port(&self) -> u16 {
        *self.inner.state.bridge_port.lock()
    }

    /// Token mestre da ponte. Os agentes recebem tokens próprios (ver [`Core::tab_token`]).
    pub fn bridge_token(&self) -> &str {
        &self.inner.state.bridge_token
    }

    /// Token da aba da instância `id` do projeto `project` (o mesmo enquanto a instância existir;
    /// o `claude` de cada projeto tem o seu).
    pub(crate) fn tab_token(&self, project: &str, id: &str) -> String {
        let mut callers = self.inner.callers.lock();
        if let Some((token, _)) = callers.iter().find(|(_, c)| c.tab && c.project == project && c.id == id) {
            return token.clone();
        }
        let token = random_token();
        callers.insert(token.clone(), Caller { id: id.to_owned(), project: project.to_owned(), tab: true });
        token
    }

    /// Token de uma execução headless de `id` no projeto `project`, válido enquanto o guarda existir.
    pub(crate) fn headless_token(&self, project: &str, id: &str) -> TokenGuard {
        let token = random_token();
        self.inner.callers.lock().insert(token.clone(), Caller { id: id.to_owned(), project: project.to_owned(), tab: false });
        TokenGuard { inner: self.inner.clone(), token }
    }

    /// Quem é o dono do token (`None` = token inválido).
    pub(crate) fn authenticate(&self, token: &str) -> Option<Auth> {
        if same_secret(token, &self.inner.state.bridge_token) {
            return Some(Auth::Master);
        }
        self.inner.callers.lock().get(token).cloned().map(Auth::Agent)
    }

    /// Encerra processos filhos headless (chamado ao fechar a janela), a ponte e o watcher.
    pub fn shutdown(&self) {
        self.inner.state.shutdown.store(true, Ordering::SeqCst);
        self.inner.processes.kill_all();
        if let Some(server) = self.inner.bridge_server.lock().take() {
            server.unblock();
        }
        *self.inner.watcher.lock() = None;
    }
}

/// Converte um caminho absoluto na referência usada para citar arquivos aos agentes
/// (relativo ao workspace quando dentro dele, com `/`). Equivalente a `refPath`.
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    util::ref_path(workspace, abs)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hold(dir: &Path, name: &str) -> std::fs::File {
        let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(name)).expect("open");
        file.try_lock().expect("travar");
        file
    }

    #[test]
    fn the_data_dir_is_locked_by_one_instance() {
        let tmp = tempfile::tempdir().expect("tmp");
        let first = lock_data_dir(tmp.path()).expect("primeira");
        let err = lock_data_dir(tmp.path()).expect_err("a segunda recusa").to_string();
        assert!(err.starts_with("Outra janela do AgentDeck já está usando a pasta de dados"), "{err}");
        drop(first);
        lock_data_dir(tmp.path()).expect("livre de novo");
    }

    /// O app antigo (CommandDeck) trava `.commanddeck.lock`: o novo, na mesma pasta, recusa abrir.
    #[test]
    fn the_old_commanddeck_lock_in_use_blocks_opening() {
        let tmp = tempfile::tempdir().expect("tmp");
        let old_app = hold(tmp.path(), ".commanddeck.lock");
        let err = lock_data_dir(tmp.path()).expect_err("recusa").to_string();
        assert!(err.starts_with("Outra janela do AgentDeck já está usando a pasta de dados"), "{err}");
        // Recusar não deixa o arquivo novo travado.
        drop(old_app);
        lock_data_dir(tmp.path()).expect("sem a trava antiga, abre");
    }

    /// O app novo também segura a trava antiga: abrir o app antigo depois também é recusado.
    #[test]
    fn the_new_instance_also_holds_the_old_lock() {
        let tmp = tempfile::tempdir().expect("tmp");
        let _new_app = lock_data_dir(tmp.path()).expect("abre");
        let old =
            std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(tmp.path().join(".commanddeck.lock")).expect("open");
        assert!(matches!(old.try_lock(), Err(std::fs::TryLockError::WouldBlock)));
    }
}

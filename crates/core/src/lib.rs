//! CommandDeck Terminal — núcleo (backend) sem dependências de interface gráfica.
//!
//! Este crate é o equivalente Rust de `main.js` + `ollamaAgent.js` da versão Electron.
//! A interface (crate `commanddeck`) conversa com ele exclusivamente através de [`Core`]
//! e dos eventos [`CoreEvent`] — o mesmo papel que o IPC/preload tinha no Electron.
//!
//! CONTRATO ESTÁVEL: os tipos e assinaturas públicas deste arquivo e de `events.rs`,
//! `agents.rs` (tipos `AgentInfo`/`SpawnSpec`), `config.rs` (`Config`) e
//! `workspace.rs` (`DirEntry`) são consumidos pela UI. Não mude sem alinhar com o lead.

pub mod agents;
pub mod attachments;
pub mod bridge;
pub mod config;
pub mod context;
pub mod delivery;
pub mod download;
pub mod events;
pub mod exchange;
pub mod headless;
pub mod instances;
pub mod ollama;
pub mod paths;
pub(crate) mod process;
pub mod sessions;
pub mod table;
pub mod util;
pub mod whisper;
pub mod workspace;

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::{Mutex, RwLock};

pub use agents::{AgentInfo, SpawnSpec};
pub use config::Config;
pub use events::{CoreEvent, Delivery, DownloadProgress, DownloadState, FeedEntry, FeedKind};
pub use headless::HeadlessOptions;
pub use instances::Ident;
pub use workspace::DirEntry;

/// Função chamada sempre que um [`CoreEvent`] é emitido, para acordar o loop da UI
/// (ex.: `EventLoopProxy::send_event`). Pode ser chamada de qualquer thread.
pub type Waker = Arc<dyn Fn() + Send + Sync>;

/// Opções de uma mesa de orquestração (equivalente a `table:run`).
#[derive(Debug, Clone)]
pub struct TableRequest {
    pub task: String,
    /// Ids dos agentes participantes, na ordem de fala (ex.: `["claude", "ollama:qwen3.5:9b"]`).
    pub agents: Vec<String>,
    /// 1..=6
    pub rounds: u32,
    /// Referências de anexos (como devolvidas por `attach_*`).
    pub attachments: Vec<String>,
}

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
    /// Chave do par → id da sessão/conversa headless.
    pub headless_sessions: Mutex<HashMap<String, String>>,
    /// Instâncias de agentes (uma por aba) e as perguntas aguardando resposta entre elas.
    pub instances: Mutex<instances::Instances>,
    pub exchange: exchange::Exchange,
    /// A interface sabe entregar mensagens nas abas (senão a ponte responde em modo headless).
    pub tab_delivery: std::sync::atomic::AtomicBool,
    /// Token de cada processo de agente → quem ele é (a ponte não confia no `from` do corpo).
    pub callers: Mutex<HashMap<String, Caller>>,
    pub bridge_queues: bridge::PairQueues,
    pub bridge_server: Mutex<Option<Arc<tiny_http::Server>>>,
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub whisper: whisper::WhisperState,
    /// Trava da pasta de dados (uma instância do app por `dados/`); solta ao encerrar.
    _data_lock: Option<std::fs::File>,
}

/// Quem chamou a ponte, pelo token que recebeu no ambiente.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Caller {
    /// Id da instância (`claude`, `claude@dev`…).
    pub id: String,
    /// É o processo da aba (interativo). `false` = execução headless (mesa ou ponte sem abas).
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

/// Trava `dados/.commanddeck.lock`. Erro se outra instância do app já usa a pasta.
fn lock_data_dir(data_dir: &Path) -> anyhow::Result<std::fs::File> {
    std::fs::create_dir_all(data_dir)?;
    let path = data_dir.join(".commanddeck.lock");
    let file = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(&path)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => anyhow::bail!(
            "Outra janela do CommandDeck já está usando a pasta de dados {}.\n\nFeche-a, ou defina COMMANDDECK_HOME com outra pasta para abrir uma instância separada.",
            data_dir.display()
        ),
        Err(std::fs::TryLockError::Error(e)) => {
            log::warn!("não consegui travar {}: {e}", path.display());
            Ok(file)
        }
    }
}

pub(crate) mod state {
    //! Estado mutável simples compartilhado.
    #[derive(Default)]
    pub struct SharedState {
        pub bridge_port: parking_lot::Mutex<u16>,
        pub bridge_token: String,
        pub session_id: String,
        pub table_abort: std::sync::atomic::AtomicBool,
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
            None => {
                let mut candidates = vec![paths.config_file.clone()];
                candidates.extend(config::legacy_config_files());
                Config::load_from(&candidates, &paths, &home)
            }
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
            instances: Mutex::new(instances::Instances::default()),
            exchange: exchange::Exchange::default(),
            tab_delivery: std::sync::atomic::AtomicBool::new(false),
            callers: Mutex::new(HashMap::new()),
            bridge_queues: bridge::PairQueues::default(),
            bridge_server: Mutex::new(None),
            watcher: Mutex::new(None),
            whisper: whisper::WhisperState::default(),
            _data_lock: Some(data_lock),
            paths,
        };
        let core = Core { inner: Arc::new(inner) };

        core.prepare_workspace();
        sessions::register_and_cleanup(&core.paths().data_dir, &core.inner.state.session_id);
        let (server, port) = bridge::start(&core)?;
        *core.inner.bridge_server.lock() = Some(server);
        *core.inner.state.bridge_port.lock() = port;

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

    /// Motor do agente Ollama compartilhado (conversas da ponte/mesas).
    pub fn ollama_agent(&self) -> &ollama::OllamaAgent {
        &self.inner.ollama
    }

    // ---------- Configuração (config:get / config:save) ----------

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    /// Aplica uma alteração e persiste em disco.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let snapshot = {
            let mut cfg = self.inner.config.write();
            f(&mut cfg);
            cfg.clone()
        };
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

    /// Monta o comando para abrir um terminal interativo do agente `kind`
    /// (`claude`, `antigravity`, `ollama`, `ollama:<modelo>`, `shell`, ...).
    /// Erro = mensagem amigável de "não instalado" a ser escrita no terminal.
    pub fn spawn_spec(&self, kind: &str) -> Result<SpawnSpec, String> {
        agents::spawn_spec(self, kind)
    }

    /// Como [`Core::spawn_spec`] para uma instância (modelo + cargo + nome): o id dela vai em
    /// `COMMANDDECK_AGENT` e o cargo em `COMMANDDECK_ROLE`.
    pub fn spawn_spec_for(&self, ident: &Ident) -> Result<SpawnSpec, String> {
        agents::spawn_spec_for(self, ident)
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

    /// Nova instância aberta pelo usuário (menu "+"): `claude`, e `claude@main@ana` se já há uma.
    pub fn register_instance(&self, kind: &str) -> Ident {
        self.inner.instances.lock().allocate(kind, None, None)
    }

    /// Reabre uma instância com o id que ela tinha (restauração das abas). `None` se o id é inválido.
    pub fn restore_instance(&self, id: &str) -> Option<Ident> {
        let ident = Ident::parse(id)?;
        self.inner.instances.lock().register(ident.clone());
        Some(ident)
    }

    /// A aba da instância fechou: esquece a instância e o token dela, avisa quem esperava
    /// resposta dela e descarta as perguntas que ela fez (ninguém mais lê as respostas).
    pub fn close_instance(&self, id: &str) {
        self.inner.instances.lock().remove(id);
        self.inner.callers.lock().retain(|_, c| !(c.tab && c.id == id));
        self.inner.exchange.fail_target(id, &format!("a aba de \"{id}\" foi fechada antes de responder"));
        self.inner.exchange.fail_asker(id, &format!("a aba de \"{id}\" foi fechada"));
    }

    /// O processo da instância terminou (a aba continua aberta): quem esperava a resposta dela é liberado.
    pub fn instance_ended(&self, id: &str) {
        self.inner.exchange.fail_target(id, &format!("o processo de \"{id}\" encerrou antes de responder"));
    }

    /// A entrega não pôde ser feita (aba morreu, não subiu…): libera quem espera.
    pub fn fail_delivery(&self, request: u64, reason: &str) {
        self.inner.exchange.fail(request, reason);
    }

    /// Ids das instâncias abertas.
    pub fn instance_ids(&self) -> Vec<String> {
        self.inner.instances.lock().ids()
    }

    /// Ids que podem ser usados com `ask-agent`: os agentes instalados e as instâncias abertas.
    pub fn peers(&self) -> Vec<String> {
        let mut out = self.inner.agents.peers();
        for id in self.inner.instances.lock().ids() {
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

    // ---------- Mesas (table:run / table:stop) ----------

    /// Executa uma mesa de orquestração. BLOQUEIA até terminar: chame numa thread.
    /// Progresso chega via [`CoreEvent::Feed`]; ao final emite [`CoreEvent::TableFinished`].
    pub fn run_table(&self, req: TableRequest) {
        table::run_table(self, req);
    }

    /// Interrompe a mesa em andamento e mata processos headless.
    pub fn stop_table(&self) {
        self.inner.state.table_abort.store(true, Ordering::SeqCst);
        self.inner.processes.kill_all();
    }

    // ---------- Workspace ----------

    pub fn workspace(&self) -> PathBuf {
        self.inner.config.read().workspace.clone()
    }

    /// Troca a pasta de trabalho (persistida), recria AGENTS.md e o watcher.
    pub fn set_workspace(&self, dir: PathBuf) -> anyhow::Result<()> {
        let dir = util::absolutize(&dir);
        std::fs::create_dir_all(&dir)?;
        self.update_config(|c| c.workspace = dir);
        self.prepare_workspace();
        Ok(())
    }

    /// Cria a pasta, atualiza o AGENTS.md e (re)inicia o watcher. Equivalente a `prepareWorkspace`.
    fn prepare_workspace(&self) {
        let ws = self.workspace();
        if let Err(e) = std::fs::create_dir_all(&ws) {
            log::warn!("falha ao criar o workspace {}: {e}", ws.display());
        }
        self.refresh_agent_context();
        let mut guard = self.inner.watcher.lock();
        *guard = None; // solta o watcher anterior antes de criar o novo
        if self.inner.watch_workspace && ws.is_dir() {
            match workspace::start_watcher(&ws, Arc::downgrade(&self.inner)) {
                Ok(w) => *guard = Some(w),
                Err(e) => log::warn!("falha ao iniciar o watcher do workspace: {e}"),
            }
        }
    }

    /// Reescreve o bloco do CommandDeck no AGENTS.md (respeitando as regras de onde criar).
    fn refresh_agent_context(&self) {
        let instructions = self.bridge_instructions(None);
        if let Err(e) = workspace::ensure_agent_context(&self.workspace(), &self.paths().app_dir, &self.inner.home, &instructions) {
            log::warn!("falha ao atualizar AGENTS.md: {e}");
        }
    }

    /// Lista uma pasta relativa ao workspace (`""` = raiz), já filtrada e ordenada
    /// (pastas primeiro, nome case-insensitive). Mesmo filtro de `workspace:readDir`.
    pub fn read_dir(&self, rel_dir: &str) -> Vec<DirEntry> {
        workspace::read_dir(&self.workspace(), rel_dir)
    }

    /// Abre um arquivo do workspace no app padrão do sistema. Aceita caminhos relativos ao
    /// workspace ou absolutos dentro do workspace / da pasta `dados` (anexos).
    pub fn open_path(&self, rel_path: &str) -> anyhow::Result<()> {
        let ws = self.workspace();
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

    /// Token da aba da instância `id` (o mesmo enquanto a instância existir).
    pub(crate) fn tab_token(&self, id: &str) -> String {
        let mut callers = self.inner.callers.lock();
        if let Some((token, _)) = callers.iter().find(|(_, c)| c.tab && c.id == id) {
            return token.clone();
        }
        let token = random_token();
        callers.insert(token.clone(), Caller { id: id.to_owned(), tab: true });
        token
    }

    /// Token de uma execução headless de `id`, válido enquanto o guarda existir.
    pub(crate) fn headless_token(&self, id: &str) -> TokenGuard {
        let token = random_token();
        self.inner.callers.lock().insert(token.clone(), Caller { id: id.to_owned(), tab: false });
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
        self.inner.state.table_abort.store(true, Ordering::SeqCst);
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

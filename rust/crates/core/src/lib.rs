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
pub mod events;
pub mod headless;
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
pub use events::{CoreEvent, FeedEntry, FeedKind};
pub use headless::HeadlessOptions;
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
}

impl Default for StartOptions {
    fn default() -> Self {
        Self { search_path: None, home_dir: None, ollama_discovery: true, watch_workspace: true, config: None }
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
    pub bridge_queues: bridge::PairQueues,
    pub bridge_server: Mutex<Option<Arc<tiny_http::Server>>>,
    pub watcher: Mutex<Option<notify::RecommendedWatcher>>,
    pub whisper: whisper::WhisperState,
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
            state: state::SharedState {
                bridge_token: random_token(),
                session_id: sessions::new_session_id(),
                ..Default::default()
            },
            agents: agents::Registry::new(agents::Discovery {
                search_path: opts.search_path,
                home: home.clone(),
                ollama: opts.ollama_discovery,
            }),
            home,
            watch_workspace: opts.watch_workspace,
            providers,
            processes: process::ProcessRegistry::default(),
            headless_sessions: Mutex::new(HashMap::new()),
            bridge_queues: bridge::PairQueues::default(),
            bridge_server: Mutex::new(None),
            watcher: Mutex::new(None),
            whisper: whisper::WhisperState::default(),
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

    /// Ids que podem ser usados com `ask-agent` (equivalente a `peers()`).
    pub fn peers(&self) -> Vec<String> {
        self.inner.agents.peers()
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

    pub fn bridge_token(&self) -> &str {
        &self.inner.state.bridge_token
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

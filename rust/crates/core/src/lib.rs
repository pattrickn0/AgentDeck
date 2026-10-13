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
pub mod sessions;
pub mod table;
pub mod util;
pub mod whisper;
pub mod workspace;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};
use parking_lot::RwLock;

pub use agents::{AgentInfo, SpawnSpec};
pub use config::Config;
pub use events::{CoreEvent, FeedEntry, FeedKind};
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
}

pub(crate) mod state {
    //! Estado mutável compartilhado (sessões headless, processos rodando, fila da ponte...).
    //! Livre para o implementador do core estruturar como quiser.
    #[derive(Default)]
    pub struct SharedState {
        pub bridge_port: parking_lot::Mutex<u16>,
        pub bridge_token: String,
        pub session_id: String,
        pub table_abort: std::sync::atomic::AtomicBool,
    }
}

impl Core {
    /// Inicializa o backend: carrega config, prepara workspace (AGENTS.md + watcher),
    /// registra a sessão/limpeza de retenção e sobe a ponte HTTP em 127.0.0.1:0.
    /// Retorna o handle e o receptor de eventos para a UI.
    pub fn start(paths: paths::AppPaths) -> anyhow::Result<(Core, Receiver<CoreEvent>)> {
        let (tx, rx) = crossbeam_channel::unbounded();
        let inner = CoreInner {
            config: RwLock::new(Config::default_for(&paths)),
            paths,
            events_tx: tx,
            waker: RwLock::new(None),
            state: state::SharedState::default(),
        };
        let core = Core { inner: Arc::new(inner) };
        // TODO(dev-core): loadConfig, prepareWorkspace, registerSessionAndCleanup, startBridge, refresh ollama.
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

    // ---------- Configuração (config:get / config:save) ----------

    pub fn config(&self) -> Config {
        self.inner.config.read().clone()
    }

    /// Aplica uma alteração e persiste em disco.
    pub fn update_config(&self, f: impl FnOnce(&mut Config)) {
        let mut cfg = self.inner.config.write();
        f(&mut cfg);
        // TODO(dev-core): saveConfig
    }

    // ---------- Agentes (agents:list / pty:spawn) ----------

    /// Lista de agentes para menus (mesma semântica de `agents:list`, incluindo
    /// expansão `ollama:<modelo>`). Pode bloquear até ~1,5 s (consulta /api/tags):
    /// chame fora da thread da UI.
    pub fn list_agents(&self) -> Vec<AgentInfo> {
        Vec::new()
    }

    /// Monta o comando para abrir um terminal interativo do agente `kind`
    /// (`claude`, `antigravity`, `ollama`, `ollama:<modelo>`, `shell`, ...).
    /// Erro = mensagem amigável de "não instalado" a ser escrita no terminal.
    pub fn spawn_spec(&self, kind: &str) -> Result<SpawnSpec, String> {
        let _ = kind;
        Err("não implementado".into())
    }

    /// Ids que podem ser usados com `ask-agent` (equivalente a `peers()`).
    pub fn peers(&self) -> Vec<String> {
        Vec::new()
    }

    // ---------- Mesas (table:run / table:stop) ----------

    /// Executa uma mesa de orquestração. BLOQUEIA até terminar: chame numa thread.
    /// Progresso chega via [`CoreEvent::Feed`]; ao final emite [`CoreEvent::TableFinished`].
    pub fn run_table(&self, req: TableRequest) {
        let _ = req;
        self.emit(CoreEvent::TableFinished);
    }

    /// Interrompe a mesa em andamento e mata processos headless.
    pub fn stop_table(&self) {}

    // ---------- Workspace ----------

    pub fn workspace(&self) -> PathBuf {
        self.inner.config.read().workspace.clone()
    }

    /// Troca a pasta de trabalho (persistida), recria AGENTS.md e o watcher.
    pub fn set_workspace(&self, dir: PathBuf) -> anyhow::Result<()> {
        self.update_config(|c| c.workspace = dir);
        Ok(())
    }

    /// Lista uma pasta relativa ao workspace (`""` = raiz), já filtrada e ordenada
    /// (pastas primeiro, nome case-insensitive). Mesmo filtro de `workspace:readDir`.
    pub fn read_dir(&self, rel_dir: &str) -> Vec<DirEntry> {
        workspace::read_dir(&self.workspace(), rel_dir)
    }

    /// Abre um arquivo do workspace no app padrão do sistema.
    pub fn open_path(&self, rel_path: &str) -> anyhow::Result<()> {
        let _ = rel_path;
        Ok(())
    }

    // ---------- Anexos ----------

    /// Copia arquivos para `dados/anexos` (ignora pastas). Retorna referências para citar (`@ref`).
    pub fn attach_files(&self, files: &[PathBuf]) -> Vec<String> {
        let _ = files;
        Vec::new()
    }

    /// Grava bytes (ex.: imagem colada) como anexo. `name` pode ser vazio.
    pub fn attach_bytes(&self, name: &str, bytes: &[u8]) -> anyhow::Result<String> {
        let _ = (name, bytes);
        anyhow::bail!("não implementado")
    }

    /// Grava uma imagem RGBA (ex.: da área de transferência) como PNG anexo.
    pub fn attach_rgba_png(&self, width: u32, height: u32, rgba: &[u8]) -> anyhow::Result<String> {
        let _ = (width, height, rgba);
        anyhow::bail!("não implementado")
    }

    // ---------- Transcrição (audio:*) ----------

    /// Rótulo do melhor motor disponível ou "indisponível".
    pub fn transcribe_engine(&self) -> String {
        "indisponível".into()
    }

    /// Transcreve PCM mono 16 kHz (f32 -1..1). Grava WAV temporário, roda whisper.cpp
    /// com fallback de motores e apaga o WAV. BLOQUEANTE: chame numa thread.
    pub fn transcribe_pcm16k(&self, samples: &[f32]) -> anyhow::Result<Transcription> {
        let _ = samples;
        anyhow::bail!("a transcrição local ainda não está pronta (veja a Conversa integrada)")
    }

    /// Garante modelo/binários do whisper (download em background com feed de progresso).
    pub fn ensure_whisper_async(&self) {}

    // ---------- Ponte ----------

    pub fn bridge_port(&self) -> u16 {
        *self.inner.state.bridge_port.lock()
    }

    pub fn bridge_token(&self) -> &str {
        &self.inner.state.bridge_token
    }

    /// Encerra processos filhos headless (chamado ao fechar a janela).
    pub fn shutdown(&self) {}
}

/// Converte um caminho absoluto na referência usada para citar arquivos aos agentes
/// (relativo ao workspace quando dentro dele, com `/`). Equivalente a `refPath`.
pub fn ref_path(workspace: &Path, abs: &Path) -> String {
    util::ref_path(workspace, abs)
}

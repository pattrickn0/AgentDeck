//! Mesa de tarefas: a unidade de trabalho do AgentDeck.
//!
//! Uma **tarefa** tem um pedido (o brief), um agente com modelo, uma worktree + branch próprias, um
//! relatório, um diff e um ciclo de vida que termina em validação (concluída) ou descarte:
//!
//! ```text
//! Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged (concluída)
//!   │  │              ▲                 │ └─discard──▶ Discarded     │ └─discard──▶ Discarded
//!   │  │              ├───return────────┘                            │
//!   │  │              └───────────────reject─────────────────────────┘
//!   │  └─ escopo ocupado ─▶ Queued ──(a outra sai de Running/Review)──▶ Running
//!   └─ start com erro ─▶ Failed ──(start de novo)──▶ Running
//! ```
//!
//! **Escopo e fila:** a tarefa pode declarar os caminhos que vai mexer ([`Task::scope`], ver [`crate::scope`]). Ao
//! iniciar, se o escopo se sobrepõe ao de uma tarefa em andamento ou em revisão (ou ao de outra que já espera na
//! fila, que chegou antes), ela fica em `Queued`, dizendo por qual espera ([`Task::waiting_for`]), e começa sozinha
//! quando essa tarefa é mesclada, descartada ou falha ([`Core::advance_queue`]), partindo da base daquele momento.
//! `Validating` não segura a fila: o código já está na base. Sem escopo, nada disso vale.
//!
//! Mesclar é "o diff parecia certo"; o teste de verdade o usuário faz depois, no app compilado. Por isso
//! mesclar **não** encerra o agente: a tarefa fica em `Validating` (aba, worktree e branch vivas, a branch
//! alinhada com a base) até o usuário validar (limpa tudo) ou reprovar (volta ao mesmo agente).
//!
//! - **Armazenamento:** `dados/tarefas/<projeto>.json`, gravado por arquivo temporário + `rename`
//!   (uma gravação por vez), fora do repositório do usuário. O contador (`t1`, `t2`…) nunca reaproveita número.
//! - **Worktrees:** `<raiz do repositório>/.agentdeck/worktrees/<id>`, dentro do repositório de propósito
//!   (agentes confinados ao workspace, como o Antigravity, conseguem chegar lá). A pasta `/.agentdeck/` entra
//!   uma vez em `<git comum>/info/exclude` (nunca no `.gitignore`).
//! - **Git:** o mesmo executor seguro de [`crate::git`] (sem shell, sem prompts, com limite de tempo).
//! - **Threads:** os métodos que rodam o git são bloqueantes; a interface os chama numa thread. Uma
//!   operação do ciclo de vida por vez (`TaskStore::op`); consultas só pegam uma trava curta.
//! - **Agente:** a instância da tarefa é `<modelo>@t<n>` (`claude@t3`), com o terminal na worktree
//!   ([`Core::task_cwd`], usado por `agents::spawn_spec_for`). O brief e o feedback chegam pelo mesmo
//!   caminho das mensagens entre agentes (`CoreEvent::Deliver` com `request: 0`, fila e regras de entrega).
//!
//! Quem pode o quê na ponte está em `bridge.rs`: o agente da tarefa só entrega o relatório (`done`) da
//! própria tarefa e consulta; o resto é de quem criou a tarefa (outro agente ou o usuário).

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capability::Level;
use crate::events::{FeedEntry, FeedKind};
use crate::git::GitError;
use crate::models;
use crate::util::now_ms;

/// Eventos guardados por tarefa (os mais antigos saem).
pub const MAX_EVENTS: usize = 200;
/// Feedbacks guardados por tarefa (os mais antigos saem), e o tamanho de cada um ao reaparecer numa mensagem.
pub const MAX_FEEDBACKS: usize = 20;
/// Mensagens guardadas por tarefa (as mais antigas saem).
pub const MAX_MESSAGES: usize = 300;
/// Tamanho de cada mensagem guardada (em caracteres); o resto é cortado com um aviso.
pub const MAX_MESSAGE_CHARS: usize = 6_000;
const HISTORY_ITEM_CHARS: usize = 700;
/// Mensagens ao agente mostram só os últimos feedbacks anteriores.
const HISTORY_SHOWN: usize = 5;
/// Tamanho máximo do título (em caracteres).
pub const MAX_TITLE: usize = 120;
/// Tamanho máximo do slug do título na branch.
pub const MAX_SLUG: usize = 40;
/// Relatório e brief maiores que isto são cortados (em caracteres).
pub const MAX_TEXT: usize = 200_000;
/// Texto do `git diff --stat` guardado (em caracteres).
/// Arquivos fora do escopo guardados por tarefa.
const MAX_OUTSIDE: usize = 100;
const MAX_STAT: usize = 8_000;
/// Nome da pasta, na raiz do repositório, que guarda as worktrees.
pub const AGENTDECK_DIR: &str = ".agentdeck";
/// Tentativas (e espera entre elas) para apagar a pasta de uma worktree enquanto o terminal do agente
/// ainda a segura (no Windows, o processo demora um instante a morrer depois de a aba fechar).
const REMOVE_TRIES: u32 = 10;
const REMOVE_WAIT: Duration = Duration::from_millis(300);

// ---------- Tipos ----------

/// Em que ponto do ciclo de vida a tarefa está.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Criada; ainda sem worktree nem agente.
    #[default]
    Draft,
    /// Iniciada, mas o escopo se sobrepõe ao de outra tarefa em andamento: espera a vez, sem worktree nem agente.
    /// Começa sozinha quando o escopo liberar.
    Queued,
    /// O agente está trabalhando na worktree.
    Running,
    /// O agente entregou o relatório: espera a revisão (mesclar, devolver ou descartar).
    Review,
    /// Mesclada na base e testada pelo usuário: concluída; worktree e branch removidas. (O nome `Merged`
    /// fica no JSON para não quebrar tarefas antigas; para o usuário é "concluída".)
    Merged,
    /// Mesclada na base, esperando o usuário testar: o agente, a worktree e a branch continuam.
    Validating,
    /// Descartada; worktree e branch removidas.
    Discarded,
    /// Erro ao iniciar (ou de infraestrutura ao mesclar); a mensagem fica em `Task::error`.
    Failed,
}

impl TaskStatus {
    /// Nome estável (o mesmo do JSON e da ponte).
    pub fn as_str(self) -> &'static str {
        match self {
            TaskStatus::Draft => "draft",
            TaskStatus::Queued => "queued",
            TaskStatus::Running => "running",
            TaskStatus::Review => "review",
            TaskStatus::Merged => "merged",
            TaskStatus::Validating => "validating",
            TaskStatus::Discarded => "discarded",
            TaskStatus::Failed => "failed",
        }
    }

    /// Nome para mostrar a pessoas.
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Draft => "rascunho",
            TaskStatus::Queued => "na fila",
            TaskStatus::Running => "em andamento",
            TaskStatus::Review => "em revisão",
            TaskStatus::Merged => "concluída",
            TaskStatus::Validating => "aguardando validação",
            TaskStatus::Discarded => "descartada",
            TaskStatus::Failed => "com erro",
        }
    }

    /// Concluída ou descartada: não há mais o que fazer com ela. `Validating` não é final: o agente continua vivo.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Merged | TaskStatus::Discarded)
    }
}

/// Uma linha do histórico curto da tarefa.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub at_ms: u64,
    pub text: String,
}

/// Um feedback dado ao agente: o pedido de ajustes da revisão ou o motivo da reprovação na validação.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feedback {
    pub at_ms: u64,
    /// `revisão` ou `validação`.
    pub origin: String,
    pub text: String,
}

/// Uma mensagem da linha do tempo da tarefa: pergunta, resposta ou erro em que a instância dela fala ou ouve.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMessage {
    pub at_ms: u64,
    pub kind: FeedKind,
    /// Id da instância que fala, sem o projeto.
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: String,
}

/// Uma tarefa. Os campos são os do JSON em disco e os da ponte (`GET /tasks/<id>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    /// `t<n>`, um contador por projeto que nunca reaproveita número.
    pub id: String,
    pub title: String,
    /// O pedido, em markdown.
    pub brief: String,
    pub status: TaskStatus,
    /// Branch de onde a tarefa parte e em que é mesclada.
    pub base: String,
    /// `tarefa/<id>-<slug do título>`.
    pub branch: String,
    /// Pasta (absoluta) da worktree: `<raiz do repositório>/.agentdeck/worktrees/<id>`.
    pub worktree: PathBuf,
    /// O tamanho do trabalho (`simples`, `media`, `complexa`), que decide quais modelos dão conta; não é o esforço.
    /// Tarefas antigas, sem o campo, valem `media`.
    pub level: Level,
    /// Agente (`claude`, `antigravity`, `opencode`, `codex`…).
    pub kind: String,
    /// Modelo de LLM (pelas regras de `models.rs`); `None` = o padrão do CLI.
    pub model: Option<String>,
    /// Esforço pedido (`models::Effort`); `None` = o padrão do CLI. No Claude vai em `--effort` ao abrir a
    /// aba; no Antigravity já está no sufixo de `model`.
    pub effort: Option<models::Effort>,
    /// Id da instância (`claude@t3`).
    pub instance: String,
    /// Quem criou: o id de um agente (`claude`) ou `usuário`. É o chefe da instância quando é um agente aberto.
    pub creator: Option<String>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    /// Quando o relatório chegou (`Review`), quando foi mesclada (`Validating`) ou quando a tarefa terminou (`Merged`/`Discarded`).
    pub finished_ms: Option<u64>,
    /// O que o agente entregou em `ask-agent --task done`.
    pub report: Option<String>,
    /// `git diff --stat base...branch` no momento do relatório.
    pub diff_stat: Option<String>,
    /// A mensagem do erro, em `Failed`.
    pub error: Option<String>,
    /// Hash completo do commit da última mesclagem na base (`git rev-parse HEAD` logo depois do `merge --no-ff`);
    /// `None` se a tarefa nunca foi mesclada. Uma nova mesclagem (depois de reprovar) sobrescreve. Serve ao diff
    /// "o que foi mesclado" em `Validating`/`Merged` e a `task_merge` reconhecer uma tarefa já mesclada antes.
    pub merge_commit: Option<String>,
    /// Histórico curto (no máximo [`MAX_EVENTS`]).
    pub events: Vec<TaskEvent>,
    /// Os feedbacks já dados ao agente, do mais antigo ao mais novo (no máximo [`MAX_FEEDBACKS`]); voltam
    /// nas mensagens de devolução e de reprovação para o agente não repetir o que já lhe foi dito.
    pub feedbacks: Vec<Feedback>,
    /// Linha do tempo das mensagens do `ask-agent` em que a instância da tarefa fala ou ouve, da mais antiga
    /// para a mais nova (no máximo [`MAX_MESSAGES`]). Ver [`Task::record_message`].
    pub messages: Vec<TaskMessage>,
    /// Os caminhos (arquivos e pastas, relativos à raiz do repositório, já normalizados) que a tarefa declarou que
    /// vai mexer; vazio = sem escopo (nunca entra na fila nem segura ninguém). Ver [`crate::scope`].
    pub scope: Vec<String>,
    /// Quando entrou na fila (`Queued`); define a ordem entre as que esperam. `None` fora da fila.
    pub queued_ms: Option<u64>,
    /// Em `Queued`, o id da tarefa por quem espera.
    pub waiting_for: Option<String>,
    /// Os arquivos do diff entregue que ficam fora do escopo (sem `memory/**` e `Cargo.lock`); a revisão mostra
    /// como aviso, nunca como bloqueio. Vazio sem escopo.
    pub outside_scope: Vec<String>,
}

impl Task {
    /// A entrada do feed pertence à linha do tempo desta tarefa? Só as perguntas, respostas e erros em que a
    /// instância da tarefa é quem manda ou quem recebe (o indicador "pensando" é transitório; o aviso do
    /// sistema não tem remetente e a tarefa já registra o que importa no histórico).
    pub fn owns_message(&self, entry: &FeedEntry) -> bool {
        matches!(entry.kind, FeedKind::Ask | FeedKind::Answer | FeedKind::Error)
            && !self.instance.is_empty()
            && (entry.from.as_deref() == Some(self.instance.as_str()) || entry.to.as_deref() == Some(self.instance.as_str()))
    }

    /// Guarda a entrada na linha do tempo (texto cortado em [`MAX_MESSAGE_CHARS`]; as mais antigas saem além
    /// de [`MAX_MESSAGES`]). Devolve `false`, sem guardar, se ela não é desta tarefa.
    pub fn record_message(&mut self, entry: &FeedEntry) -> bool {
        if !self.owns_message(entry) {
            return false;
        }
        self.messages.push(TaskMessage {
            at_ms: entry.ts,
            kind: entry.kind,
            from: entry.from.clone(),
            to: entry.to.clone(),
            text: cap_text(&entry.text, MAX_MESSAGE_CHARS),
        });
        if self.messages.len() > MAX_MESSAGES {
            let extra = self.messages.len() - MAX_MESSAGES;
            self.messages.drain(..extra);
        }
        true
    }

    /// Guarda um feedback entregue ao agente (os mais antigos saem além de [`MAX_FEEDBACKS`]).
    pub fn add_feedback(&mut self, origin: &str, text: &str) {
        self.feedbacks.push(Feedback { at_ms: now_ms(), origin: origin.to_owned(), text: text.to_owned() });
        if self.feedbacks.len() > MAX_FEEDBACKS {
            let extra = self.feedbacks.len() - MAX_FEEDBACKS;
            self.feedbacks.drain(..extra);
        }
    }

    /// Acrescenta uma linha ao histórico.
    pub fn log(&mut self, text: impl Into<String>) {
        self.events.push(TaskEvent { at_ms: now_ms(), text: text.into() });
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }

    /// Versão enxuta (sem brief, relatório nem histórico) para listas.
    pub fn summary_json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "title": self.title,
            "status": self.status,
            "base": self.base,
            "branch": self.branch,
            "level": self.level,
            "kind": self.kind,
            "model": self.model,
            "effort": self.effort,
            "instance": self.instance,
            "creator": self.creator,
            "created_ms": self.created_ms,
            "started_ms": self.started_ms,
            "finished_ms": self.finished_ms,
            "has_report": self.report.is_some(),
            "error": self.error,
            "scope": self.scope,
            "waiting_for": self.waiting_for,
        })
    }
}

/// O que se informa para criar uma tarefa ([`Core::task_create`]).
#[derive(Debug, Clone, Default)]
pub struct TaskSpec {
    pub title: String,
    /// O pedido (markdown). Pode ficar vazio: o título vai no lugar ao entregar ao agente.
    pub brief: String,
    /// Agente; vazio = escolher o CLI oficial com mais cota disponível. Aceita os apelidos (`agy`).
    pub kind: String,
    /// Modelo pedido; `None` = o padrão do cargo ou do CLI.
    pub model: Option<String>,
    /// Esforço pedido; `None` = o padrão do CLI. Só vale onde [`models::effort_unavailable`] não objeta.
    pub effort: Option<models::Effort>,
    /// O tamanho do trabalho ([`Task::level`]); sem ele vale `media`.
    pub level: Level,
    /// Branch de partida e de mesclagem; `None` = a branch atual do repositório do projeto.
    pub base: Option<String>,
    /// Quem pediu (id de agente ou `usuário`).
    pub creator: Option<String>,
    /// Os caminhos que a tarefa vai mexer ([`Task::scope`]); vazio = sem escopo. Normalizados em `task_create`.
    pub scope: Vec<String>,
}

/// Por que uma operação com tarefas não foi feita. `http_status` é o código da ponte.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// Projeto ou tarefa que não existe (404).
    NotFound(String),
    /// Pedido mal formado: título vazio, agente ou modelo inválidos, projeto sem git… (400).
    Invalid(String),
    /// Fora do momento certo ou com pendência a resolver: estado errado, repositório principal sujo, conflito (409).
    Conflict(String),
    /// Erro de infraestrutura (git, disco) (500).
    Failed(String),
}

impl TaskError {
    /// A mensagem, sem o tipo.
    pub fn message(&self) -> &str {
        match self {
            TaskError::NotFound(m) | TaskError::Invalid(m) | TaskError::Conflict(m) | TaskError::Failed(m) => m,
        }
    }

    /// Código HTTP da ponte para este erro.
    pub fn http_status(&self) -> u16 {
        match self {
            TaskError::Invalid(_) => 400,
            TaskError::NotFound(_) => 404,
            TaskError::Conflict(_) => 409,
            TaskError::Failed(_) => 500,
        }
    }
}

impl std::fmt::Display for TaskError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message())
    }
}

impl std::error::Error for TaskError {}

/// Uma recusa da mesclagem que tem saída que a tela pode oferecer num botão (a mensagem do erro continua sendo o
/// texto para a linha de comando e para os agentes).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeBlock {
    /// O repositório principal tem alterações suas sem commit (os arquivos): "guardar, mesclar e devolver".
    MainDirty(Vec<String>),
    /// A pasta da tarefa tem trabalho sem commit (os arquivos): "pedir ao agente para fazer o commit".
    TaskDirty(Vec<String>),
    /// A mesclagem deu conflito e foi desfeita (os arquivos): "pedir ao agente para resolver o conflito".
    Conflict(Vec<String>),
}

impl MergeBlock {
    /// Os arquivos, completos (a mensagem do erro corta em cinco).
    pub fn files(&self) -> &[String] {
        match self {
            MergeBlock::MainDirty(f) | MergeBlock::TaskDirty(f) | MergeBlock::Conflict(f) => f,
        }
    }

    /// A explicação para quem não é desenvolvedor, no aviso da tela.
    pub fn explain(&self, task: &Task) -> String {
        match self {
            MergeBlock::MainDirty(f) => format!(
                "Há alterações suas, ainda sem commit, na pasta principal do projeto ({}). O git não mescla por cima delas. \
                 Você pode guardá-las por um instante: o AgentDeck mescla a tarefa e devolve cada alteração ao lugar.",
                short_list(f)
            ),
            MergeBlock::TaskDirty(f) => format!(
                "O agente deixou trabalho sem commit na pasta da tarefa ({}). Só o que foi commitado entra na mesclagem. \
                 Peça a ele para fazer o commit e entregar de novo.",
                short_list(f)
            ),
            MergeBlock::Conflict(f) => format!(
                "A tarefa e a branch \"{}\" mudaram as mesmas linhas ({}). Nada foi mesclado e a tarefa continua em revisão. \
                 Peça ao agente para juntar as duas versões e entregar de novo.",
                task.base,
                if f.is_empty() { "arquivos não identificados".to_owned() } else { short_list(f) }
            ),
        }
    }

    /// O pedido pronto para devolver a tarefa ao agente (`task_return`); `None` para o que o agente não resolve.
    pub fn return_feedback(&self, task: &Task) -> Option<String> {
        let list = |f: &[String]| f.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n");
        match self {
            MergeBlock::MainDirty(_) => None,
            MergeBlock::TaskDirty(f) => Some(format!(
                "Não consegui mesclar a sua tarefa: a sua pasta ({wt}) tem trabalho sem commit, e só o que está commitado entra na mesclagem. Arquivos:\n\n{files}\n\n\
                 Faça o commit do que faz parte da tarefa (git add e git commit) e descarte o que não faz (git restore, ou apague o arquivo). \
                 Não deixe nada pendente: confira com git status e entregue de novo.",
                wt = task.worktree.display(),
                files = list(f),
            )),
            MergeBlock::Conflict(f) => Some(format!(
                "A mesclagem da sua branch ({branch}) em {base} deu conflito e foi desfeita. Arquivos em conflito:\n\n{files}\n\n\
                 Na sua pasta ({wt}), rode git merge {base}, resolva os conflitos mantendo o que as duas versões trouxeram de útil, \
                 rode os testes, faça o commit da mesclagem e entregue de novo.",
                branch = task.branch,
                base = task.base,
                wt = task.worktree.display(),
                files = if f.is_empty() { "- (o git não informou; veja com git merge)".to_owned() } else { list(f) },
            )),
        }
    }
}

/// A mesclagem recusada: o erro de sempre e, quando há saída para o usuário, o motivo estruturado.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRefusal {
    pub error: TaskError,
    pub block: Option<MergeBlock>,
    /// O aviso do stash ("guardar minhas alterações"), que também vai no fim do erro: a tela que troca o erro pela
    /// explicação de `block` o mostra à parte, senão o usuário não sabe onde ficaram as alterações dele.
    pub stash_note: Option<String>,
}

impl From<TaskError> for MergeRefusal {
    fn from(error: TaskError) -> Self {
        MergeRefusal { error, block: None, stash_note: None }
    }
}

/// A mesclagem feita; `note` é o aviso sobre as alterações guardadas durante ela (devolvidas, ou ainda no stash).
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub task: Task,
    pub note: Option<String>,
}

fn git_error(e: GitError) -> TaskError {
    TaskError::Failed(e.to_string())
}

mod finish;
mod git_ops;
mod lifecycle;
mod merge;
mod stash;
mod store;
mod text;

pub use git_ops::dirty_paths;
pub use lifecycle::exclude_with_agentdeck;
pub use text::{boot_line, branch_name, clean_title, memory_block, parse_task_id, reject_message, return_message, slugify, start_message};

pub(crate) use git_ops::{changed_files, union_files, worktree_dirty_unlocked};
pub(crate) use store::TaskStore;

use text::{cap_text, short_list};

#[cfg(test)]
mod tests {
    use super::*;

    pub(super) fn sample() -> Task {
        Task {
            id: "t3".into(),
            title: "Corrigir o login".into(),
            brief: "Faça X.\n\nDepois Y.".into(),
            base: "main".into(),
            branch: "tarefa/t3-corrigir-o-login".into(),
            worktree: PathBuf::from("/repo/.agentdeck/worktrees/t3"),
            ..Task::default()
        }
    }

    pub(super) fn entry(kind: FeedKind, from: Option<&str>, to: Option<&str>, text: &str) -> FeedEntry {
        FeedEntry::new(kind, from, to, text)
    }

    #[test]
    fn events_are_capped() {
        let mut t = Task::default();
        for i in 0..MAX_EVENTS + 25 {
            t.log(format!("e{i}"));
        }
        assert_eq!(t.events.len(), MAX_EVENTS);
        assert_eq!(t.events[0].text, "e25");
        assert_eq!(t.events.last().map(|e| e.text.as_str()), Some("e224"));
    }

    #[test]
    fn only_the_latest_feedbacks_are_kept() {
        let mut t = sample();
        for n in 0..(MAX_FEEDBACKS + 5) {
            t.add_feedback("revisão", &n.to_string());
        }
        assert_eq!(t.feedbacks.len(), MAX_FEEDBACKS);
        assert_eq!(t.feedbacks.first().map(|f| f.text.as_str()), Some("5"));
        // Tarefas gravadas antes do campo existir continuam abrindo.
        let old: Task = serde_json::from_str(r#"{"id":"t1","title":"x"}"#).unwrap();
        assert!(old.feedbacks.is_empty());
    }

    #[test]
    fn only_a_completed_or_discarded_task_is_final() {
        for (s, terminal) in [
            (TaskStatus::Draft, false),
            (TaskStatus::Running, false),
            (TaskStatus::Review, false),
            (TaskStatus::Validating, false),
            (TaskStatus::Failed, false),
            (TaskStatus::Merged, true),
            (TaskStatus::Discarded, true),
        ] {
            assert_eq!(s.is_terminal(), terminal, "{s:?}");
        }
        // Estável: o JSON antigo (`merged`) segue valendo e o novo estado tem nome e rótulo próprios.
        assert_eq!(serde_json::from_str::<TaskStatus>("\"merged\"").ok(), Some(TaskStatus::Merged));
        assert_eq!(serde_json::to_string(&TaskStatus::Validating).ok().as_deref(), Some("\"validating\""));
        assert_eq!((TaskStatus::Validating.as_str(), TaskStatus::Validating.label()), ("validating", "aguardando validação"));
        assert_eq!(TaskStatus::Merged.label(), "concluída");
    }

    #[test]
    fn the_timeline_keeps_only_the_messages_of_the_task_instance() {
        let mut t = Task { instance: "claude@t3".into(), ..sample() };
        // Pergunta feita pela tarefa, resposta recebida e erro que volta: todos são dela.
        assert!(t.record_message(&entry(FeedKind::Ask, Some("claude@t3"), Some("antigravity"), "e o css?")));
        assert!(t.record_message(&entry(FeedKind::Answer, Some("antigravity"), Some("claude@t3"), "use flex")));
        assert!(t.record_message(&entry(FeedKind::Error, Some("opencode"), Some("claude@t3"), "sem cota")));
        // Outra instância, indicador transitório, mensagem do usuário e aviso do sistema ficam de fora.
        assert!(!t.record_message(&entry(FeedKind::Ask, Some("claude"), Some("antigravity"), "outra")));
        assert!(!t.record_message(&entry(FeedKind::Thinking, Some("claude@t3"), None, "⚙️ [read_file] x")));
        assert!(!t.record_message(&entry(FeedKind::User, Some("você"), Some("claude@t3"), "oi")));
        assert!(!t.record_message(&FeedEntry::system("Tarefa t3 iniciada")));
        // `claude@t30` não é `claude@t3`.
        assert!(!t.record_message(&entry(FeedKind::Ask, Some("claude@t30"), Some("claude"), "x")));
        let texts: Vec<&str> = t.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["e o css?", "use flex", "sem cota"]);
        assert_eq!(t.messages[0].from.as_deref(), Some("claude@t3"));
        assert_eq!(t.messages[1].to.as_deref(), Some("claude@t3"));
        // Uma tarefa sem instância (rascunho) não adota mensagens sem remetente nem destinatário.
        let mut draft = sample();
        assert!(!draft.record_message(&entry(FeedKind::Answer, None, None, "x")));
    }

    #[test]
    fn the_timeline_has_a_ceiling_on_count_and_size() {
        let mut t = Task { instance: "claude@t3".into(), ..sample() };
        for i in 0..MAX_MESSAGES + 5 {
            t.record_message(&entry(FeedKind::Ask, Some("claude@t3"), Some("x"), &format!("m{i}")));
        }
        assert_eq!(t.messages.len(), MAX_MESSAGES);
        assert_eq!(t.messages[0].text, "m5", "as mais antigas saem");
        t.record_message(&entry(FeedKind::Answer, Some("x"), Some("claude@t3"), &"a".repeat(MAX_MESSAGE_CHARS * 2)));
        let last = &t.messages[t.messages.len() - 1].text;
        assert!(
            last.starts_with("aaaa") && last.contains("texto cortado") && last.chars().count() < MAX_MESSAGE_CHARS + 100,
            "{}",
            last.len()
        );
    }

    #[test]
    fn errors_map_to_http_codes() {
        let codes: Vec<u16> = [
            TaskError::Invalid("a".into()),
            TaskError::NotFound("a".into()),
            TaskError::Conflict("a".into()),
            TaskError::Failed("a".into()),
        ]
        .iter()
        .map(TaskError::http_status)
        .collect();
        assert_eq!(codes, [400, 404, 409, 500]);
    }
}

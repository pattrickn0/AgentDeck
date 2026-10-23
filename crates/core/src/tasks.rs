//! Mesa de tarefas: a unidade de trabalho do AgentDeck.
//!
//! Uma **tarefa** tem um pedido (o brief), um agente com modelo, uma worktree + branch próprias, um
//! relatório, um diff e um ciclo de vida que termina em validação (concluída) ou descarte:
//!
//! ```text
//! Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged (concluída)
//!   │                 ▲                 │ └─discard──▶ Discarded     │ └─discard──▶ Discarded
//!   │                 ├───return────────┘                            │
//!   │                 └───────────────reject─────────────────────────┘
//!   └─ start com erro ─▶ Failed ──(start de novo)──▶ Running
//! ```
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

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use unicode_normalization::UnicodeNormalization;

use crate::events::{CoreEvent, Delivery, FeedEntry};
use crate::git::{run, GitError, Repo, RepoState, COMMIT_TIMEOUT, WRITE_TIMEOUT};
use crate::instances::Ident;
use crate::util::{self, now_ms, terminal_text};
use crate::{models, Core};

/// Eventos guardados por tarefa (os mais antigos saem).
pub const MAX_EVENTS: usize = 200;
/// Feedbacks guardados por tarefa (os mais antigos saem), e o tamanho de cada um ao reaparecer numa mensagem.
pub const MAX_FEEDBACKS: usize = 20;
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
    /// Agente (`claude`, `antigravity`, `opencode`, `ollama:qwen3.5:9b`…).
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
}

impl Task {
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
        })
    }
}

/// O que se informa para criar uma tarefa ([`Core::task_create`]).
#[derive(Debug, Clone, Default)]
pub struct TaskSpec {
    pub title: String,
    /// O pedido (markdown). Pode ficar vazio: o título vai no lugar ao entregar ao agente.
    pub brief: String,
    /// Agente; vazio = `claude`. Aceita os apelidos (`agy`).
    pub kind: String,
    /// Modelo pedido; `None` = o padrão do cargo ou do CLI.
    pub model: Option<String>,
    /// Esforço pedido; `None` = o padrão do CLI. Só vale onde [`models::effort_unavailable`] não objeta.
    pub effort: Option<models::Effort>,
    /// Branch de partida e de mesclagem; `None` = a branch atual do repositório do projeto.
    pub base: Option<String>,
    /// Quem pediu (id de agente ou `usuário`).
    pub creator: Option<String>,
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
}

impl From<TaskError> for MergeRefusal {
    fn from(error: TaskError) -> Self {
        MergeRefusal { error, block: None }
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

// ---------- Funções puras ----------

/// `Tarefa  nova!` → `tarefa-nova`: ASCII minúsculo, sem acentos, no máximo `max` caracteres.
pub fn slugify(title: &str, max: usize) -> String {
    let mut out = String::new();
    for c in title.nfd() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase());
        } else if c.is_ascii() && !out.ends_with('-') {
            out.push('-');
        }
        // Não ASCII (acentos soltos pela decomposição, ideogramas…): some sem separar a palavra.
    }
    let cut: String = out.trim_matches('-').chars().take(max).collect();
    cut.trim_end_matches('-').to_owned()
}

/// `tarefa/<id>-<slug>` (sem slug, `tarefa/<id>`).
pub fn branch_name(id: &str, title: &str) -> String {
    match slugify(title, MAX_SLUG) {
        s if s.is_empty() => format!("tarefa/{id}"),
        s => format!("tarefa/{id}-{s}"),
    }
}

/// `T3`, `t3` e `3` → `t3`. `None` se não for um id de tarefa.
pub fn parse_task_id(text: &str) -> Option<String> {
    let t = text.trim().to_ascii_lowercase();
    let digits = t.strip_prefix('t').unwrap_or(&t);
    if digits.is_empty() || digits.len() > 9 || !digits.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok().map(|n| format!("t{n}"))
}

/// Título numa linha só, sem espaços sobrando e com no máximo [`MAX_TITLE`] caracteres.
pub fn clean_title(title: &str) -> Result<String, TaskError> {
    let one_line = title.split_whitespace().collect::<Vec<_>>().join(" ");
    let one_line: String = one_line.chars().filter(|c| !c.is_control()).collect();
    if one_line.is_empty() {
        return Err(TaskError::Invalid("o título da tarefa não pode ficar vazio".into()));
    }
    Ok(one_line.chars().take(MAX_TITLE).collect())
}

fn cap_text(text: &str, max: usize) -> String {
    let (head, cut) = util::truncate_chars(text, max);
    if cut {
        format!("{head}\n\n[texto cortado em {max} caracteres]")
    } else {
        head.to_owned()
    }
}

/// O bloco de memória do início da tarefa: o que o projeto já aprendeu (longo prazo) e a situação atual
/// (curto prazo). Cada parte some quando está vazia; vazio quando as duas estão.
pub fn memory_block(learnings: &str, situation: &str) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !learnings.trim().is_empty() {
        parts.push(format!("O que o projeto já aprendeu:\n{}", learnings.trim()));
    }
    if !situation.trim().is_empty() {
        parts.push(format!("Situação atual:\n{}", situation.trim()));
    }
    parts.join("\n\n")
}

/// Os feedbacks que a tarefa já recebeu, para repetir ao agente numa devolução ou reprovação (os últimos
/// [`HISTORY_SHOWN`], cada um cortado) e seguidos de uma linha em branco. Vazio quando não há nenhum.
/// `task.feedbacks` não pode já conter o feedback que está sendo entregue agora.
fn history_section(task: &Task) -> String {
    if task.feedbacks.is_empty() {
        return String::new();
    }
    let skipped = task.feedbacks.len().saturating_sub(HISTORY_SHOWN);
    let mut out = String::from("Feedbacks anteriores desta tarefa (do mais antigo ao mais novo; não repita o que já foi pedido):");
    if skipped > 0 {
        out.push_str(&format!("\n(mais {skipped} anterior(es) omitido(s))"));
    }
    for (n, f) in task.feedbacks.iter().enumerate().skip(skipped) {
        let (head, cut) = util::truncate_chars(f.text.trim(), HISTORY_ITEM_CHARS);
        out.push_str(&format!("\n{}. [{}] {}{}", n + 1, f.origin, head, if cut { "…" } else { "" }));
    }
    out.push_str("\n\n");
    out
}

/// Os aprendizados dos arquivos que o feedback cita (já prontos, de [`crate::learnings::render_specific`]) para a
/// devolução ou reprovação, seguidos de uma linha em branco. Vazio quando não há nenhum.
fn learned_section(learnings: &str) -> String {
    match learnings.trim() {
        "" => String::new(),
        l => format!("O que o projeto já aprendeu sobre os arquivos citados:\n\n{l}\n\n"),
    }
}

/// A mensagem que entrega o brief ao agente: o pedido, a memória do projeto (`memory`, de [`memory_block`]) e
/// as regras da tarefa. `boss` é quem criou a tarefa quando é um agente (as dúvidas vão a ele por
/// `ask-agent --boss`).
pub fn start_message(task: &Task, boss: Option<&str>, memory: &str) -> String {
    let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.trim() };
    let doubts = match boss {
        Some(b) => format!("Dúvidas: pergunte a quem criou a tarefa (\"{b}\") com ask-agent --boss \"sua pergunta\"."),
        None => "Dúvidas: escreva aqui no terminal; o usuário acompanha esta aba.".to_owned(),
    };
    let memory = match memory.trim() {
        "" => String::new(),
        m => format!("{m}\n\n---\n"),
    };
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title}]\n\n{brief}\n\n---\n{memory}\
Regras da tarefa {id}:\n\
- Trabalhe SOMENTE na pasta {wt}, na branch {branch} (criada a partir de {base}). Não mexa no repositório principal nem em outras pastas.\n\
- Faça commits pequenos nessa branch. Não faça merge nem push: quem mescla é o AgentDeck, depois da revisão.\n\
- Não peça a ninguém (outro agente ou o usuário) que mescle ou rode ask-agent --task merge: a decisão é de quem revisa. O seu trabalho termina no --task done.\n\
- Antes de mexer num arquivo pela primeira vez, rode ask-agent --learn <arquivo> (mostra o que o projeto já aprendeu sobre ele).\n\
- Se descobrir algo não óbvio que o próximo agente precisaria saber (armadilha, decisão, convenção), crie um arquivo em memory/aprendizados/ nesta branch (formato em memory/aprendizados/README.md) e cite-o no relatório: ele passa pela mesma revisão e mescla que o código.\n\
- Ao terminar, escreva o relatório (o que foi feito, arquivos alterados, testes rodados e o que falta) em um arquivo e rode: ask-agent --task done {id} --file <arquivo>.\n\
  (Entregar por arquivo preserva os acentos; canalizar pela entrada padrão (-) faz o shell do Windows trocá-los por \"?\".)\n\
- Para saber que agentes e modelos este computador tem, rode `ask-agent --models` antes de escolher `--agent`/`--model`.\n\
- {doubts}

Comece agora, por conta própria: este brief é a sua ordem de trabalho. Não espere confirmação nem pergunte se pode começar.",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        base = task.base,
    ))
}

/// A mensagem que devolve a tarefa ao agente com o feedback da revisão.
pub fn return_message(task: &Task, feedback: &str, learnings: &str) -> String {
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title} — ajustes pedidos na revisão]\n\n{feedback}\n\n{history}{learned}---\n\
Continue na mesma pasta ({wt}) e na branch {branch}, com commits pequenos. Ao terminar, escreva o relatório em um arquivo e rode de novo ask-agent --task done {id} --file <arquivo> \
(entregar por arquivo preserva os acentos).",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
        history = history_section(task),
        learned = learned_section(learnings),
    ))
}

/// A mensagem que devolve ao agente uma tarefa reprovada na validação: o trabalho já foi mesclado na base,
/// o usuário testou e reprovou. `aligned` = a branch da tarefa já está alinhada com a base.
pub fn reject_message(task: &Task, feedback: &str, aligned: bool, learnings: &str) -> String {
    let align = if aligned {
        String::new()
    } else {
        format!(
            " Atenção: não consegui alinhar a sua branch com {base}; antes de continuar, rode na sua pasta: git merge {base}.",
            base = task.base
        )
    };
    terminal_text(&format!(
        "[AgentDeck · Tarefa {id}: {title} — reprovada na validação]\n\n\
O seu trabalho anterior já foi mesclado em {base}, mas o usuário testou e reprovou pelo seguinte motivo:\n\n{feedback}\n\n{history}{learned}---\n\
Corrija na mesma pasta ({wt}) e na mesma branch {branch}, com commits pequenos.{align} Ao terminar, escreva o relatório em um arquivo e rode de novo ask-agent --task done {id} --file <arquivo> \
(entregar por arquivo preserva os acentos).",
        id = task.id,
        title = task.title,
        base = task.base,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
        history = history_section(task),
        learned = learned_section(learnings),
    ))
}

/// A linha curta (ASCII) que abre o agente de uma tarefa no lugar do texto completo: aspas, quebras de linha e
/// acentos na linha de comando se perdem nos shims `.cmd` do Windows, então o texto vai num arquivo e a linha só
/// manda lê-lo. `intro` diz o que aconteceu (`o pedido esta no arquivo`, `foi devolvida na revisao`…).
/// `None` se o caminho não é ASCII (a linha de comando não o carregaria inteiro).
pub fn boot_line(task: &Task, intro: &str, file: &Path) -> Option<String> {
    let line = format!(
        "Tarefa {}: {intro}. Leia o arquivo {} e comece agora, por conta propria, sem esperar confirmacao.",
        task.id,
        file.display()
    );
    (line.is_ascii() && !line.contains(['\n', '\r', '"'])).then_some(line)
}

/// Caminhos sujos de um `git status --porcelain=v1` (o destino, nas renomeações).
pub fn dirty_paths(porcelain: &str) -> Vec<String> {
    porcelain
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| {
            let path = &l[3..];
            let path = path.rsplit_once(" -> ").map_or(path, |(_, new)| new);
            path.trim_matches('"').to_owned()
        })
        .collect()
}

/// O novo conteúdo de `info/exclude` com `/.agentdeck/`, ou `None` se já está lá.
pub fn exclude_with_agentdeck(existing: &str) -> Option<String> {
    if existing.lines().map(str::trim).any(|l| matches!(l, "/.agentdeck/" | "/.agentdeck" | ".agentdeck/" | ".agentdeck")) {
        return None;
    }
    let mut text = existing.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# AgentDeck: worktrees das tarefas\n/.agentdeck/\n");
    Some(text)
}

/// Lista curta de caminhos para mensagens (`a, b, c e mais 2`).
fn short_list(paths: &[String]) -> String {
    const SHOWN: usize = 5;
    let mut out = paths.iter().take(SHOWN).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > SHOWN {
        out.push_str(&format!(" e mais {}", paths.len() - SHOWN));
    }
    out
}

// ---------- Armazenamento ----------

/// Tarefas de um projeto como ficam em disco.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct ProjectTasks {
    version: u32,
    /// Próximo número a usar (nunca diminui).
    next: u64,
    tasks: Vec<Task>,
}

impl ProjectTasks {
    fn task_mut(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    /// O contador nunca fica abaixo do maior número já usado (arquivo editado à mão ou antigo).
    fn fix_counter(&mut self) {
        let max = self.tasks.iter().filter_map(|t| parse_task_id(&t.id)).filter_map(|id| id[1..].parse::<u64>().ok()).max().unwrap_or(0);
        self.next = self.next.max(max + 1).max(1);
        self.version = 1;
    }
}

/// Estado das tarefas no core: os projetos já lidos e a trava das operações do ciclo de vida.
#[derive(Default)]
pub(crate) struct TaskStore {
    projects: Mutex<HashMap<String, ProjectTasks>>,
    /// Uma operação do ciclo de vida por vez (criar, iniciar, mesclar…): cada uma pode rodar o git.
    op: Mutex<()>,
    /// Prompt inicial à espera da abertura do processo (`projeto/instância` → linha), ver `Core::set_boot_prompt`.
    boot: Mutex<HashMap<String, String>>,
}

fn load_file(path: &Path) -> ProjectTasks {
    let mut tasks = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str::<ProjectTasks>(&text).unwrap_or_else(|e| {
            // Ilegível: guarda uma cópia antes que a próxima gravação ponha uma lista vazia por cima.
            let backup = path.with_extension(format!("json.ilegivel-{}", now_ms()));
            match std::fs::copy(path, &backup) {
                Ok(_) => log::error!("tarefas ilegíveis em {} ({e}); cópia guardada em {}", path.display(), backup.display()),
                Err(e2) => log::error!("tarefas ilegíveis em {} ({e}; e a cópia falhou: {e2})", path.display()),
            }
            ProjectTasks::default()
        }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => ProjectTasks::default(),
        Err(e) => {
            log::error!("não consegui ler as tarefas em {}: {e}", path.display());
            ProjectTasks::default()
        }
    };
    tasks.fix_counter();
    tasks
}

/// Grava ao lado e troca: uma queda no meio não deixa o arquivo pela metade.
fn save_file(path: &Path, tasks: &ProjectTasks) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(tasks).map_err(std::io::Error::other)?;
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

/// Id de projeto que serve de nome de arquivo (`p` + hexadecimal, na prática).
fn safe_project_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

impl Core {
    fn tasks_path(&self, project: &str) -> Option<PathBuf> {
        safe_project_id(project).then(|| self.paths().data_dir.join("tarefas").join(format!("{project}.json")))
    }

    /// Aplica `f` às tarefas do projeto (lidas do disco na primeira vez). Com `save`, grava e avisa a
    /// interface (`TasksChanged`) no fim.
    fn with_tasks<R>(&self, project: &str, save: bool, f: impl FnOnce(&mut ProjectTasks) -> R) -> R {
        let path = self.tasks_path(project);
        let out = {
            let mut all = self.inner.tasks.projects.lock();
            let Some(path) = &path else {
                // Id estranho (não é nome de arquivo): nada é lido nem gravado.
                return f(&mut ProjectTasks::default());
            };
            let entry = all.entry(project.to_owned()).or_insert_with(|| load_file(path));
            let out = f(entry);
            if save {
                if let Err(e) = save_file(path, entry) {
                    log::warn!("não consegui gravar as tarefas em {}: {e}", path.display());
                }
            }
            out
        };
        if save {
            self.emit(CoreEvent::TasksChanged { project: project.to_owned() });
        }
        out
    }

    /// As tarefas do projeto, da mais antiga para a mais nova.
    pub fn tasks(&self, project: &str) -> Vec<Task> {
        self.with_tasks(project, false, |p| p.tasks.clone())
    }

    /// Uma tarefa do projeto (`t3`, `T3` ou `3`).
    pub fn task(&self, project: &str, id: &str) -> Option<Task> {
        let id = parse_task_id(id)?;
        self.with_tasks(project, false, |p| p.tasks.iter().find(|t| t.id == id).cloned())
    }

    /// A tarefa de que a instância `instance` é o agente (e que ainda não terminou), se houver.
    pub fn task_of_instance(&self, project: &str, instance: &str) -> Option<Task> {
        self.with_tasks(project, false, |p| {
            p.tasks.iter().find(|t| t.instance == instance && !t.status.is_terminal() && t.status != TaskStatus::Draft).cloned()
        })
    }

    /// A pasta em que o terminal da instância deve abrir: a worktree da tarefa em andamento, em revisão ou
    /// aguardando validação. Uma tarefa `Failed` que já entregou o relatório (a mesclagem falhou) também
    /// segue na worktree: reabrir o app não pode soltar o agente no repositório principal.
    pub(crate) fn task_cwd(&self, project: &str, instance: &str) -> Option<PathBuf> {
        self.with_tasks(project, false, |p| {
            p.tasks
                .iter()
                .find(|t| {
                    t.instance == instance
                        && match t.status {
                            TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating => true,
                            TaskStatus::Failed => t.report.is_some(),
                            _ => false,
                        }
                })
                .map(|t| t.worktree.clone())
        })
    }

    fn task_or_404(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.task(project, id).ok_or_else(|| TaskError::NotFound(format!("tarefa \"{}\" não encontrada neste projeto", id.escape_debug())))
    }

    /// Altera uma tarefa existente e grava. `None` se ela sumiu.
    fn update_task<R>(&self, project: &str, id: &str, f: impl FnOnce(&mut Task) -> R) -> Option<R> {
        self.with_tasks(project, true, |p| p.task_mut(id).map(f))
    }

    /// Marca a tarefa como `Failed` com a mensagem e devolve o erro para quem chamou.
    fn fail_task(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Failed;
            t.error = Some(message.clone());
            t.log(format!("erro: {message}"));
        });
        TaskError::Failed(message)
    }

    /// A mesclagem falhou por motivo externo (não conflito): o erro vai para o histórico e para quem chamou, e a
    /// tarefa fica (ou volta) em `Review`, onde ainda se pode mesclar de novo, devolver ou descartar. `Failed` aqui
    /// seria um beco: a mesa recusava devolver e concluir.
    fn fail_merge(&self, project: &str, id: &str, message: String) -> TaskError {
        self.update_task(project, id, |t| {
            t.status = TaskStatus::Review;
            t.error = None;
            t.log(format!("erro: {message}"));
        });
        TaskError::Failed(message)
    }

    // ---------- Git ----------

    /// O repositório do projeto (a raiz dele, mesmo que o projeto seja uma subpasta).
    fn task_repo(&self, project: &str) -> Result<Repo, TaskError> {
        let path = self.project_path(project).ok_or_else(|| TaskError::NotFound(format!("projeto desconhecido: {project}")))?;
        self.git_repo_at(&path).map_err(|e| match e {
            GitError::NotARepo => {
                TaskError::Invalid("o projeto não é um repositório git; as tarefas precisam de um (rode git init)".into())
            }
            other => git_error(other),
        })
    }

    // ---------- Criar ----------

    /// Cria uma tarefa em rascunho. Confere o agente, o modelo e o repositório (a branch atual é a
    /// base, se nenhuma for pedida). Roda o git: chame fora da thread da interface.
    pub fn task_create(&self, project: &str, spec: TaskSpec) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let title = clean_title(&spec.title)?;
        let brief = cap_text(spec.brief.trim(), MAX_TEXT);

        // Agente: instalado, com aba (nada de `shell`) e sem `@` (o cargo é o id da tarefa).
        let asked = match spec.kind.trim().to_lowercase() {
            k if k.is_empty() => "claude".to_owned(),
            k => k,
        };
        if asked.contains('@') {
            return Err(TaskError::Invalid(format!(
                "agente inválido \"{}\": informe só o modelo (claude, antigravity…)",
                asked.escape_debug()
            )));
        }
        let kinds = self.inner.agents.peers();
        let ollama = self.inner.agents.resolve_ollama(None).map(|o| o.models).unwrap_or_default();
        let kind = crate::bridge::resolve_target(&asked, &kinds, &ollama);
        if !kinds.contains(&kind) || Ident::parse(&kind).is_none() {
            return Err(TaskError::Invalid(format!(
                "agente desconhecido ou não instalado \"{}\". Disponíveis: {}",
                asked.escape_debug(),
                kinds.join(", ")
            )));
        }
        // Modelo: o pedido (validado) ou o padrão do cargo; só quem aceita escolher.
        let requested = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        if let Some(m) = requested {
            if !models::valid_model(m) {
                return Err(TaskError::Invalid(format!("modelo inválido: \"{}\"", m.chars().take(40).collect::<String>().escape_debug())));
            }
            if !models::accepts_model(&kind) {
                return Err(TaskError::Invalid(format!("o agente {kind} não aceita escolher modelo")));
            }
        }
        let model = models::resolve_model(&Ident::main(&kind), requested, None, &self.config().role_models);
        // Esforço: só onde o CLI o aceita. No Antigravity ele troca o sufixo do modelo, então o modelo gravado
        // já é o que vai abrir.
        let (model, effort) = match spec.effort {
            Some(e) => match models::effort_unavailable(&kind, model.as_deref()) {
                Some(why) => return Err(TaskError::Invalid(format!("esforço não aplicável a {kind}: {why}"))),
                None => (model.map(|m| models::effort_model(&kind, &m, e)), Some(e)),
            },
            None => (model, None),
        };

        // Repositório e base.
        let repo = self.task_repo(project)?;
        if repo.read(&["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
            return Err(TaskError::Invalid(
                "o repositório ainda não tem nenhum commit; faça o primeiro commit antes de criar tarefas".into(),
            ));
        }
        let base = match spec.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => b.to_owned(),
            None => match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
                Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                Err(_) => return Err(TaskError::Invalid("o repositório está sem branch (HEAD solto); informe a base da tarefa".into())),
            },
        };
        if !crate::git::valid_branch_name(&base) || repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).is_err() {
            return Err(TaskError::Invalid(format!("a base \"{}\" não é uma branch local deste repositório", base.escape_debug())));
        }

        let number = self.with_tasks(project, false, |p| p.next.max(1));
        let id = format!("t{number}");
        let instance = Ident::parse(&format!("{kind}@{id}"))
            .map(|i| i.id())
            .ok_or_else(|| TaskError::Invalid(format!("agente \"{kind}\" não pode ter instância de tarefa")))?;
        let mut task = Task {
            branch: branch_name(&id, &title),
            worktree: util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees").join(&id)),
            id: id.clone(),
            title,
            brief,
            status: TaskStatus::Draft,
            base,
            kind,
            model,
            effort,
            instance,
            creator: spec.creator.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()),
            created_ms: now_ms(),
            ..Task::default()
        };
        task.log(match &task.creator {
            Some(c) => format!("criada por {c}"),
            None => "criada".to_owned(),
        });
        self.with_tasks(project, true, |p| {
            p.next = number + 1;
            p.tasks.push(task.clone());
        });
        Ok(task)
    }

    // ---------- Iniciar ----------

    /// Inicia a tarefa: worktree + branch, instância do agente com o terminal na worktree e o brief
    /// entregue à aba. Vale para `Draft` e para `Failed` sem relatório (nova tentativa). Só com as abas dos
    /// agentes ligadas (tarefas não rodam em headless). Roda o git: fora da thread da interface.
    pub fn task_start(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_none();
        if task.status != TaskStatus::Draft && !retry {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se inicia um rascunho (ou uma tarefa que falhou ao iniciar)",
                task.id,
                task.status.label()
            )));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações (no modo headless não há aba nem worktree por agente)".into(),
            ));
        }
        if let Err(e) = self.prepare_worktree(project, &task, retry) {
            return Err(self.fail_task(project, &task.id, e.message().to_owned()));
        }
        let now = now_ms();
        let started = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.started_ms = Some(now);
            t.finished_ms = None;
            t.error = None;
            t.log(format!("iniciada: worktree {} na branch {}", t.worktree.display(), t.branch));
            t.clone()
        });
        let Some(task) = started else { return Err(TaskError::NotFound(format!("tarefa {id} sumiu"))) };
        let ident = self.open_task_instance(project, &task);
        let boss = self.task_boss(project, &task);
        let prompt = start_message(&task, boss.as_deref(), &self.task_memory(project, &task));
        let boot = self.task_boot_prompt(project, &task, "brief", "o pedido completo esta no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "brief entregue ao agente");
        self.feed(FeedEntry::system(format!("Tarefa {} iniciada: {} ({})", task.id, task.title, task.instance)).in_project(project));
        Ok(task)
    }

    /// O bloco de memória do início da tarefa ([`memory_block`]): os aprendizados que valem para o pedido (lidos
    /// da worktree, que já tem a base) e a situação atual. Qualquer falha só deixa a parte de fora: a tarefa
    /// nunca deixa de começar por causa da memória.
    fn task_memory(&self, project: &str, task: &Task) -> String {
        let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.as_str() };
        let cited = crate::learnings::paths_in_text(brief, &task.worktree);
        let learnings = crate::learnings::render(&crate::learnings::load(&task.worktree), &cited, crate::learnings::START_BUDGET);
        memory_block(&learnings, &self.task_situation(project, task, &cited))
    }

    /// Os aprendizados específicos dos arquivos que o `feedback` novo cita e que o agente ainda não viu (nem no
    /// pedido, nem nos feedbacks anteriores de `before`), lidos da worktree. O índice e os globais ele já recebeu
    /// no início da tarefa. Vazio se não há arquivo novo ou aprendizado que case.
    fn feedback_learnings(&self, before: &Task, feedback: &str) -> String {
        let brief = if before.brief.trim().is_empty() { before.title.as_str() } else { before.brief.as_str() };
        let seen: Vec<String> = std::iter::once(brief)
            .chain(before.feedbacks.iter().map(|f| f.text.as_str()))
            .flat_map(|text| crate::learnings::paths_in_text(text, &before.worktree))
            .collect();
        let fresh: Vec<String> =
            crate::learnings::paths_in_text(feedback, &before.worktree).into_iter().filter(|p| !seen.contains(p)).collect();
        if fresh.is_empty() {
            return String::new();
        }
        crate::learnings::render_specific(&crate::learnings::load(&before.worktree), &fresh, crate::learnings::FEEDBACK_BUDGET)
    }

    /// A "situação atual" ([`crate::situation::render`]): as outras tarefas em andamento do projeto, com os
    /// arquivos que a branch delas já mexeu, e os últimos commits da base. Vazia se não há repositório.
    fn task_situation(&self, project: &str, task: &Task, cited: &[String]) -> String {
        let Ok(repo) = self.task_repo(project) else { return String::new() };
        let others: Vec<crate::situation::OtherTask> = self
            .tasks(project)
            .into_iter()
            .filter(|t| t.id != task.id && matches!(t.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Validating))
            .map(|t| {
                // `Validating` já está na base (a branch foi alinhada): não há diff a mostrar.
                // Em andamento, junta o que já está na branch com o que ainda não foi commitado.
                let files = match t.status {
                    TaskStatus::Validating => Vec::new(),
                    _ => union_files(changed_files(&repo, &t.base, &t.branch), worktree_dirty_unlocked(&repo, &t.worktree)),
                };
                crate::situation::OtherTask { id: t.id, title: t.title, instance: t.instance, status: t.status.label().to_owned(), files }
            })
            .collect();
        let commits: Vec<String> = repo
            .read(&["log", "--oneline", "-8", &task.base, "--"])
            .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::to_owned).collect())
            .unwrap_or_default();
        crate::situation::render(&others, &commits, &task.base, cited, crate::situation::BUDGET)
    }

    /// Worktree + branch da tarefa: exclui `/.agentdeck/` do git, limpa o que sobrou de uma tentativa
    /// anterior e cria a worktree a partir da base.
    fn prepare_worktree(&self, project: &str, task: &Task, retry: bool) -> Result<(), TaskError> {
        let repo = self.task_repo(project)?;
        self.ensure_exclude(&repo)?;
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !safe_worktree(&task.worktree, &expected) || !valid_task_branch(&task.branch) {
            return Err(TaskError::Failed(format!(
                "dados da tarefa {} fora do esperado (pasta {} / branch {}); não mexi em nada",
                task.id,
                task.worktree.display(),
                task.branch
            )));
        }
        if retry {
            let _ = self.remove_task_worktree(&repo, task);
            let _ = delete_branch(&repo, &task.branch, true);
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.base)]).is_err() {
            return Err(TaskError::Failed(format!("a base \"{}\" não existe mais neste repositório", task.base)));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_ok() {
            return Err(TaskError::Failed(format!("a branch {} já existe neste repositório; remova-a ou descarte a tarefa", task.branch)));
        }
        if task.worktree.exists() {
            return Err(TaskError::Failed(format!("a pasta {} já existe; remova-a ou descarte a tarefa", task.worktree.display())));
        }
        let path = task.worktree.to_string_lossy().into_owned();
        run(repo.cmd(&["worktree", "add", "-b", &task.branch, &path, &task.base]), None, COMMIT_TIMEOUT)
            .map_err(|e| TaskError::Failed(format!("não consegui criar a worktree: {e}")))?;
        Ok(())
    }

    /// Escreve `/.agentdeck/` em `<git comum>/info/exclude`, uma vez (nunca no `.gitignore`).
    fn ensure_exclude(&self, repo: &Repo) -> Result<(), TaskError> {
        let common = repo
            .read(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| repo.git_dir.clone());
        let file = common.join("info").join("exclude");
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        if let Some(text) = exclude_with_agentdeck(&existing) {
            std::fs::create_dir_all(common.join("info"))
                .and_then(|_| std::fs::write(&file, text))
                .map_err(|e| TaskError::Failed(format!("não consegui escrever {}: {e}", file.display())))?;
        }
        Ok(())
    }

    /// Registra a instância da tarefa (modelo, chefe) e a devolve; idempotente.
    fn open_task_instance(&self, project: &str, task: &Task) -> Ident {
        let ident = Ident::parse(&task.instance).unwrap_or_else(|| Ident::main(&task.kind));
        self.with_instances(project, |list| list.register(ident.clone()));
        self.assign_model(project, &ident, task.model.as_deref());
        if let Some(boss) = self.task_boss(project, task) {
            if let Err(e) = self.set_boss(project, &task.instance, Some(&boss)) {
                log::warn!("chefe da tarefa {}: {e}", task.id);
            }
        }
        ident
    }

    /// Quem criou a tarefa, quando é um agente com aba aberta (o usuário não é chefe de ninguém).
    fn task_boss(&self, project: &str, task: &Task) -> Option<String> {
        let creator = task.creator.as_deref()?;
        (creator != task.instance && self.project_instance_ids(project).iter().any(|o| o == creator)).then(|| creator.to_owned())
    }

    /// Prompt inicial para a abertura do processo da tarefa: grava `prompt` em `dados/tarefas/<projeto>-<id>-<stem>.md`
    /// e devolve a linha curta que manda o agente lê-lo. `None` se o CLI não aceita prompt inicial, se o arquivo
    /// não pôde ser gravado ou se a linha não cabe em ASCII: a entrega fica digitada, como sempre.
    fn task_boot_prompt(&self, project: &str, task: &Task, stem: &str, intro: &str, prompt: &str) -> Option<String> {
        models::boot_prompt_flag(Ident::parse(&task.instance)?.kind.as_str())?;
        let dir = self.paths().data_dir.join("tarefas");
        let file = util::absolutize(&dir.join(format!("{project}-{}-{stem}.md", task.id)));
        let file = util::strip_verbatim(&file);
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, prompt)) {
            Ok(()) => boot_line(task, intro, &file),
            Err(e) => {
                log::warn!("não consegui gravar {}: {e}", file.display());
                None
            }
        }
    }

    /// Guarda o prompt inicial da próxima abertura do processo de `instance`: `agents::spawn_spec_for` o leva
    /// na linha de comando (uma vez). A interface chama antes de abrir ou reabrir a aba.
    pub fn set_boot_prompt(&self, project: &str, instance: &str, line: String) {
        self.inner.tasks.boot.lock().insert(format!("{project}/{instance}"), line);
    }

    /// Pega (e esquece) o prompt inicial guardado para `instance`.
    pub fn take_boot_prompt(&self, project: &str, instance: &str) -> Option<String> {
        self.inner.tasks.boot.lock().remove(&format!("{project}/{instance}"))
    }

    /// Manda `prompt` à aba da tarefa (abre a aba se preciso) pelo caminho das mensagens entre agentes. Se a
    /// aba precisar ser aberta e o CLI aceitar um prompt inicial, a interface usa `boot` na linha de comando em
    /// vez de digitar `prompt` (ver [`Delivery::boot`]).
    fn deliver_to_task(&self, project: &str, task: &Task, ident: Ident, prompt: String, boot: Option<String>, log: &str) {
        let from = task.creator.clone().unwrap_or_else(|| "usuário".to_owned());
        self.emit(CoreEvent::Deliver(Delivery {
            request: 0,
            project: project.to_owned(),
            ident,
            create: true,
            from,
            prompt,
            answer: false,
            boot,
        }));
        self.update_task(project, &task.id, |t| t.log(log));
    }

    // ---------- Relatório ----------

    /// O agente terminou: guarda o relatório e o `git diff --stat base...branch` e põe a tarefa em
    /// revisão. Exige `Running`, ou `Validating` com commits novos na branch (o usuário corrigiu algo na aba
    /// enquanto esperava a validação e o agente entrega o que falta; sem commits novos, recusa). A ponte só
    /// deixa o próprio agente da tarefa chamar.
    pub fn task_done(&self, project: &str, id: &str, report: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let after_merge = task.status == TaskStatus::Validating;
        if task.status != TaskStatus::Running && !after_merge {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só uma tarefa em andamento recebe o relatório",
                task.id,
                task.status.label()
            )));
        }
        if after_merge {
            let repo = self.task_repo(project)?;
            let has_new = valid_task_branch(&task.branch)
                && crate::git::valid_branch_name(&task.base)
                && branch_exists(&repo, &task.branch)
                && git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)])
                    .map(|n| n.trim() != "0")
                    .unwrap_or(false);
            if !has_new {
                return Err(TaskError::Conflict(format!(
                    "a tarefa {} já foi mesclada e espera a validação do usuário; não há commits novos para entregar. Se algo ainda falta, quem decide é o usuário (Reprovar)",
                    task.id
                )));
            }
        }
        let report = report.trim();
        if report.is_empty() {
            return Err(TaskError::Invalid("o relatório está vazio: diga o que foi feito, os arquivos, os testes e o que falta".into()));
        }
        let report = cap_text(report, MAX_TEXT);
        let (stat, warning) = match self.task_repo(project) {
            Ok(repo) => (
                diff_stat(&repo, &task.base, &task.branch),
                worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()).map(|d| {
                    format!(
                        "atenção: há alterações sem commit na worktree ({}); o agente precisa fazer o commit antes da mesclagem",
                        short_list(&d)
                    )
                }),
            ),
            Err(e) => (Err(e.message().to_owned()), None),
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.report = Some(report);
            match &stat {
                Ok(s) => t.diff_stat = Some(s.clone()),
                Err(e) => t.log(format!("não consegui o diff: {e}")),
            }
            if let Some(w) = &warning {
                t.log(w.clone());
            }
            t.status = TaskStatus::Review;
            t.finished_ms = Some(now_ms());
            t.log(if after_merge {
                "o agente entregou mais trabalho depois da mesclagem; em revisão"
            } else {
                "relatório recebido; em revisão"
            });
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.feed(FeedEntry::system(format!("Tarefa {} pronta para revisão: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Diff ----------

    /// O diff completo da tarefa (`git diff base...branch`, só o que a branch trouxe), no mesmo formato
    /// do diff da aba Git, para a interface mostrar. Em `Validating` e `Merged` com `merge_commit`, é o que a
    /// mesclagem trouxe (`git diff <hash>^1 <hash>`): depois do alinhamento a branch é a própria base e o
    /// diff por intervalo ficaria vazio. Só leitura: não toma a trava das operações. Falha com `Conflict` se a
    /// branch já não existe (tarefa descartada) ou se o commit da mesclagem sumiu (histórico reescrito).
    pub fn task_diff(&self, project: &str, id: &str) -> Result<crate::git::GitDiff, TaskError> {
        let task = self.task_or_404(project, id)?;
        let repo = self.task_repo(project)?;
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não li o diff", task.id)));
        }
        let merged = matches!(task.status, TaskStatus::Validating | TaskStatus::Merged);
        let args: Vec<String> = if let Some(hash) = task.merge_commit.as_deref().filter(|_| merged) {
            let well_formed = !hash.is_empty() && hash.len() <= 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            if !well_formed || repo.read(&["rev-parse", "--verify", "-q", &format!("{hash}^{{commit}}")]).is_err() {
                return Err(TaskError::Conflict(format!(
                    "o commit da mesclagem ({}) não existe mais no repositório (o histórico foi reescrito?): não há diff para mostrar",
                    short_hash(hash)
                )));
            }
            let first_parent = format!("{hash}^1");
            ["diff", "--no-color", "--no-ext-diff", "-M", first_parent.as_str(), hash, "--"].map(str::to_owned).to_vec()
        } else {
            if !branch_exists(&repo, &task.branch) {
                return Err(TaskError::Conflict(format!("a branch {} não existe mais: não há diff para mostrar", task.branch)));
            }
            let range = format!("{}...{}", task.base, task.branch);
            ["diff", "--no-color", "--no-ext-diff", "-M", range.as_str(), "--"].map(str::to_owned).to_vec()
        };
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        let out = run(repo.cmd(&args), None, WRITE_TIMEOUT).map_err(git_error)?;
        let cut = out.stdout.len() > crate::git::DIFF_MAX_BYTES;
        let bytes = &out.stdout[..out.stdout.len().min(crate::git::DIFF_MAX_BYTES)];
        let mut diff = crate::git::parse_diff(&String::from_utf8_lossy(bytes), crate::git::DIFF_MAX_LINES);
        diff.truncated |= cut;
        Ok(diff)
    }

    // ---------- Devolver ----------

    /// Revisão pediu ajustes: `Review` (ou `Failed` com relatório, de uma mesclagem que falhou antes) volta a
    /// `Running` e o feedback é entregue ao agente (a aba é reaberta se foi fechada).
    pub fn task_return(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        // `Failed` com relatório é uma mesclagem que falhou numa versão anterior (hoje fica em `Review`).
        if task.status != TaskStatus::Review && !(task.status == TaskStatus::Failed && task.report.is_some()) {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se devolve uma tarefa em revisão",
                task.id,
                task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid("o feedback está vazio: diga o que precisa mudar".into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações".into(),
            ));
        }
        let feedback = cap_text(feedback, MAX_TEXT);
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.error = None;
            t.log("devolvida ao agente com feedback");
            // `before` não tem o feedback novo: a mensagem mostra só os anteriores.
            let before = t.clone();
            t.add_feedback("revisão", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        let ident = self.open_task_instance(project, &task);
        let prompt = return_message(&before, &feedback, &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "foi devolvida na revisao com ajustes, descritos no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "feedback entregue ao agente");
        Ok(task)
    }

    // ---------- Mesclar ----------

    /// Mescla a branch da tarefa na base (`git merge --no-ff`) no repositório principal e põe a tarefa em
    /// `Validating`: a aba, a worktree e a branch ficam (o usuário ainda vai testar) e a branch da tarefa é
    /// alinhada com a base já mesclada (`git merge --ff-only`). Exige `Review` (ou `Failed` com relatório,
    /// para tentar de novo). Recusa, sem mudar nada, se o repositório principal não está na base ou tem alterações
    /// sem commit em arquivos rastreados (o `AGENTS.md`, que o app reescreve, não conta), ou se a worktree
    /// da tarefa tem trabalho sem commit. Em conflito, desfaz a mesclagem (`merge --abort`) e a tarefa continua em
    /// `Review`, com os arquivos em conflito na mensagem. Se o git falha por outro motivo, a tarefa também fica em
    /// `Review`, com o erro no histórico e devolvido a quem chamou. Se o `AGENTS.md` da pasta principal só difere
    /// pelo bloco do app e a branch o altera, ele volta ao do HEAD durante a mesclagem e o bloco é regravado depois.
    pub fn task_merge(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        self.merge_task(project, id, false).map(|o| o.task).map_err(|r| r.error)
    }

    /// Como [`Core::task_merge`], para a interface: a recusa que tem saída vem com o motivo estruturado
    /// ([`MergeRefusal::block`], para a tela oferecer o botão), e `stash_main` liga o "guardar minhas alterações,
    /// mesclar e devolvê-las": as alterações sem commit do repositório principal vão para um `git stash`
    /// (identificado pela tarefa) e voltam (`stash pop`) depois da mesclagem, qualquer que seja o desfecho. Se
    /// não voltam limpas (conflito com o que a tarefa trouxe), ficam no stash, intactas, e o aviso diz como
    /// recuperá-las. Só o clique do usuário liga isso: a linha de comando (`task_merge`) nunca guarda nada.
    pub fn task_merge_assisted(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        self.merge_task(project, id, stash_main)
    }

    fn merge_task(&self, project: &str, id: &str, stash_main: bool) -> Result<MergeOutcome, MergeRefusal> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_some();
        if task.status != TaskStatus::Review && !retry {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se mescla uma tarefa em revisão",
                task.id,
                task.status.label()
            ))
            .into());
        }
        let repo = self.task_repo(project)?;
        let refuse_with = |t: &Task, msg: String, block: Option<MergeBlock>| -> MergeRefusal {
            self.update_task(project, &t.id, |t| t.log(format!("mesclagem recusada: {msg}")));
            MergeRefusal { error: TaskError::Conflict(msg), block }
        };
        let refuse = |t: &Task, msg: String| -> MergeRefusal { refuse_with(t, msg, None) };
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não mesclei nada", task.id)).into());
        }
        let _git = self.inner.git.op.lock();

        // O repositório principal precisa estar pronto para receber a mesclagem.
        match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
            Ok(out) if String::from_utf8_lossy(&out.stdout).trim() == task.base => {}
            Ok(out) => {
                let now = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                return Err(refuse(
                    &task,
                    format!(
                        "o repositório principal está na branch \"{now}\", não em \"{base}\": troque para \"{base}\" e tente de novo",
                        base = task.base
                    ),
                ));
            }
            Err(_) => {
                return Err(refuse(
                    &task,
                    format!("o repositório principal está com o HEAD solto: troque para \"{}\" e tente de novo", task.base),
                ))
            }
        }
        if repo.state() != RepoState::Normal {
            return Err(refuse(
                &task,
                "o repositório principal está no meio de outra operação (mesclagem, rebase…): termine ou aborte antes".into(),
            ));
        }
        let status = git_out(&repo, &["status", "--porcelain=v1", "--untracked-files=no"]).map_err(git_error)?;
        // O app reescreve o bloco do AGENTS.md; o git protege o arquivo se a mesclagem o tocar.
        let dirty: Vec<String> = dirty_paths(&status).into_iter().filter(|p| p != "AGENTS.md").collect();
        if !dirty.is_empty() && !stash_main {
            return Err(refuse_with(
                &task,
                format!(
                    "o repositório principal tem alterações sem commit ({}): faça o commit ou guarde-as e tente de novo",
                    short_list(&dirty)
                ),
                Some(MergeBlock::MainDirty(dirty)),
            ));
        }
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse_with(
                &task,
                format!(
                    "a worktree da tarefa tem trabalho sem commit ({}): devolva a tarefa pedindo o commit, ou descarte-a",
                    short_list(&d)
                ),
                Some(MergeBlock::TaskDirty(d)),
            ));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_err() {
            return Err(refuse(&task, format!("a branch {} não existe mais: descarte a tarefa", task.branch)));
        }
        let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
        if ahead.trim() == "0" {
            // Reprovada e entregue sem commit novo: o agente concluiu que não havia o que mudar. O que a
            // última mesclagem trouxe já está na base; volta a esperar a validação, sem novo commit de mesclagem.
            if let Some(last) = task.merge_commit.as_deref() {
                let short = short_hash(last).to_owned();
                let updated = self.update_task(project, &task.id, |t| {
                    t.status = TaskStatus::Validating;
                    t.finished_ms = Some(now_ms());
                    t.error = None;
                    t.log(format!("sem commits novos desde a última mesclagem ({short}); aguardando validação de novo"));
                    t.clone()
                });
                let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
                self.feed(
                    FeedEntry::system(format!("Tarefa {} sem commits novos, aguardando validação de novo: {}", task.id, task.title))
                        .in_project(project),
                );
                return Ok(MergeOutcome { task, note: None });
            }
            return Err(refuse(
                &task,
                format!(
                    "a branch {} não tem commits novos em relação a \"{}\": não há o que mesclar (descarte a tarefa)",
                    task.branch, task.base
                ),
            ));
        }
        if let Err(e) = repo.check_gpg_ready() {
            return Err(refuse(&task, e.to_string()));
        }
        // Arquivo novo (não rastreado) do usuário que a branch também cria: o git recusaria no meio da mesclagem. Vale
        // com ou sem "guardar": o stash não leva arquivos não rastreados, e aqui ainda não se guardou nada.
        let clash = untracked_clash(&repo, &task.base, &task.branch);
        if !clash.is_empty() {
            return Err(refuse(
                &task,
                format!(
                    "o repositório principal tem arquivos novos seus, sem commit, que a tarefa também cria ({}): mova-os para outra pasta (ou apague-os) e tente de novo",
                    short_list(&clash)
                ),
            ));
        }

        // "Guardar minhas alterações": o que está sem commit vai para o stash (menos o AGENTS.md, que o app
        // trata à parte, abaixo). Daqui em diante todo desfecho passa por `give_back`, que devolve o stash.
        let mut stashed: Option<Stash> = None;
        if stash_main && !dirty.is_empty() {
            match stash_push(&repo, &task) {
                Ok(s) => stashed = s,
                Err(e) => {
                    return Err(refuse(
                        &task,
                        format!("não consegui guardar as alterações do repositório principal ({e}); nada foi mudado"),
                    ))
                }
            }
            if stashed.is_some() {
                self.update_task(project, &task.id, |t| {
                    t.log(format!("guardei {} arquivo(s) sem commit do repositório principal no stash", dirty.len()))
                });
            }
        }
        let give_back = || -> Option<String> {
            let stash = stashed.as_ref()?;
            let (note, kept) = stash_pop(&repo, stash);
            self.update_task(project, &task.id, |t| {
                t.log(format!("alterações guardadas: {}", if kept { "continuam no stash" } else { "devolvidas" }))
            });
            Some(note)
        };

        // O app reescreve o bloco do AGENTS.md da pasta principal, e o git recusa mesclar sobre um arquivo
        // alterado que a branch também mexe. Se a alteração é só o bloco do app, o arquivo volta ao do HEAD
        // para a mesclagem e o bloco é regravado depois (com sucesso, conflito desfeito ou falha). Se o usuário
        // editou o texto fora do bloco, nada é descartado: recusa.
        let mut app_block: Option<String> = None;
        let agents_dirty = dirty_paths(&status).iter().any(|p| p == "AGENTS.md");
        if agents_dirty && changed_files(&repo, &task.base, &task.branch).iter().any(|f| f == "AGENTS.md") {
            let file = repo.root.join("AGENTS.md");
            let current = std::fs::read_to_string(&file).unwrap_or_default();
            let committed = git_out(&repo, &["show", "HEAD:AGENTS.md"]).unwrap_or_default();
            if !crate::workspace::only_marker_blocks_differ(&committed, &current) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(
                        format!(
                            "o AGENTS.md do repositório principal tem alterações suas sem commit, e a branch {} também mexe nele: faça o commit ou guarde-as e tente de novo",
                            task.branch
                        ),
                        note,
                    ),
                ));
            }
            if let Err(e) = repo.write(&["checkout", "--", "AGENTS.md"], None) {
                let note = give_back();
                return Err(refuse(
                    &task,
                    with_note(format!("não consegui preparar o AGENTS.md do repositório principal para a mesclagem: {e}"), note),
                ));
            }
            app_block = Some(current);
        }
        let restore_block = || {
            let Some(saved) = &app_block else { return };
            if let Err(e) = crate::workspace::restore_marker_block(&repo.root.join("AGENTS.md"), saved) {
                log::warn!("falha ao regravar o bloco do AGENTS.md depois da mesclagem: {e}");
                self.update_task(project, &task.id, |t| t.log(format!("aviso: não consegui regravar o bloco do app no AGENTS.md: {e}")));
            }
        };

        // A mesclagem.
        let message = format!("Merge tarefa {}: {}", task.id, task.title);
        if let Err(e) = run(repo.cmd(&["merge", "--no-ff", "-m", &message, &task.branch]), None, COMMIT_TIMEOUT) {
            if repo.state() == RepoState::Merging {
                let files = git_out(&repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let files: Vec<String> = files.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                return match repo.write(&["merge", "--abort"], None) {
                    Ok(_) => {
                        restore_block();
                        let note = give_back();
                        let msg = format!(
                            "conflito ao mesclar {} em \"{}\" (arquivos: {}); a mesclagem foi desfeita e a tarefa continua em revisão. Devolva-a ao agente pedindo para atualizar a branch com a base, ou resolva à mão",
                            task.branch,
                            task.base,
                            if files.is_empty() { "não identificados".to_owned() } else { short_list(&files) }
                        );
                        self.update_task(project, &task.id, |t| t.log(format!("conflito na mesclagem ({}); desfeita", short_list(&files))));
                        Err(MergeRefusal { error: TaskError::Conflict(with_note(msg, note)), block: Some(MergeBlock::Conflict(files)) })
                    }
                    // Sem conseguir desfazer, a pasta está no meio da mesclagem: o stash não é devolvido por cima.
                    Err(abort) => Err(self
                        .fail_merge(
                            project,
                            &task.id,
                            format!(
                                "conflito ao mesclar e não consegui desfazer ({abort}): rode git merge --abort em {}{}",
                                repo.root.display(),
                                if stashed.is_some() {
                                    "; as suas alterações sem commit continuam guardadas (git stash list)"
                                } else {
                                    ""
                                }
                            ),
                        )
                        .into()),
                };
            }
            restore_block();
            let note = give_back();
            return Err(self.fail_merge(project, &task.id, with_note(format!("a mesclagem falhou: {e}"), note)).into());
        }
        restore_block();
        let note = give_back();
        let full_head = git_out(&repo, &["rev-parse", "HEAD"]).map(|s| s.trim().to_owned()).unwrap_or_default();
        let head = short_hash(&full_head).to_owned();

        // A branch da tarefa passa a ser a própria base: se for reprovada, o agente continua dali.
        let aligned = align_with_base(&repo, &task);
        let merged = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Validating;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.merge_commit = Some(full_head.clone()).filter(|h| !h.is_empty());
            t.log(format!("mesclada em {} ({head}); aguardando validação", t.base));
            if let Err(e) = &aligned {
                t.log(format!("aviso: não consegui alinhar a branch {} com {}: {e}", t.branch, t.base));
            }
            t.clone()
        });
        let task = merged.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(
            FeedEntry::system(format!("Tarefa {} mesclada em {}, aguardando validação: {}", task.id, task.base, task.title))
                .in_project(project),
        );
        Ok(MergeOutcome { task, note })
    }

    // ---------- Validar ----------

    /// O usuário testou e aprovou: fecha a aba, remove a worktree e a branch e marca a tarefa como
    /// concluída (`Merged`). Exige `Validating`. Recusa, sem mudar nada, se a worktree tem trabalho sem
    /// commit ou se a branch tem commits que a base não tem (o agente seguiu trabalhando): reprove a
    /// tarefa ou descarte-a.
    pub fn task_validate(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se valida uma tarefa aguardando validação",
                task.id,
                task.status.label()
            )));
        }
        let repo = self.task_repo(project)?;
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não removi nada", task.id)));
        }
        let _git = self.inner.git.op.lock();
        let refuse = |msg: String| -> TaskError {
            self.update_task(project, &task.id, |t| t.log(format!("validação recusada: {msg}")));
            TaskError::Conflict(msg)
        };
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse(format!(
                "a worktree da tarefa tem trabalho sem commit ({}): reprove a tarefa pedindo o commit, ou descarte-a",
                short_list(&d)
            )));
        }
        let mut warnings = Vec::new();
        if branch_exists(&repo, &task.branch) {
            if branch_exists(&repo, &task.base) {
                let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
                if ahead.trim() != "0" {
                    return Err(refuse(format!(
                        "a branch {} tem commits que ainda não foram mesclados em \"{}\": reprove a tarefa para o agente entregá-los, ou descarte-a",
                        task.branch, task.base
                    )));
                }
            } else {
                warnings.push(format!("a base {} não existe mais; não conferi se a branch tinha commits fora dela", task.base));
            }
        }

        // Limpeza: aba, worktree, branch (o que ela trouxe já está na base).
        self.close_task_instance(project, &task);
        warnings.extend(self.remove_task_worktree(&repo, &task));
        if task.worktree.exists() {
            // Algo ainda segura a pasta: concluir agora deixaria pasta e branch órfãs, sem como limpar depois.
            let why = warnings.join("; ");
            self.update_task(project, &task.id, |t| t.log(format!("validação incompleta: {why}")));
            return Err(TaskError::Conflict(format!(
                "não consegui apagar a pasta da worktree ({why}): feche o que a usa e valide de novo"
            )));
        }
        if let Err(e) = delete_branch(&repo, &task.branch, true) {
            warnings.push(format!("a branch {} ficou: {e}", task.branch));
        }
        let done = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Merged;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.log("validada; concluída; worktree e branch removidas");
            for w in &warnings {
                t.log(format!("aviso: {w}"));
            }
            t.clone()
        });
        let task = done.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(format!("Tarefa {} concluída: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Reprovar ----------

    /// O usuário testou e reprovou: `Validating` volta a `Running` na mesma worktree (que já tem o código da
    /// base) e o motivo é entregue ao mesmo agente (a aba é reaberta se foi fechada). Se a pasta da worktree
    /// sumiu nesse meio-tempo, é recriada a partir da branch (ou da base). O ciclo segue: done → revisão →
    /// mesclar → aguardando validação.
    pub fn task_reject(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Validating {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se reprova uma tarefa aguardando validação",
                task.id,
                task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid("o motivo está vazio: diga o que não funcionou no teste".into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                "as tarefas rodam nas abas dos agentes: ligue \"Abrir abas dos agentes\" nas Configurações".into(),
            ));
        }
        let feedback = cap_text(feedback, MAX_TEXT);
        let aligned = {
            let repo = self.task_repo(project)?;
            let _git = self.inner.git.op.lock();
            self.restore_worktree(&repo, &task)?;
            align_with_base(&repo, &task)
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.log("reprovada na validação; devolvida ao agente com o motivo");
            if let Err(e) = &aligned {
                t.log(format!("aviso: não consegui alinhar a branch {} com {}: {e}", t.branch, t.base));
            }
            // `before` não tem o feedback novo: a mensagem mostra só os anteriores.
            let before = t.clone();
            t.add_feedback("validação", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        let ident = self.open_task_instance(project, &task);
        let prompt = reject_message(&before, &feedback, aligned.is_ok(), &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "foi reprovada na validacao, com o motivo no arquivo", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, "motivo da reprovação entregue ao agente");
        Ok(task)
    }

    /// Garante a pasta da worktree: se foi apagada à mão, recria a partir da branch da tarefa (ou da base,
    /// se a branch também sumiu). Só mexe em `<raiz>/.agentdeck/worktrees`.
    fn restore_worktree(&self, repo: &Repo, task: &Task) -> Result<(), TaskError> {
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não mexi em nada", task.id)));
        }
        if task.worktree.is_dir() {
            return Ok(());
        }
        let _ = repo.write(&["worktree", "prune"], None);
        let path = task.worktree.to_string_lossy().into_owned();
        let args: Vec<&str> = if branch_exists(repo, &task.branch) {
            vec!["worktree", "add", &path, &task.branch]
        } else {
            vec!["worktree", "add", "-b", &task.branch, &path, &task.base]
        };
        run(repo.cmd(&args), None, COMMIT_TIMEOUT)
            .map(|_| ())
            .map_err(|e| TaskError::Failed(format!("a pasta da worktree sumiu e não consegui recriá-la: {e}")))
    }

    // ---------- Descartar ----------

    /// Descarta a tarefa (qualquer estado que não seja final): fecha a aba, remove a worktree (à força,
    /// mesmo se a pasta já sumiu) e a branch. Funciona sem o repositório: aí só o registro muda. Em
    /// `Validating` o que já foi mesclado continua na base: descartar não desfaz o merge.
    pub fn task_discard(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status.is_terminal() {
            return Err(TaskError::Conflict(format!("a tarefa {} já está {}", task.id, task.status.label())));
        }
        let was_validating = task.status == TaskStatus::Validating;
        let mut warnings = Vec::new();
        if task.started_ms.is_some() {
            self.close_task_instance(project, &task);
        }
        match self.task_repo(project) {
            Ok(repo) => {
                let _git = self.inner.git.op.lock();
                warnings.extend(self.remove_task_worktree(&repo, &task));
                if valid_task_branch(&task.branch) {
                    if let Err(e) = delete_branch(&repo, &task.branch, true) {
                        warnings.push(format!("a branch {} ficou: {e}", task.branch));
                    }
                }
            }
            Err(e) => warnings.push(format!("repositório indisponível ({e}): só o registro da tarefa mudou")),
        }
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Discarded;
            t.finished_ms = Some(now_ms());
            if was_validating {
                t.log(format!("descartada; worktree e branch removidas (o que já foi mesclado continua em {})", t.base));
            } else {
                t.log("descartada; worktree e branch removidas");
            }
            for w in &warnings {
                t.log(format!("aviso: {w}"));
            }
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(format!("Tarefa {} descartada: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    /// Esquece a instância da tarefa no core e manda a interface fechar a aba dela.
    fn close_task_instance(&self, project: &str, task: &Task) {
        self.close_instance(project, &task.instance);
        self.emit(CoreEvent::CloseInstance { project: project.to_owned(), id: task.instance.clone() });
    }

    /// Remove a worktree da tarefa e as marcas dela no git. Só mexe na pasta se ela estiver mesmo em
    /// `<raiz>/.agentdeck/worktrees` (o JSON pode ter sido editado). Tenta por alguns segundos: no
    /// Windows o terminal do agente ainda segura a pasta logo depois de a aba fechar. Devolve os avisos.
    fn remove_task_worktree(&self, repo: &Repo, task: &Task) -> Vec<String> {
        let mut warnings = Vec::new();
        let allowed = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if safe_worktree(&task.worktree, &allowed) {
            let dir = util::absolutize(&task.worktree);
            let path = dir.to_string_lossy().into_owned();
            let mut last = String::new();
            for attempt in 0..REMOVE_TRIES {
                if !dir.exists() {
                    break;
                }
                match repo.write(&["worktree", "remove", "--force", &path], None) {
                    Ok(_) => {}
                    Err(e) => last = e.to_string(),
                }
                if !dir.exists() {
                    break;
                }
                if attempt + 1 < REMOVE_TRIES {
                    std::thread::sleep(REMOVE_WAIT);
                }
            }
            if dir.exists() {
                // O git desistiu (ou removeu só o registro): apaga a pasta, que é nossa.
                if let Err(e) = std::fs::remove_dir_all(&dir) {
                    warnings.push(format!(
                        "a pasta {} ficou ({}{e})",
                        dir.display(),
                        if last.is_empty() { String::new() } else { format!("{last}; ") }
                    ));
                }
            }
        } else {
            warnings.push(format!("a pasta {} não está em .agentdeck/worktrees: não mexi nela", task.worktree.display()));
        }
        // Tira do git as worktrees cuja pasta não existe mais (inclusive a que acabou de sumir).
        if let Err(e) = repo.write(&["worktree", "prune"], None) {
            warnings.push(format!("git worktree prune falhou: {e}"));
        }
        warnings
    }
}

/// A pasta da worktree está dentro de `<raiz>/.agentdeck/worktrees` (e não é a própria pasta-pai).
fn safe_worktree(worktree: &Path, allowed: &Path) -> bool {
    let wt = util::absolutize(worktree);
    util::path_within(&wt, allowed) && !util::same_path(&wt, allowed)
}

/// Branch de tarefa (`tarefa/…`) com nome aceitável.
fn valid_task_branch(branch: &str) -> bool {
    branch.starts_with("tarefa/") && crate::git::valid_branch_name(branch)
}

/// Saída padrão de um comando do git no repositório principal (limite das operações locais).
fn git_out(repo: &Repo, args: &[&str]) -> Result<String, GitError> {
    run(repo.cmd(args), None, WRITE_TIMEOUT).map(|o| String::from_utf8_lossy(&o.stdout).into_owned())
}

/// Arquivos que a branch trouxe em relação à base (`git diff --name-only base...branch`); vazio se a branch
/// não existe mais ou o git falha.
fn changed_files(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    if !valid_task_branch(branch) || !crate::git::valid_branch_name(base) {
        return Vec::new();
    }
    let range = format!("{base}...{branch}");
    repo.read(&["diff", "--name-only", "--no-ext-diff", &range, "--"])
        .map(|o| String::from_utf8_lossy(&o.stdout).lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect())
        .unwrap_or_default()
}

/// `git diff --stat base...branch`, cortado; "sem alterações" quando vazio.
fn diff_stat(repo: &Repo, base: &str, branch: &str) -> Result<String, String> {
    let out = git_out(repo, &["diff", "--stat", &format!("{base}...{branch}"), "--"]).map_err(|e| e.to_string())?;
    let out = out.trim();
    if out.is_empty() {
        return Ok("(nenhuma alteração em relação à base)".into());
    }
    Ok(cap_text(out, MAX_STAT))
}

/// Caminhos com alterações (inclusive novos, não rastreados) na worktree. `None` se a pasta não existe
/// ou o git não respondeu.
fn worktree_dirty(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Como [`worktree_dirty`], mas pedindo ao git que não tome o `index.lock` da worktree (`--no-optional-locks`):
/// serve para olhar a pasta de outro agente sem atrapalhar o `git add`/`commit` dele.
fn worktree_dirty_unlocked(repo: &Repo, worktree: &Path) -> Option<Vec<String>> {
    if !worktree.is_dir() {
        return None;
    }
    let cmd = crate::git::base_command(&repo.git, worktree, &["--no-optional-locks", "status", "--porcelain=v1"]);
    let out = run(cmd, None, WRITE_TIMEOUT).ok()?;
    Some(dirty_paths(&String::from_utf8_lossy(&out.stdout)))
}

/// Une os arquivos da branch com os sem commit: normalizados (`/`), sem repetição, em ordem alfabética e sem o
/// `AGENTS.md`, que o app reescreve e não é trabalho do agente.
fn union_files(committed: Vec<String>, dirty: Option<Vec<String>>) -> Vec<String> {
    let mut files: Vec<String> = committed
        .into_iter()
        .chain(dirty.unwrap_or_default())
        .map(|f| crate::learnings::normalize_path(&f))
        .filter(|f| !f.is_empty() && f != "AGENTS.md")
        .collect();
    files.sort();
    files.dedup();
    files
}

/// O hash curto (7 caracteres) de um hash completo, para mensagens e eventos.
fn short_hash(hash: &str) -> &str {
    hash.get(..7).unwrap_or(hash)
}

/// A branch existe neste repositório.
fn branch_exists(repo: &Repo, branch: &str) -> bool {
    repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_ok()
}

/// Põe a branch da tarefa no mesmo commit da base já mesclada, dentro da worktree (`git merge --ff-only`:
/// a branch é ancestral do commit de mesclagem, então avança sem criar commit). Erro = a mensagem do git.
fn align_with_base(repo: &Repo, task: &Task) -> Result<(), String> {
    if !task.worktree.is_dir() {
        return Err(format!("a pasta {} não existe", task.worktree.display()));
    }
    let cmd = crate::git::base_command(&repo.git, &task.worktree, &["merge", "--ff-only", &task.base]);
    run(cmd, None, COMMIT_TIMEOUT).map(|_| ()).map_err(|e| e.to_string())
}

/// Apaga a branch (se existir). `force` = `-D`.
fn delete_branch(repo: &Repo, branch: &str, force: bool) -> Result<(), String> {
    if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_err() {
        return Ok(());
    }
    repo.write(&["branch", if force { "-D" } else { "-d" }, branch], None).map(|_| ()).map_err(|e| e.to_string())
}

/// O stash que o app criou para guardar as alterações do repositório principal (o hash identifica a entrada:
/// a pilha do stash é compartilhada com as outras worktrees, `stash@{0}` pode não ser a nossa na hora de devolver).
struct Stash {
    sha: String,
    message: String,
}

/// O topo do stash (`refs/stash`), se há.
fn stash_top(repo: &Repo) -> Option<String> {
    repo.read(&["rev-parse", "-q", "--verify", "refs/stash"]).ok().map(|o| String::from_utf8_lossy(&o.stdout).trim().to_owned())
}

/// Guarda no stash as alterações sem commit dos arquivos rastreados do repositório principal, menos o `AGENTS.md`
/// (o app trata dele à parte). Não leva arquivos não rastreados. `Ok(None)` = não havia o que guardar.
fn stash_push(repo: &Repo, task: &Task) -> Result<Option<Stash>, String> {
    let listed = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "HEAD", "--"]).map_err(|e| e.to_string())?;
    let mut stdin = Vec::new();
    for path in listed.split('\0').filter(|p| !p.is_empty() && *p != "AGENTS.md") {
        stdin.extend_from_slice(path.as_bytes());
        stdin.push(0);
    }
    if stdin.is_empty() {
        return Ok(None);
    }
    let before = stash_top(repo);
    let message = format!("agentdeck: guardado antes de mesclar a tarefa {}", task.id);
    let args = ["stash", "push", "-m", message.as_str(), "--pathspec-from-file=-", "--pathspec-file-nul"];
    run(repo.cmd(&args), Some(stdin), COMMIT_TIMEOUT).map_err(|e| e.to_string())?;
    let after = stash_top(repo);
    Ok(after.filter(|a| Some(a) != before.as_ref()).map(|sha| Stash { sha, message }))
}

/// A posição atual (`stash@{n}`) da entrada do stash, se ela ainda existe.
fn stash_ref(repo: &Repo, sha: &str) -> Option<String> {
    let list = git_out(repo, &["stash", "list", "--format=%H %gd"]).ok()?;
    list.lines().find_map(|l| l.split_once(' ').filter(|(h, _)| *h == sha).map(|(_, r)| r.trim().to_owned()))
}

/// Os arquivos em que devolver o stash por cima de `HEAD` daria conflito, sem tocar na pasta
/// (`git merge-tree --write-tree`, git 2.38+). `None` = não deu para conferir (git antigo, erro).
fn stash_pop_conflicts(repo: &Repo, sha: &str) -> Option<Vec<String>> {
    let base = format!("--merge-base={sha}^1");
    let out = repo.cmd(&["merge-tree", "--write-tree", "--name-only", "--no-messages", &base, "HEAD", sha]).output().ok()?;
    match out.status.code() {
        Some(0) => Some(Vec::new()),
        Some(1) => {
            let text = String::from_utf8_lossy(&out.stdout);
            // A primeira linha é o tree; os arquivos em conflito vêm depois, até uma linha vazia.
            Some(text.lines().skip(1).take_while(|l| !l.is_empty()).map(str::to_owned).collect())
        }
        _ => None,
    }
}

/// Devolve o stash à pasta principal depois da mesclagem. Só faz o `stash pop` quando a conferência diz que não há
/// conflito; na dúvida, ou em conflito, deixa no stash (intacto) e o aviso explica como recuperar. Devolve o aviso
/// e se o stash continua guardado.
fn stash_pop(repo: &Repo, stash: &Stash) -> (String, bool) {
    let recover = "Para recuperá-las: abra um terminal na pasta do projeto, rode git stash list para achar a entrada e git stash pop para devolvê-la (resolvendo o que chocar).";
    let kept = |why: String| {
        (
            format!(
                "Suas alterações sem commit continuam guardadas no stash do git (\"{}\"): {why} Nada se perdeu. {recover}",
                stash.message
            ),
            true,
        )
    };
    let Some(reference) = stash_ref(repo, &stash.sha) else {
        return (
            "O stash com as suas alterações não está mais na lista do git (alguém o usou ou apagou?); confira com git stash list.".into(),
            true,
        );
    };
    match stash_pop_conflicts(repo, &stash.sha) {
        Some(files) if files.is_empty() => {}
        Some(files) => return kept(format!("ao devolvê-las elas se chocariam com o que a tarefa trouxe em {}.", short_list(&files))),
        None => return kept("não consegui conferir se elas se chocam com o que a tarefa trouxe.".into()),
    }
    match repo.write(&["stash", "pop", &reference], None) {
        Ok(_) => ("Suas alterações sem commit foram guardadas durante a mesclagem e já voltaram para a pasta.".to_owned(), false),
        Err(e) if stash_ref(repo, &stash.sha).is_some() => {
            let unmerged = git_out(repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
            let mess = if unmerged.trim().is_empty() { "" } else { " A pasta ficou com arquivos em conflito: confira com git status." };
            kept(format!("o git não conseguiu devolvê-las ({e}).{mess}"))
        }
        // O pop falhou depois de já ter apagado a entrada: as alterações estão na pasta.
        Err(_) => ("Suas alterações sem commit voltaram para a pasta.".into(), false),
    }
}

/// Arquivos novos do usuário (não rastreados, no repositório principal) que a branch da tarefa também cria.
fn untracked_clash(repo: &Repo, base: &str, branch: &str) -> Vec<String> {
    let range = format!("{base}...{branch}");
    let added = git_out(repo, &["diff", "--name-only", "-z", "--no-renames", "--diff-filter=A", &range, "--"]).unwrap_or_default();
    let untracked = git_out(repo, &["ls-files", "--others", "--exclude-standard", "-z"]).unwrap_or_default();
    let mine: std::collections::HashSet<&str> = untracked.split('\0').filter(|p| !p.is_empty()).collect();
    added.split('\0').filter(|p| mine.contains(p)).map(str::to_owned).collect()
}

/// A mensagem seguida do aviso do stash, se houve.
fn with_note(msg: String, note: Option<String>) -> String {
    match note {
        Some(n) => format!("{msg} {n}"),
        None => msg,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slugs_are_ascii_lowercase_and_short() {
        assert_eq!(slugify("Corrigir o botão de Ação!", 40), "corrigir-o-botao-de-acao");
        assert_eq!(slugify("  --Olá,   mundo--  ", 40), "ola-mundo");
        assert_eq!(slugify("日本語", 40), "");
        assert_eq!(slugify("a/b\\c:d", 40), "a-b-c-d");
        let long = slugify(&"palavra ".repeat(20), 40);
        assert!(long.len() <= 40 && !long.ends_with('-'), "{long}");
        assert_eq!(branch_name("t3", "Corrigir o botão"), "tarefa/t3-corrigir-o-botao");
        assert_eq!(branch_name("t3", "???"), "tarefa/t3");
        assert!(crate::git::valid_branch_name(&branch_name("t12", "Título com / barra e \"aspas\" ~ ^ :")));
    }

    #[test]
    fn task_ids_are_normalized() {
        assert_eq!(parse_task_id("t3").as_deref(), Some("t3"));
        assert_eq!(parse_task_id(" T03 ").as_deref(), Some("t3"));
        assert_eq!(parse_task_id("12").as_deref(), Some("t12"));
        for bad in ["", "t", "tx", "t-1", "t3a", "claude@t3", "../t3", "12345678901"] {
            assert_eq!(parse_task_id(bad), None, "{bad}");
        }
    }

    #[test]
    fn titles_are_one_clean_line() {
        assert_eq!(clean_title("  Uma \n tarefa\t boa ").unwrap(), "Uma tarefa boa");
        assert!(matches!(clean_title(" \n\t "), Err(TaskError::Invalid(_))));
        assert_eq!(clean_title(&"x".repeat(500)).unwrap().chars().count(), MAX_TITLE);
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
    fn exclude_is_written_once() {
        let first = exclude_with_agentdeck("# regras\n*.log").unwrap();
        assert_eq!(first, "# regras\n*.log\n# AgentDeck: worktrees das tarefas\n/.agentdeck/\n");
        assert_eq!(exclude_with_agentdeck(&first), None);
        assert!(exclude_with_agentdeck("").unwrap().ends_with("/.agentdeck/\n"));
        assert_eq!(exclude_with_agentdeck("a\n/.agentdeck/\n"), None);
    }

    #[test]
    fn porcelain_paths() {
        let text = " M src/a.rs\n?? novo arquivo.txt\nR  velho.rs -> novo.rs\nA  \"com aspas.txt\"\n";
        assert_eq!(dirty_paths(text), ["src/a.rs", "novo arquivo.txt", "novo.rs", "com aspas.txt"]);
        assert!(dirty_paths("").is_empty());
    }

    fn sample() -> Task {
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

    #[test]
    fn the_start_message_carries_the_brief_and_the_rules() {
        let m = start_message(&sample(), Some("claude"), "");
        assert!(m.starts_with("[AgentDeck · Tarefa t3: Corrigir o login]\n\nFaça X.\n\nDepois Y."), "{m}");
        assert!(m.contains("SOMENTE na pasta") && m.contains("tarefa/t3-corrigir-o-login") && m.contains("criada a partir de main"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(m.contains("Não faça merge nem push"), "{m}");
        assert!(m.contains("Não peça a ninguém") && m.contains("termina no --task done"), "{m}");
        assert!(m.contains("ask-agent --boss") && m.contains("\"claude\""), "{m}");
        // O brief termina com uma ordem: o agente não fica esperando um "pode começar".
        assert!(m.ends_with("Não espere confirmação nem pergunte se pode começar."), "{m}");
        // Sem chefe agente, as dúvidas ficam no terminal; sem brief, vale o título; sem ESC.
        let mut t = sample();
        t.brief = "  ".into();
        t.title = "T\x1b[201~ítulo".into();
        let m = start_message(&t, None, "");
        assert!(m.contains("escreva aqui no terminal") && !m.contains("--boss"), "{m}");
        assert!(!m.contains('\x1b'), "{m:?}");
    }

    #[test]
    fn the_start_message_carries_the_memory_and_the_memory_rules() {
        // Sem memória, nada de seção vazia.
        let plain = start_message(&sample(), None, &memory_block("", " "));
        assert!(!plain.contains("O que o projeto já aprendeu") && !plain.contains("Situação atual"), "{plain}");
        // Com memória: entre o pedido e as regras, cada parte com o seu título.
        let memory = memory_block("### T (memory/aprendizados/a.md)\ncorpo", "Outras tarefas em andamento: nenhuma.");
        let m = start_message(&sample(), None, &memory);
        let (brief, learned, situation, rules) = (
            m.find("Depois Y."),
            m.find("O que o projeto já aprendeu:\n### T"),
            m.find("Situação atual:\nOutras tarefas"),
            m.find("Regras da tarefa t3"),
        );
        assert!(brief < learned && learned < situation && situation < rules && rules.is_some(), "{m}");
        // As regras mandam consultar e registrar aprendizados.
        assert!(m.contains("ask-agent --learn <arquivo>") && m.contains("memory/aprendizados/"), "{m}");
        assert_eq!(memory_block("só longo prazo", ""), "O que o projeto já aprendeu:\nsó longo prazo");
    }

    #[test]
    fn feedbacks_come_back_in_the_return_and_reject_messages() {
        let mut t = sample();
        // Primeira devolução: nada anterior.
        assert!(!return_message(&t, "Faltou o teste.", "").contains("Feedbacks anteriores"));
        t.add_feedback("revisão", "Faltou o teste.");
        t.add_feedback("validação", "Fecha ao salvar.");
        let m = return_message(&t, "Agora o log.", "");
        assert!(m.contains("\n\nAgora o log.\n\nFeedbacks anteriores desta tarefa"), "{m}");
        assert!(m.contains("1. [revisão] Faltou o teste.\n2. [validação] Fecha ao salvar.\n\n---"), "{m}");
        let m = reject_message(&t, "Ainda fecha.", true, "");
        assert!(m.contains("Ainda fecha.\n\nFeedbacks anteriores") && m.contains("2. [validação] Fecha ao salvar."), "{m}");
        // Muitos: só os últimos, e cada um cortado.
        for n in 0..10 {
            t.add_feedback("revisão", &format!("pedido {n} {}", "x".repeat(2 * HISTORY_ITEM_CHARS)));
        }
        let m = return_message(&t, "mais um", "");
        assert!(m.contains("(mais 7 anterior(es) omitido(s))") && m.contains("pedido 9") && !m.contains("pedido 4"), "{m}");
        assert!(m.contains('…') && m.chars().count() < 6 * (HISTORY_ITEM_CHARS + 60) + 800, "{}", m.chars().count());
    }

    #[test]
    fn the_learnings_of_the_cited_files_come_before_the_closing_rule() {
        let learned = "### T (memory/aprendizados/a.md)\ncorpo";
        let mut t = sample();
        t.add_feedback("revisão", "Primeiro.");
        for m in [return_message(&t, "Mexa em x.rs.", learned), reject_message(&t, "Mexa em x.rs.", true, learned)] {
            let (feedback, history, section, rule) = (
                m.find("Mexa em x.rs."),
                m.find("Feedbacks anteriores"),
                m.find("O que o projeto já aprendeu sobre os arquivos citados:\n\n### T"),
                m.find("corpo\n\n---\n"),
            );
            assert!(feedback < history && history < section && section < rule && rule.is_some(), "{m}");
        }
        // Vazio ou só espaços: a mensagem fica idêntica à de sempre.
        for blank in ["", "  \n "] {
            assert_eq!(return_message(&t, "x", blank), return_message(&t, "x", ""));
            assert!(!reject_message(&t, "x", true, blank).contains("sobre os arquivos citados"));
        }
    }

    #[test]
    fn the_situation_files_join_the_branch_and_the_uncommitted_work() {
        let got = union_files(
            vec!["b.rs".into(), "a.rs".into(), "AGENTS.md".into()],
            Some(vec!["c\\d.rs".into(), "a.rs".into(), "AGENTS.md".into()]),
        );
        assert_eq!(got, ["a.rs", "b.rs", "c/d.rs"]);
        // A worktree que o git não respondeu não apaga o que a branch já tem.
        assert_eq!(union_files(vec!["a.rs".into()], None), ["a.rs"]);
        assert!(union_files(Vec::new(), Some(Vec::new())).is_empty());
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
    fn the_return_message_has_the_feedback() {
        let m = return_message(&sample(), "  Faltou o teste.  ", "");
        assert!(m.contains("ajustes pedidos") && m.contains("\n\nFaltou o teste.\n\n"), "{m}");
        assert!(m.contains("ask-agent --task done t3 --file"), "{m}");
    }

    #[test]
    fn the_reject_message_says_what_was_merged_and_why_it_failed() {
        let m = reject_message(&sample(), "  Fecha ao salvar.  ", true, "");
        assert!(m.contains("reprovada na validação") && m.contains("já foi mesclado em main"), "{m}");
        assert!(
            m.contains(
                "pelo seguinte motivo:

Fecha ao salvar.

"
            ),
            "{m}"
        );
        assert!(m.contains("mesma branch tarefa/t3-corrigir-o-login") && m.contains("ask-agent --task done t3 --file"), "{m}");
        assert!(!m.contains("alinhar") && !m.contains(''), "{m:?}");
        // Branch que não pôde ser alinhada: o agente recebe o comando para fazê-lo.
        assert!(reject_message(&sample(), "x", false, "").contains("rode na sua pasta: git merge main"));
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
    fn the_counter_never_goes_back() {
        let mut p = ProjectTasks { next: 2, tasks: vec![Task { id: "t7".into(), ..Task::default() }], ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 8);
        let mut p = ProjectTasks { next: 40, ..ProjectTasks::default() };
        p.fix_counter();
        assert_eq!(p.next, 40);
        let mut p = ProjectTasks::default();
        p.fix_counter();
        assert_eq!(p.next, 1);
    }

    #[test]
    fn the_file_round_trips_and_an_unreadable_one_is_kept() {
        let tmp = tempfile::tempdir().expect("tmp");
        let file = tmp.path().join("tarefas").join("p1.json");
        assert!(load_file(&file).tasks.is_empty());
        let mut p = ProjectTasks { next: 5, tasks: vec![sample()], ..ProjectTasks::default() };
        p.fix_counter();
        save_file(&file, &p).expect("save");
        save_file(&file, &p).expect("save de novo por cima");
        let back = load_file(&file);
        assert_eq!((back.next, back.tasks.len()), (5, 1));
        assert_eq!(back.tasks[0], sample());
        let text = std::fs::read_to_string(&file).expect("ler");
        assert!(text.contains("\"status\": \"draft\"") && text.contains("\"created_ms\""), "{text}");
        assert!(!file.with_extension("json.tmp").exists());
        // Ilegível: vem vazio, mas o original fica guardado ao lado.
        std::fs::write(&file, "{ isto não é json").expect("write");
        assert!(load_file(&file).tasks.is_empty());
        let kept = std::fs::read_dir(file.parent().expect("pasta"))
            .expect("dir")
            .flatten()
            .any(|e| e.file_name().to_string_lossy().contains("ilegivel"));
        assert!(kept, "a cópia do arquivo ilegível deveria ficar");
    }

    #[test]
    fn only_the_expected_folder_is_ever_touched() {
        // Como no código real, a pasta permitida já vem absoluta (no Windows, `/repo` ganha a unidade).
        let allowed = &util::absolutize(Path::new("/repo/.agentdeck/worktrees"));
        assert!(safe_worktree(Path::new("/repo/.agentdeck/worktrees/t1"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees"), allowed));
        assert!(!safe_worktree(Path::new("/repo/src"), allowed));
        assert!(!safe_worktree(Path::new("/repo/.agentdeck/worktrees/../../src"), allowed));
        assert!(valid_task_branch("tarefa/t1-x") && !valid_task_branch("main") && !valid_task_branch("tarefa/../x"));
    }

    #[test]
    fn project_ids_are_file_names() {
        assert!(safe_project_id("p1a2b3c4d"));
        for bad in ["", "../x", "a/b", "a\\b", "a b", "x.json"] {
            assert!(!safe_project_id(bad), "{bad}");
        }
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

//! Mesa de tarefas: a unidade de trabalho do AgentDeck.
//!
//! Uma **tarefa** tem um pedido (o brief), um agente com modelo, uma worktree + branch próprias, um
//! relatório, um diff e um ciclo de vida que termina em mesclagem ou descarte:
//!
//! ```text
//! Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Merged
//!   │                 ▲                 │ └─discard──▶ Discarded
//!   │                 └───return────────┘
//!   └─ start com erro ─▶ Failed ──(start de novo)──▶ Running
//! ```
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
    /// Mesclada na base; worktree e branch removidas.
    Merged,
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
            TaskStatus::Merged => "mesclada",
            TaskStatus::Discarded => "descartada",
            TaskStatus::Failed => "com erro",
        }
    }

    /// Mesclada ou descartada: não há mais o que fazer com ela.
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
    /// Id da instância (`claude@t3`).
    pub instance: String,
    /// Quem criou: o id de um agente (`claude`) ou `usuário`. É o chefe da instância quando é um agente aberto.
    pub creator: Option<String>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    /// Quando o relatório chegou (`Review`) ou quando a tarefa terminou (`Merged`/`Discarded`).
    pub finished_ms: Option<u64>,
    /// O que o agente entregou em `ask-agent --task done`.
    pub report: Option<String>,
    /// `git diff --stat base...branch` no momento do relatório.
    pub diff_stat: Option<String>,
    /// A mensagem do erro, em `Failed`.
    pub error: Option<String>,
    /// Histórico curto (no máximo [`MAX_EVENTS`]).
    pub events: Vec<TaskEvent>,
}

impl Task {
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

/// A mensagem que entrega o brief ao agente: o pedido e as regras da tarefa. `boss` é quem criou a
/// tarefa quando é um agente (as dúvidas vão a ele por `ask-agent --boss`).
pub fn start_message(task: &Task, boss: Option<&str>) -> String {
    let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.trim() };
    let doubts = match boss {
        Some(b) => format!("Dúvidas: pergunte a quem criou a tarefa (\"{b}\") com ask-agent --boss \"sua pergunta\"."),
        None => "Dúvidas: escreva aqui no terminal; o usuário acompanha esta aba.".to_owned(),
    };
    terminal_text(&format!(
        "[CommandDeck · Tarefa {id}: {title}]\n\n{brief}\n\n---\n\
Regras da tarefa {id}:\n\
- Trabalhe SOMENTE na pasta {wt}, na branch {branch} (criada a partir de {base}). Não mexa no repositório principal nem em outras pastas.\n\
- Faça commits pequenos nessa branch. Não faça merge nem push: quem mescla é o CommandDeck, depois da revisão.\n\
- Ao terminar, rode ask-agent --task done {id} - com o relatório na entrada padrão (o que foi feito, arquivos alterados, testes rodados e o que falta).\n\
- {doubts}",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        base = task.base,
    ))
}

/// A mensagem que devolve a tarefa ao agente com o feedback da revisão.
pub fn return_message(task: &Task, feedback: &str) -> String {
    terminal_text(&format!(
        "[CommandDeck · Tarefa {id}: {title} — ajustes pedidos na revisão]\n\n{feedback}\n\n---\n\
Continue na mesma pasta ({wt}) e na branch {branch}, com commits pequenos. Ao terminar, rode de novo ask-agent --task done {id} - \
com o relatório na entrada padrão.",
        id = task.id,
        title = task.title,
        wt = task.worktree.display(),
        branch = task.branch,
        feedback = feedback.trim(),
    ))
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

    /// A pasta em que o terminal da instância deve abrir: a worktree da tarefa em andamento ou em revisão.
    pub(crate) fn task_cwd(&self, project: &str, instance: &str) -> Option<PathBuf> {
        self.with_tasks(project, false, |p| {
            p.tasks
                .iter()
                .find(|t| t.instance == instance && matches!(t.status, TaskStatus::Running | TaskStatus::Review))
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
        self.deliver_to_task(project, &task, ident, start_message(&task, boss.as_deref()), "brief entregue ao agente");
        self.feed(FeedEntry::system(format!("Tarefa {} iniciada: {} ({})", task.id, task.title, task.instance)).in_project(project));
        Ok(task)
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

    /// Manda `prompt` à aba da tarefa (abre a aba se preciso) pelo caminho das mensagens entre agentes.
    fn deliver_to_task(&self, project: &str, task: &Task, ident: Ident, prompt: String, log: &str) {
        let from = task.creator.clone().unwrap_or_else(|| "usuário".to_owned());
        self.emit(CoreEvent::Deliver(Delivery {
            request: 0,
            project: project.to_owned(),
            ident,
            create: true,
            from,
            prompt,
            answer: false,
        }));
        self.update_task(project, &task.id, |t| t.log(log));
    }

    // ---------- Relatório ----------

    /// O agente terminou: guarda o relatório e o `git diff --stat base...branch` e põe a tarefa em
    /// revisão. Exige `Running`. A ponte só deixa o próprio agente da tarefa chamar.
    pub fn task_done(&self, project: &str, id: &str, report: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Running {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só uma tarefa em andamento recebe o relatório",
                task.id,
                task.status.label()
            )));
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
            t.log("relatório recebido; em revisão");
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.feed(FeedEntry::system(format!("Tarefa {} pronta para revisão: {}", task.id, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Devolver ----------

    /// Revisão pediu ajustes: `Review` volta a `Running` e o feedback é entregue ao agente (a aba é
    /// reaberta se foi fechada).
    pub fn task_return(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status != TaskStatus::Review {
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
            t.log("devolvida ao agente com feedback");
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        let ident = self.open_task_instance(project, &task);
        self.deliver_to_task(project, &task, ident, return_message(&task, &feedback), "feedback entregue ao agente");
        Ok(task)
    }

    // ---------- Mesclar ----------

    /// Mescla a branch da tarefa na base (`git merge --no-ff`) no repositório principal e limpa tudo:
    /// fecha a aba, remove a worktree e a branch. Exige `Review` (ou `Failed` com relatório, para tentar de
    /// novo). Recusa, sem mudar nada, se o repositório principal não está na base ou tem alterações
    /// sem commit em arquivos rastreados (o `AGENTS.md`, que o app reescreve, não conta), ou se a worktree
    /// da tarefa tem trabalho sem commit. Em conflito, desfaz a mesclagem (`merge --abort`) e a tarefa continua em
    /// `Review`, com os arquivos em conflito na mensagem.
    pub fn task_merge(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_some();
        if task.status != TaskStatus::Review && !retry {
            return Err(TaskError::Conflict(format!(
                "a tarefa {} está {}: só se mescla uma tarefa em revisão",
                task.id,
                task.status.label()
            )));
        }
        let repo = self.task_repo(project)?;
        let refuse = |t: &Task, msg: String| -> TaskError {
            self.update_task(project, &t.id, |t| t.log(format!("mesclagem recusada: {msg}")));
            TaskError::Conflict(msg)
        };
        let expected = util::absolutize(&repo.root.join(AGENTDECK_DIR).join("worktrees"));
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) || !safe_worktree(&task.worktree, &expected) {
            return Err(TaskError::Failed(format!("dados da tarefa {} fora do esperado; não mesclei nada", task.id)));
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
        if !dirty.is_empty() {
            return Err(refuse(
                &task,
                format!(
                    "o repositório principal tem alterações sem commit ({}): faça o commit ou guarde-as e tente de novo",
                    short_list(&dirty)
                ),
            ));
        }
        if let Some(d) = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty()) {
            return Err(refuse(
                &task,
                format!(
                    "a worktree da tarefa tem trabalho sem commit ({}): devolva a tarefa pedindo o commit, ou descarte-a",
                    short_list(&d)
                ),
            ));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_err() {
            return Err(refuse(&task, format!("a branch {} não existe mais: descarte a tarefa", task.branch)));
        }
        let ahead = git_out(&repo, &["rev-list", "--count", &format!("{}..{}", task.base, task.branch)]).map_err(git_error)?;
        if ahead.trim() == "0" {
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

        // A mesclagem.
        let message = format!("Merge tarefa {}: {}", task.id, task.title);
        if let Err(e) = run(repo.cmd(&["merge", "--no-ff", "-m", &message, &task.branch]), None, COMMIT_TIMEOUT) {
            if repo.state() == RepoState::Merging {
                let files = git_out(&repo, &["diff", "--name-only", "--diff-filter=U"]).unwrap_or_default();
                let files: Vec<String> = files.lines().map(str::trim).filter(|l| !l.is_empty()).map(str::to_owned).collect();
                return match repo.write(&["merge", "--abort"], None) {
                    Ok(_) => {
                        let msg = format!(
                            "conflito ao mesclar {} em \"{}\" (arquivos: {}); a mesclagem foi desfeita e a tarefa continua em revisão. Devolva-a ao agente pedindo para atualizar a branch com a base, ou resolva à mão",
                            task.branch,
                            task.base,
                            if files.is_empty() { "não identificados".to_owned() } else { short_list(&files) }
                        );
                        self.update_task(project, &task.id, |t| t.log(format!("conflito na mesclagem ({}); desfeita", short_list(&files))));
                        Err(TaskError::Conflict(msg))
                    }
                    Err(abort) => Err(self.fail_task(
                        project,
                        &task.id,
                        format!("conflito ao mesclar e não consegui desfazer ({abort}): rode git merge --abort em {}", repo.root.display()),
                    )),
                };
            }
            return Err(self.fail_task(project, &task.id, format!("a mesclagem falhou: {e}")));
        }
        let head = git_out(&repo, &["rev-parse", "--short", "HEAD"]).map(|s| s.trim().to_owned()).unwrap_or_default();

        // Limpeza: aba, worktree, branch.
        self.close_task_instance(project, &task);
        let mut warnings = self.remove_task_worktree(&repo, &task);
        if let Err(e) = delete_branch(&repo, &task.branch, false) {
            warnings.push(format!("a branch {} ficou: {e}", task.branch));
        }
        let merged = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Merged;
            t.finished_ms = Some(now_ms());
            t.error = None;
            t.log(format!("mesclada em {} ({head}); worktree e branch removidas", t.base));
            for w in &warnings {
                t.log(format!("aviso: {w}"));
            }
            t.clone()
        });
        let task = merged.ok_or_else(|| TaskError::NotFound(format!("tarefa {id} sumiu")))?;
        self.emit(CoreEvent::GitChanged);
        self.feed(FeedEntry::system(format!("Tarefa {} mesclada em {}: {}", task.id, task.base, task.title)).in_project(project));
        Ok(task)
    }

    // ---------- Descartar ----------

    /// Descarta a tarefa (qualquer estado que não seja final): fecha a aba, remove a worktree (à força,
    /// mesmo se a pasta já sumiu) e a branch. Funciona sem o repositório: aí só o registro muda.
    pub fn task_discard(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if task.status.is_terminal() {
            return Err(TaskError::Conflict(format!("a tarefa {} já está {}", task.id, task.status.label())));
        }
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
            t.log("descartada; worktree e branch removidas");
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

/// Apaga a branch (se existir). `force` = `-D`.
fn delete_branch(repo: &Repo, branch: &str, force: bool) -> Result<(), String> {
    if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{branch}")]).is_err() {
        return Ok(());
    }
    repo.write(&["branch", if force { "-D" } else { "-d" }, branch], None).map(|_| ()).map_err(|e| e.to_string())
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
        let m = start_message(&sample(), Some("claude"));
        assert!(m.starts_with("[CommandDeck · Tarefa t3: Corrigir o login]\n\nFaça X.\n\nDepois Y."), "{m}");
        assert!(m.contains("SOMENTE na pasta") && m.contains("tarefa/t3-corrigir-o-login") && m.contains("criada a partir de main"), "{m}");
        assert!(m.contains("ask-agent --task done t3 -"), "{m}");
        assert!(m.contains("Não faça merge nem push"), "{m}");
        assert!(m.contains("ask-agent --boss") && m.contains("\"claude\""), "{m}");
        // Sem chefe agente, as dúvidas ficam no terminal; sem brief, vale o título; sem ESC.
        let mut t = sample();
        t.brief = "  ".into();
        t.title = "T\x1b[201~ítulo".into();
        let m = start_message(&t, None);
        assert!(m.contains("escreva aqui no terminal") && !m.contains("--boss"), "{m}");
        assert!(!m.contains('\x1b'), "{m:?}");
    }

    #[test]
    fn the_return_message_has_the_feedback() {
        let m = return_message(&sample(), "  Faltou o teste.  ");
        assert!(m.contains("ajustes pedidos") && m.contains("\n\nFaltou o teste.\n\n"), "{m}");
        assert!(m.contains("ask-agent --task done t3 -"), "{m}");
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

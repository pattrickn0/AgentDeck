//! Estado e ações da aba "Tarefas" da barra direita (a pintura fica em `ui/tasks.rs`).
//!
//! Toda chamada de tarefa do core roda em git (criar, iniciar, mesclar, descartar, diff), então vai
//! para uma thread (`Deck::spawn_bg`) e a resposta volta como `UiMsg::Tasks`. A lista é relida quando
//! a aba abre, quando o core avisa (`CoreEvent::TasksChanged`) e depois de cada ação. Aqui também
//! ficam as funções puras que decidem o que a tela mostra (grupos, textos, ações por estado), com testes.

use std::collections::HashSet;
use std::path::PathBuf;

use commanddeck_core::git::GitDiff;
use commanddeck_core::{models, Task, TaskError, TaskEvent, TaskSpec, TaskStatus};

use crate::deck::{AttachTarget, Deck, UiMsg};

// ---------------------------------------------------------------------------
// Funções puras
// ---------------------------------------------------------------------------

/// Grupos da lista, na ordem em que aparecem (o que precisa do humano vem primeiro).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Review,
    /// Mescladas, esperando o usuário testar.
    Validating,
    Running,
    Draft,
    Failed,
    /// Validadas (concluídas).
    Done,
    /// Descartadas (canceladas): ficam à parte das concluídas.
    Cancelled,
}

impl Group {
    pub const ORDER: [Group; 7] =
        [Group::Review, Group::Validating, Group::Running, Group::Draft, Group::Failed, Group::Done, Group::Cancelled];

    pub fn title(self) -> &'static str {
        match self {
            Group::Review => "Em revisão",
            Group::Validating => "Aguardando validação",
            Group::Running => "Em andamento",
            Group::Draft => "Rascunho",
            Group::Failed => "Falhou",
            Group::Done => "Concluídas",
            Group::Cancelled => "Canceladas",
        }
    }

    pub fn of(status: TaskStatus) -> Group {
        match status {
            TaskStatus::Review => Group::Review,
            TaskStatus::Validating => Group::Validating,
            TaskStatus::Running => Group::Running,
            TaskStatus::Draft => Group::Draft,
            TaskStatus::Failed => Group::Failed,
            TaskStatus::Merged => Group::Done,
            TaskStatus::Discarded => Group::Cancelled,
        }
    }
}

/// Nome do estado para o chip (com inicial maiúscula, ao contrário de `TaskStatus::label`).
pub fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Draft => "Rascunho",
        TaskStatus::Running => "Em andamento",
        TaskStatus::Review => "Em revisão",
        TaskStatus::Validating => "Aguardando validação",
        TaskStatus::Merged => "Concluída",
        TaskStatus::Discarded => "Descartada",
        TaskStatus::Failed => "Falhou",
    }
}

/// A última coisa que aconteceu com a tarefa (ms): o último evento do histórico, ou a criação.
pub fn last_activity(task: &Task) -> u64 {
    task.events.iter().map(|e| e.at_ms).max().unwrap_or(0).max(task.created_ms)
}

/// Tarefas por grupo (só os que têm alguma), cada um da atividade mais recente para a mais antiga.
pub fn group_tasks(tasks: &[Task]) -> Vec<(Group, Vec<&Task>)> {
    Group::ORDER
        .iter()
        .filter_map(|&g| {
            let mut list: Vec<&Task> = tasks.iter().filter(|t| Group::of(t.status) == g).collect();
            list.sort_by(|a, b| last_activity(b).cmp(&last_activity(a)).then_with(|| task_number(&b.id).cmp(&task_number(&a.id))));
            (!list.is_empty()).then_some((g, list))
        })
        .collect()
}

/// `t12` → 12 (0 se não for um id de tarefa).
fn task_number(id: &str) -> u64 {
    id.trim_start_matches('t').parse().unwrap_or(0)
}

/// Quantas tarefas esperam a revisão do humano.
pub fn review_count(tasks: &[Task]) -> usize {
    tasks.iter().filter(|t| t.status == TaskStatus::Review).count()
}

/// Quantas tarefas, já mescladas, esperam o teste do humano.
pub fn validating_count(tasks: &[Task]) -> usize {
    tasks.iter().filter(|t| t.status == TaskStatus::Validating).count()
}

/// O resumo do `git diff --stat`: arquivos, linhas novas e linhas removidas.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffSummary {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

impl DiffSummary {
    /// `3 arquivos` (singular com 1; "sem alterações" com 0).
    pub fn files_text(&self) -> String {
        match self.files {
            0 => "sem alterações".to_owned(),
            1 => "1 arquivo".to_owned(),
            n => format!("{n} arquivos"),
        }
    }

    /// `3 arquivos, +120 −8` (sem o lado que é zero).
    pub fn text(&self) -> String {
        let mut out = self.files_text();
        let lines: Vec<String> =
            [(self.insertions, "+"), (self.deletions, "−")].iter().filter(|(n, _)| *n > 0).map(|(n, sign)| format!("{sign}{n}")).collect();
        if !lines.is_empty() {
            out.push_str(&format!(", {}", lines.join(" ")));
        }
        out
    }
}

/// Lê a última linha do `git diff --stat` ("3 files changed, 120 insertions(+), 8 deletions(-)").
/// "(nenhuma alteração em relação à base)" (o que o core guarda sem diferenças) vira zero arquivos.
pub fn parse_diff_stat(stat: &str) -> Option<DiffSummary> {
    if stat.contains("nenhuma alteração") {
        return Some(DiffSummary { files: 0, insertions: 0, deletions: 0 });
    }
    let line = stat.lines().rev().map(str::trim).find(|l| l.contains("changed"))?;
    let mut sum = DiffSummary { files: 0, insertions: 0, deletions: 0 };
    let mut found = false;
    for part in line.split(',') {
        let part = part.trim();
        let Some(n) = part.split_whitespace().next().and_then(|n| n.parse::<u32>().ok()) else { continue };
        if part.contains("file") {
            sum.files = n;
            found = true;
        } else if part.contains("insertion") {
            sum.insertions = n;
        } else if part.contains("deletion") {
            sum.deletions = n;
        }
    }
    found.then_some(sum)
}

/// Uma linha por arquivo do `git diff --stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatLine {
    pub path: String,
    /// Linhas mudadas (`412`), ou `Bin 0 -> 1234 bytes` num arquivo binário.
    pub changes: String,
}

/// Os arquivos do `git diff --stat` (` caminho | 12 ++++--`); a linha do total não entra.
pub fn parse_stat_lines(stat: &str) -> Vec<StatLine> {
    stat.lines()
        .filter_map(|line| {
            let (path, rest) = line.rsplit_once('|')?;
            let path = path.trim();
            let rest = rest.trim();
            let changes = match rest.split_whitespace().next() {
                Some(n) if n.chars().all(|c| c.is_ascii_digit()) => n,
                _ => rest,
            };
            (!path.is_empty() && !changes.is_empty()).then(|| StatLine { path: path.to_owned(), changes: changes.to_owned() })
        })
        .collect()
}

/// `agora`, `há 5 min`, `há 3 h`, `há 2 d` (`then` no futuro conta como agora).
pub fn age_text(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => "agora".to_owned(),
        60..=3599 => format!("há {} min", secs / 60),
        3600..=86_399 => format!("há {} h", secs / 3600),
        _ => format!("há {} d", secs / 86_400),
    }
}

/// O que aconteceu e quando, para a segunda linha do cartão (`pronta há 3 min`).
pub fn status_age(task: &Task, now_ms: u64) -> String {
    let (verb, at) = match task.status {
        TaskStatus::Draft => ("criada", task.created_ms),
        TaskStatus::Running => ("iniciada", task.started_ms.unwrap_or(task.created_ms)),
        TaskStatus::Review => ("pronta", task.finished_ms.unwrap_or_else(|| last_activity(task))),
        TaskStatus::Failed => ("falhou", last_activity(task)),
        TaskStatus::Validating => ("mesclada", task.finished_ms.unwrap_or_else(|| last_activity(task))),
        TaskStatus::Merged => ("concluída", task.finished_ms.unwrap_or_else(|| last_activity(task))),
        TaskStatus::Discarded => ("descartada", task.finished_ms.unwrap_or_else(|| last_activity(task))),
    };
    let age = age_text(now_ms, at);
    if age == "agora" {
        format!("{verb} agora")
    } else {
        format!("{verb} {age}")
    }
}

/// Modelo de LLM para mostrar (`opencode/glm-5.3` → `glm-5.3`); vazio quando é o padrão do CLI.
pub fn model_text(task: &Task) -> Option<&str> {
    task.model.as_deref().map(models::short_model).filter(|m| !m.is_empty())
}

/// Botões do detalhe de uma tarefa. A ordem é a da tela (o primeiro botão é a ação principal).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskAct {
    Start,
    Retry,
    Merge,
    Return,
    Validate,
    Reject,
    GoToTab,
    ReopenTab,
    Discard,
}

impl TaskAct {
    pub fn label(self, task: &Task) -> String {
        match self {
            TaskAct::Start => "Iniciar".to_owned(),
            TaskAct::Retry => "Tentar de novo".to_owned(),
            TaskAct::Merge => format!("Mesclar em {}", task.base),
            TaskAct::Return => "Devolver".to_owned(),
            TaskAct::Validate => "Validar".to_owned(),
            TaskAct::Reject => "Reprovar".to_owned(),
            TaskAct::GoToTab => "Ir para a aba".to_owned(),
            TaskAct::ReopenTab => "Reabrir aba".to_owned(),
            TaskAct::Discard => "Descartar".to_owned(),
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            TaskAct::Start => "Cria a worktree e a branch, abre o agente e entrega o pedido",
            TaskAct::Retry => "Tenta de novo o que falhou (iniciar, ou mesclar se o agente já entregou o relatório)",
            TaskAct::Merge => "Mescla a branch da tarefa na base (git merge --no-ff); o agente e a worktree ficam até você validar",
            TaskAct::Return => "Devolve a tarefa ao agente com um pedido de ajustes",
            TaskAct::Validate => "Você testou e está certo: conclui a tarefa e apaga a worktree e a branch",
            TaskAct::Reject => "Você testou e reprovou: devolve a tarefa ao mesmo agente, com o motivo",
            TaskAct::GoToTab => "Mostra o terminal do agente desta tarefa",
            TaskAct::ReopenTab => "Abre de novo a aba do agente, no terminal da worktree da tarefa",
            TaskAct::Discard => "Descarta a tarefa: apaga a worktree e a branch (sem mesclar)",
        }
    }

    /// Ação principal (botão cheio).
    pub fn is_primary(self) -> bool {
        matches!(self, TaskAct::Start | TaskAct::Retry | TaskAct::Merge | TaskAct::Validate)
    }

    /// Pede confirmação dentro do painel antes de rodar.
    pub fn needs_confirm(self, task: &Task) -> bool {
        match self {
            TaskAct::Discard | TaskAct::Merge => true,
            // Tentar de novo com relatório é uma mesclagem.
            TaskAct::Retry => task.report.is_some(),
            _ => false,
        }
    }
}

/// Os botões de cada estado. `tab_open`: a aba do agente está aberta (senão, "Reabrir aba").
pub fn actions_for(task: &Task, tab_open: bool) -> Vec<TaskAct> {
    let tab = if tab_open { TaskAct::GoToTab } else { TaskAct::ReopenTab };
    match task.status {
        TaskStatus::Draft => vec![TaskAct::Start, TaskAct::Discard],
        TaskStatus::Running => vec![tab, TaskAct::Discard],
        TaskStatus::Review => vec![TaskAct::Merge, TaskAct::Return, tab, TaskAct::Discard],
        TaskStatus::Validating => vec![TaskAct::Validate, TaskAct::Reject, tab, TaskAct::Discard],
        TaskStatus::Failed => vec![TaskAct::Retry, TaskAct::Discard],
        TaskStatus::Merged | TaskStatus::Discarded => Vec::new(),
    }
}

/// Dá para ver o diff da branch (ela existe: a tarefa foi iniciada e não terminou).
pub fn can_show_diff(task: &Task) -> bool {
    task.started_ms.is_some() && matches!(task.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Failed)
}

/// Troca a tarefa de mesmo id na lista (ou a acrescenta).
pub fn upsert(tasks: &mut Vec<Task>, task: Task) {
    match tasks.iter_mut().find(|t| t.id == task.id) {
        Some(slot) => *slot = task,
        None => tasks.push(task),
    }
}

/// Data e hora locais de um evento do histórico (`17/10 14:32`).
pub fn event_time(at_ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(at_ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format("%d/%m %H:%M").to_string(),
        chrono::LocalResult::None => String::new(),
    }
}

/// Texto cortado em `max` caracteres, com reticências e o aviso (relatórios e pedidos podem ter 200 mil).
pub fn clip_text(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(max) {
        Some((i, _)) => {
            std::borrow::Cow::Owned(format!("{}\n\n[… texto cortado na tela: {} de {} caracteres]", &text[..i], max, text.chars().count()))
        }
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// Histórico da tarefa, do mais novo para o mais antigo.
pub fn events_newest_first(task: &Task) -> Vec<&TaskEvent> {
    let mut list: Vec<&TaskEvent> = task.events.iter().collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    list
}

// ---------------------------------------------------------------------------
// Estado
// ---------------------------------------------------------------------------

/// O que a aba mostra.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TasksView {
    #[default]
    List,
    /// Detalhe da tarefa (id).
    Detail(String),
    /// Formulário de nova tarefa.
    New,
}

/// Seções recolhíveis do detalhe.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Brief,
    Report,
    Changes,
    History,
}

/// Ação perigosa esperando o "sim" no painel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Merge(String),
    Discard(String),
}

/// Diff completo aberto no lugar do detalhe.
#[derive(Debug, Clone)]
pub struct OpenDiff {
    pub id: String,
    /// `None` = carregando.
    pub diff: Option<Result<GitDiff, String>>,
}

/// Uma operação do ciclo de vida em andamento (botões desligados, rodinha girando).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub id: String,
    pub label: &'static str,
}

/// Menu aberto do formulário.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormMenu {
    Agent,
    Model,
}

/// Formulário "Nova tarefa".
#[derive(Debug, Clone)]
pub struct NewForm {
    pub title: String,
    pub brief: String,
    /// Arquivos anexados ao pedido (caminhos absolutos em `dados/anexos`).
    pub attachments: Vec<PathBuf>,
    /// Agente (`claude`); vazio até o formulário abrir.
    pub kind: String,
    /// `None` = "Padrão".
    pub model: Option<String>,
    /// Provedor do passo do menu de modelo (`opencode`): `None` = a lista de provedores aparece
    /// primeiro. Só vale enquanto o menu está aberto (o OpenCode mistura vários provedores).
    pub provider: Option<String>,
    pub base: String,
    pub start_now: bool,
    pub error: Option<String>,
    /// Criando (o botão fica desligado).
    pub busy: bool,
    pub menu: Option<FormMenu>,
    /// O título pede o foco do teclado no próximo quadro (o formulário acabou de abrir).
    pub focus_title: bool,
}

impl Default for NewForm {
    fn default() -> Self {
        Self {
            title: String::new(),
            brief: String::new(),
            attachments: Vec::new(),
            kind: String::new(),
            model: None,
            provider: None,
            base: String::new(),
            start_now: true,
            error: None,
            busy: false,
            menu: None,
            focus_title: false,
        }
    }
}

/// Estado da aba Tarefas no `Deck`.
pub struct TasksUi {
    pub view: TasksView,
    pub tasks: Vec<Task>,
    /// A primeira leitura já chegou.
    pub loaded: bool,
    pub refreshing: bool,
    /// Pediram outra leitura enquanto uma estava em andamento.
    dirty: bool,
    pub collapsed: HashSet<Group>,
    pub closed_sections: HashSet<Section>,
    pub diff: Option<OpenDiff>,
    pub confirm: Option<Confirm>,
    /// O campo de feedback do "Devolver" (em revisão) ou do "Reprovar" (aguardando validação) está aberto.
    pub return_open: bool,
    pub feedback: String,
    pub busy: Option<Busy>,
    /// Erro da última ação (faixa vermelha no detalhe).
    pub notice: Option<String>,
    pub form: NewForm,
    /// Muda a cada troca de projeto: respostas de pedidos do projeto anterior são descartadas.
    pub generation: u64,
    /// SOMENTE DEPURAÇÃO: estado fictício que as releituras não substituem.
    pub debug_hold: bool,
}

impl Default for TasksUi {
    fn default() -> Self {
        Self {
            view: TasksView::List,
            tasks: Vec::new(),
            loaded: false,
            refreshing: false,
            dirty: false,
            // As concluídas e as canceladas já foram resolvidas: começam recolhidas.
            collapsed: HashSet::from([Group::Done, Group::Cancelled]),
            closed_sections: HashSet::new(),
            diff: None,
            confirm: None,
            return_open: false,
            feedback: String::new(),
            busy: None,
            notice: None,
            form: NewForm::default(),
            generation: 0,
            debug_hold: false,
        }
    }
}

impl TasksUi {
    pub fn task(&self, id: &str) -> Option<&Task> {
        self.tasks.iter().find(|t| t.id == id)
    }

    pub fn review_count(&self) -> usize {
        review_count(&self.tasks)
    }

    pub fn validating_count(&self) -> usize {
        validating_count(&self.tasks)
    }

    /// O número do selo do ícone da aba: o que espera uma decisão do humano (revisar ou validar).
    pub fn attention_count(&self) -> usize {
        self.review_count() + self.validating_count()
    }
}

/// O que uma operação do ciclo de vida faz.
#[derive(Debug, Clone)]
pub enum TaskOp {
    Start,
    Merge,
    Validate,
    Discard,
    Return(String),
    Reject(String),
}

impl TaskOp {
    fn busy_label(&self) -> &'static str {
        match self {
            TaskOp::Start => "Iniciando…",
            TaskOp::Merge => "Mesclando…",
            TaskOp::Validate => "Validando…",
            TaskOp::Discard => "Descartando…",
            TaskOp::Return(_) => "Devolvendo…",
            TaskOp::Reject(_) => "Reprovando…",
        }
    }
}

/// Respostas das chamadas em segundo plano.
pub enum TaskReply {
    List(Vec<Task>),
    Created {
        result: Box<Result<Task, TaskError>>,
        /// O resultado de iniciar logo em seguida (só com "Iniciar agora").
        started: Option<Box<Result<Task, TaskError>>>,
    },
    Done {
        id: String,
        op: TaskOp,
        result: Box<Result<Task, TaskError>>,
    },
    Diff {
        id: String,
        result: Result<GitDiff, TaskError>,
    },
}

// ---------------------------------------------------------------------------
// Ações
// ---------------------------------------------------------------------------

impl Deck {
    /// O projeto da tela mudou: esquece a lista, o detalhe e o formulário do anterior e ignora as
    /// respostas que ainda vierem dele. Os grupos recolhidos ficam.
    pub fn tasks_reset_for_project(&mut self) {
        let generation = self.tasks.generation + 1;
        let collapsed = std::mem::take(&mut self.tasks.collapsed);
        self.tasks = TasksUi { generation, collapsed, ..TasksUi::default() };
    }

    /// Relê a lista (uma por vez; um pedido no meio repete a leitura ao terminar).
    pub fn tasks_refresh(&mut self) {
        if self.tasks.debug_hold || self.active_project.is_empty() {
            return;
        }
        if self.tasks.refreshing {
            self.tasks.dirty = true;
            return;
        }
        self.tasks.refreshing = true;
        let (generation, project) = (self.tasks.generation, self.active_project.clone());
        self.spawn_bg("tasks-list", move |core| Some(UiMsg::Tasks(generation, TaskReply::List(core.tasks(&project)))));
    }

    /// O core gravou as tarefas de `project`.
    pub fn tasks_changed(&mut self, project: &str) {
        if project == self.active_project {
            self.tasks_refresh();
        }
    }

    pub fn tasks_open_detail(&mut self, id: &str) {
        self.tasks.view = TasksView::Detail(id.to_owned());
        self.tasks_clear_detail_state();
        self.tasks.notice = None;
    }

    fn tasks_clear_detail_state(&mut self) {
        self.tasks.diff = None;
        self.tasks.confirm = None;
        self.tasks.return_open = false;
        self.tasks.feedback.clear();
    }

    /// "←": do diff ao detalhe, do detalhe e do formulário à lista.
    pub fn tasks_back(&mut self) {
        if self.tasks.diff.take().is_some() {
            return;
        }
        self.tasks.view = TasksView::List;
        self.tasks_clear_detail_state();
        self.tasks.notice = None;
        self.tasks.form.menu = None;
        self.tasks.form.provider = None;
    }

    /// Agentes que uma tarefa pode usar: instalados, com aba (nada de terminal comum).
    pub fn tasks_agent_choices(&self) -> Vec<String> {
        self.agents.available.iter().filter(|a| a.installed && a.peer && a.id != "shell").map(|a| a.id.clone()).collect()
    }

    /// Abre o formulário: agente padrão `claude` (ou o primeiro disponível) e a branch atual de base.
    pub fn tasks_open_new(&mut self) {
        let choices = self.tasks_agent_choices();
        let form = &mut self.tasks.form;
        form.menu = None;
        form.provider = None;
        form.error = None;
        if form.kind.is_empty() || (!choices.is_empty() && !choices.contains(&form.kind)) {
            form.kind = if choices.iter().any(|k| k == "claude") {
                "claude".into()
            } else {
                choices.first().cloned().unwrap_or_else(|| "claude".into())
            };
            form.model = None;
        }
        if form.base.trim().is_empty() {
            form.base = self.git.status().and_then(|st| st.branch.clone()).unwrap_or_default();
        }
        let kind = self.tasks.form.kind.clone();
        self.request_models(&kind);
        self.tasks.form.focus_title = true;
        self.tasks.view = TasksView::New;
        self.tasks.notice = None;
    }

    /// Escolheu outro agente no formulário: o modelo volta ao padrão.
    pub fn tasks_form_set_kind(&mut self, kind: String) {
        if self.tasks.form.kind != kind {
            self.tasks.form.model = None;
            self.tasks.form.kind = kind.clone();
        }
        self.tasks.form.menu = None;
        self.tasks.form.provider = None;
        self.request_models(&kind);
    }

    /// Botão "Anexar arquivos" do formulário "Nova tarefa".
    pub fn tasks_pick_attachments(&self) {
        self.pick_attachments_for(AttachTarget::TaskForm(self.active_project.clone()));
    }

    /// "×" do chip de anexo no formulário "Nova tarefa": tira da lista e apaga a cópia em `dados/anexos`.
    pub fn tasks_remove_attachment(&mut self, i: usize) {
        if i >= self.tasks.form.attachments.len() {
            return;
        }
        let path = self.tasks.form.attachments.remove(i);
        self.delete_attachment_copy(path);
    }

    /// Cria a tarefa (e a inicia, com "Iniciar agora") numa thread.
    pub fn tasks_submit_form(&mut self) {
        let f = &mut self.tasks.form;
        if f.busy || f.title.trim().is_empty() || self.active_project.is_empty() {
            return;
        }
        f.busy = true;
        f.error = None;
        f.menu = None;
        f.provider = None;
        let spec = TaskSpec {
            title: f.title.clone(),
            brief: crate::util::append_brief_attachments(&f.brief, &f.attachments),
            kind: f.kind.clone(),
            model: f.model.clone(),
            base: Some(f.base.trim().to_owned()).filter(|b| !b.is_empty()),
            creator: None,
        };
        let start_now = f.start_now;
        let (generation, project) = (self.tasks.generation, self.active_project.clone());
        self.spawn_bg("task-create", move |core| {
            let result = Box::new(core.task_create(&project, spec));
            let started = match (&*result, start_now) {
                (Ok(task), true) => Some(Box::new(core.task_start(&project, &task.id))),
                _ => None,
            };
            Some(UiMsg::Tasks(generation, TaskReply::Created { result, started }))
        });
    }

    /// Roda uma operação do ciclo de vida numa thread.
    pub fn tasks_run(&mut self, id: String, op: TaskOp) {
        if self.tasks.busy.is_some() || self.active_project.is_empty() {
            return;
        }
        self.tasks.busy = Some(Busy { id: id.clone(), label: op.busy_label() });
        self.tasks.notice = None;
        self.tasks.confirm = None;
        let (generation, project) = (self.tasks.generation, self.active_project.clone());
        self.spawn_bg("task-op", move |core| {
            let result = Box::new(match &op {
                TaskOp::Start => core.task_start(&project, &id),
                TaskOp::Merge => core.task_merge(&project, &id),
                TaskOp::Validate => core.task_validate(&project, &id),
                TaskOp::Discard => core.task_discard(&project, &id),
                TaskOp::Return(feedback) => core.task_return(&project, &id, feedback),
                TaskOp::Reject(feedback) => core.task_reject(&project, &id, feedback),
            });
            Some(UiMsg::Tasks(generation, TaskReply::Done { id, op, result }))
        });
    }

    /// Clique num botão do detalhe.
    pub fn tasks_act(&mut self, id: &str, act: TaskAct) {
        let Some(task) = self.tasks.task(id).cloned() else { return };
        if act.needs_confirm(&task) {
            self.tasks.confirm =
                Some(if act == TaskAct::Discard { Confirm::Discard(id.to_owned()) } else { Confirm::Merge(id.to_owned()) });
            return;
        }
        match act {
            TaskAct::Start => self.tasks_run(id.to_owned(), TaskOp::Start),
            TaskAct::Retry => self.tasks_run(id.to_owned(), TaskOp::Start),
            TaskAct::Merge => self.tasks_run(id.to_owned(), TaskOp::Merge),
            TaskAct::Validate => self.tasks_run(id.to_owned(), TaskOp::Validate),
            TaskAct::Discard => self.tasks_run(id.to_owned(), TaskOp::Discard),
            TaskAct::Return | TaskAct::Reject => {
                self.tasks.return_open = !self.tasks.return_open;
            }
            TaskAct::GoToTab | TaskAct::ReopenTab => {
                if let Err(e) = self.open_task_tab(&task) {
                    self.tasks.notice = Some(e);
                }
            }
        }
    }

    /// O "sim" do painel de confirmação.
    pub fn tasks_confirm_accepted(&mut self) {
        match self.tasks.confirm.take() {
            Some(Confirm::Merge(id)) => self.tasks_run(id, TaskOp::Merge),
            Some(Confirm::Discard(id)) => self.tasks_run(id, TaskOp::Discard),
            None => {}
        }
    }

    /// Envia o feedback do formulário: "Devolver" (em revisão) ou "Reprovar" (aguardando validação).
    pub fn tasks_send_return(&mut self, id: &str) {
        let feedback = self.tasks.feedback.trim().to_owned();
        let Some(task) = self.tasks.task(id) else { return };
        if feedback.is_empty() {
            return;
        }
        let op = if task.status == TaskStatus::Validating { TaskOp::Reject(feedback) } else { TaskOp::Return(feedback) };
        self.tasks_run(id.to_owned(), op);
    }

    /// Abre o diff completo da tarefa no lugar do detalhe.
    pub fn tasks_open_diff(&mut self, id: &str) {
        self.tasks.diff = Some(OpenDiff { id: id.to_owned(), diff: None });
        if self.tasks.debug_hold || self.active_project.is_empty() {
            return;
        }
        let (generation, project, id) = (self.tasks.generation, self.active_project.clone(), id.to_owned());
        self.spawn_bg("task-diff", move |core| {
            let result = core.task_diff(&project, &id);
            Some(UiMsg::Tasks(generation, TaskReply::Diff { id, result }))
        });
    }

    /// A aba do agente da tarefa está aberta?
    pub fn tasks_tab_open(&self, task: &Task) -> bool {
        self.tabs.iter().any(|t| t.project == self.active_project && t.ident.id() == task.instance)
    }

    pub fn on_tasks(&mut self, generation: u64, reply: TaskReply) {
        if generation != self.tasks.generation || self.tasks.debug_hold {
            return; // pedida no projeto anterior (ou estado fictício de captura de tela)
        }
        match reply {
            TaskReply::List(list) => {
                self.tasks.refreshing = false;
                self.tasks.loaded = true;
                self.tasks.tasks = list;
                if std::mem::take(&mut self.tasks.dirty) {
                    self.tasks_refresh();
                }
                self.tasks_drop_stale_view();
            }
            TaskReply::Created { result, started } => {
                self.tasks.form.busy = false;
                match *result {
                    Err(e) => self.tasks.form.error = Some(e.message().to_owned()),
                    Ok(task) => {
                        let id = task.id.clone();
                        match started.map(|s| *s) {
                            Some(Ok(running)) => upsert(&mut self.tasks.tasks, running),
                            Some(Err(e)) => {
                                upsert(&mut self.tasks.tasks, task);
                                self.tasks.notice = Some(e.message().to_owned());
                            }
                            None => upsert(&mut self.tasks.tasks, task),
                        }
                        // Pronto: o formulário volta vazio (agente, base e "Iniciar agora" ficam).
                        let form = &mut self.tasks.form;
                        form.title.clear();
                        form.brief.clear();
                        form.attachments.clear();
                        form.error = None;
                        let notice = self.tasks.notice.take();
                        self.tasks_open_detail(&id);
                        self.tasks.notice = notice;
                        self.tasks_refresh();
                    }
                }
            }
            TaskReply::Done { id, op, result } => {
                if self.tasks.busy.as_ref().is_some_and(|b| b.id == id) {
                    self.tasks.busy = None;
                }
                match *result {
                    Ok(task) => {
                        upsert(&mut self.tasks.tasks, task);
                        self.tasks.notice = None;
                        if matches!(op, TaskOp::Return(_) | TaskOp::Reject(_)) {
                            self.tasks.return_open = false;
                            self.tasks.feedback.clear();
                        }
                        // Mesclada, a branch é a própria base; validada ou descartada, ela some: o diff aberto não vale mais.
                        if matches!(op, TaskOp::Merge | TaskOp::Validate | TaskOp::Discard) {
                            self.tasks.diff = None;
                        }
                    }
                    Err(e) => self.tasks.notice = Some(e.message().to_owned()),
                }
                self.tasks_refresh();
            }
            TaskReply::Diff { id, result } => {
                if let Some(open) = self.tasks.diff.as_mut().filter(|d| d.id == id) {
                    open.diff = Some(result.map_err(|e| e.message().to_owned()));
                }
            }
        }
    }

    /// O detalhe aberto aponta para uma tarefa que sumiu da lista: volta à lista.
    fn tasks_drop_stale_view(&mut self) {
        if let TasksView::Detail(id) = &self.tasks.view {
            if self.tasks.task(id).is_none() {
                self.tasks.view = TasksView::List;
                self.tasks_clear_detail_state();
            }
        }
    }

    /// Chamado quando a barra direita passa a mostrar a aba Tarefas.
    pub fn tasks_on_open(&mut self) {
        self.tasks_refresh();
    }
}

// ---------------------------------------------------------------------------
// Estados fictícios (capturas de tela, `COMMANDDECK_DEBUG_UI=tasks-*`)
// ---------------------------------------------------------------------------

/// Uma tarefa de cada estado, com dados realistas.
pub fn demo_tasks() -> Vec<Task> {
    let now = commanddeck_core::util::now_ms();
    let min = 60_000u64;
    let ev = |ago_min: u64, text: &str| TaskEvent { at_ms: now - ago_min * min, text: text.to_owned() };
    let base = |id: &str, title: &str, status: TaskStatus, kind: &str, model: Option<&str>| Task {
        id: id.to_owned(),
        title: title.to_owned(),
        status,
        base: "dev".into(),
        branch: format!("tarefa/{id}-{}", commanddeck_core::tasks::slugify(title, 40)),
        worktree: format!(r"C:\Users\voce\commanddeck-dev\.agentdeck\worktrees\{id}").into(),
        kind: kind.to_owned(),
        model: model.map(str::to_owned),
        instance: format!("{kind}@{id}"),
        ..Task::default()
    };

    let review = Task {
        brief: "Mostrar na barra direita uma aba **Tarefas** com a lista agrupada por estado.\n\n- cartão com o título, o agente e o tempo\n- detalhe com o relatório e o diff\n- botões de mesclar, devolver e descartar".into(),
        created_ms: now - 95 * min,
        started_ms: Some(now - 90 * min),
        finished_ms: Some(now - 6 * min),
        report: Some("Fiz a aba Tarefas completa.\n\nArquivos: crates/app/src/ui/tasks.rs (pintura), crates/app/src/tasks_ui.rs (estado e ações), right_panel.rs (ícone e badge).\n\nTestes: cargo test --workspace passa; as funções puras (grupos, resumo do diff, ações por estado) têm testes.\n\nFalta: conferir a aba no Windows com várias tarefas ao mesmo tempo.".into()),
        diff_stat: Some(" crates/app/src/tasks_ui.rs       | 412 +++++++++++++++++++++++++++\n crates/app/src/ui/tasks.rs      | 688 ++++++++++++++++++++++++++++++++++++\n crates/app/src/ui/right_panel.rs |  14 +-\n crates/core/src/tasks.rs        |  23 ++\n 4 files changed, 1129 insertions(+), 8 deletions(-)\n".into()),
        events: vec![
            ev(95, "criada"),
            ev(90, "iniciada: worktree C:\\Users\\voce\\commanddeck-dev\\.agentdeck\\worktrees\\t4 na branch tarefa/t4-aba-tarefas-na-barra-direita"),
            ev(90, "brief entregue ao agente"),
            ev(6, "relatório recebido; em revisão"),
        ],
        ..base("t4", "Aba Tarefas na barra direita", TaskStatus::Review, "claude", Some("claude-opus-4-5"))
    };
    let review2 = Task {
        created_ms: now - 50 * min,
        started_ms: Some(now - 48 * min),
        finished_ms: Some(now - 2 * 60 * min),
        report: Some("Corrigi o vazamento de handle no PTY.".into()),
        diff_stat: Some(" crates/app/src/terminal/mod.rs | 6 +++---\n 1 file changed, 3 insertions(+), 3 deletions(-)\n".into()),
        events: vec![ev(150, "relatório recebido; em revisão")],
        ..base("t5", "Corrigir vazamento de handle ao fechar a aba", TaskStatus::Review, "antigravity", Some("gemini-3-pro"))
    };
    let running = Task {
        created_ms: now - 25 * min,
        started_ms: Some(now - 24 * min),
        events: vec![ev(25, "criada"), ev(24, "iniciada: worktree … na branch tarefa/t6")],
        ..base("t6", "Documentar as rotas /tasks da ponte", TaskStatus::Running, "claude", Some("sonnet"))
    };
    let draft = Task {
        brief: "Rodar o clippy com -D warnings e corrigir o que aparecer.".into(),
        created_ms: now - 3 * 60 * min,
        events: vec![ev(180, "criada")],
        ..base("t7", "Limpar os avisos do clippy no crate app", TaskStatus::Draft, "ollama:qwen3.5:9b", None)
    };
    let failed = Task {
        created_ms: now - 5 * 60 * min,
        error: Some("não consegui criar a worktree: a branch tarefa/t3-nova-branch já existe neste repositório".into()),
        events: vec![
            ev(300, "criada"),
            ev(299, "erro: não consegui criar a worktree: a branch tarefa/t3-nova-branch já existe neste repositório"),
        ],
        ..base("t3", "Trocar o parser de markdown do relatório", TaskStatus::Failed, "opencode", Some("opencode/glm-5.3"))
    };
    let validating = Task {
        brief: "Mostrar o estado da mesclagem no cabeçalho do detalhe.".into(),
        created_ms: now - 3 * 60 * min,
        started_ms: Some(now - 170 * min),
        finished_ms: Some(now - 8 * min),
        report: Some(
            "Cabeçalho do detalhe mostra a mesclagem.\n\nArquivos: crates/app/src/ui/tasks.rs.\n\nTestes: cargo test --workspace passa."
                .into(),
        ),
        diff_stat: Some(" crates/app/src/ui/tasks.rs | 18 ++++++++++++++----\n 1 file changed, 14 insertions(+), 4 deletions(-)\n".into()),
        events: vec![
            ev(180, "criada"),
            ev(170, "iniciada: worktree … na branch tarefa/t8"),
            ev(20, "relatório recebido; em revisão"),
            ev(8, "mesclada em dev (a1b2c3d); aguardando validação"),
        ],
        ..base("t8", "Estado da mesclagem no cabeçalho do detalhe", TaskStatus::Validating, "claude", Some("sonnet"))
    };
    let merged = Task {
        created_ms: now - 26 * 60 * min,
        started_ms: Some(now - 26 * 60 * min),
        finished_ms: Some(now - 24 * 60 * min),
        events: vec![ev(24 * 60, "validada; concluída; worktree e branch removidas")],
        ..base("t2", "Rotas /tasks na ponte", TaskStatus::Merged, "claude", Some("opus"))
    };
    let discarded = Task {
        created_ms: now - 50 * 60 * min,
        finished_ms: Some(now - 48 * 60 * min),
        events: vec![ev(48 * 60, "descartada; worktree e branch removidas")],
        ..base("t1", "Experimento com cache de galleys", TaskStatus::Discarded, "claude", None)
    };
    vec![discarded, merged, failed, review, running, draft, review2, validating]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(id: &str, status: TaskStatus, activity: u64) -> Task {
        Task { id: id.into(), status, created_ms: 1, events: vec![TaskEvent { at_ms: activity, text: "x".into() }], ..Task::default() }
    }

    #[test]
    fn tasks_are_grouped_in_order_and_sorted_by_recent_activity() {
        let tasks = vec![
            task("t1", TaskStatus::Merged, 100),
            task("t2", TaskStatus::Running, 300),
            task("t3", TaskStatus::Review, 200),
            task("t4", TaskStatus::Review, 900),
            task("t5", TaskStatus::Discarded, 500),
            task("t6", TaskStatus::Failed, 50),
            task("t7", TaskStatus::Draft, 10),
        ];
        let groups = group_tasks(&tasks);
        let shape: Vec<(Group, Vec<&str>)> = groups.iter().map(|(g, l)| (*g, l.iter().map(|t| t.id.as_str()).collect())).collect();
        assert_eq!(
            shape,
            vec![
                (Group::Review, vec!["t4", "t3"]),
                (Group::Running, vec!["t2"]),
                (Group::Draft, vec!["t7"]),
                (Group::Failed, vec!["t6"]),
                (Group::Done, vec!["t1"]),
                (Group::Cancelled, vec!["t5"]),
            ]
        );
        // Grupos vazios não aparecem.
        assert_eq!(group_tasks(&tasks[..1]).len(), 1);
        assert!(group_tasks(&[]).is_empty());
        assert_eq!(review_count(&tasks), 2);
        assert_eq!(validating_count(&tasks), 0);
    }

    #[test]
    fn awaiting_validation_sits_between_review_and_the_rest() {
        let tasks = vec![
            task("t1", TaskStatus::Merged, 100),
            task("t2", TaskStatus::Validating, 300),
            task("t3", TaskStatus::Review, 200),
            task("t4", TaskStatus::Running, 400),
            task("t5", TaskStatus::Validating, 500),
        ];
        let shape: Vec<(Group, Vec<&str>)> =
            group_tasks(&tasks).iter().map(|(g, l)| (*g, l.iter().map(|t| t.id.as_str()).collect())).collect();
        assert_eq!(
            shape,
            vec![
                (Group::Review, vec!["t3"]),
                (Group::Validating, vec!["t5", "t2"]),
                (Group::Running, vec!["t4"]),
                (Group::Done, vec!["t1"]),
            ]
        );
        assert_eq!(Group::Validating.title(), "Aguardando validação");
        assert_eq!(Group::of(TaskStatus::Validating), Group::Validating);
        assert_eq!(status_name(TaskStatus::Validating), "Aguardando validação");
        // Concluída é o nome da tarefa validada (o estado gravado continua `Merged`).
        assert_eq!(status_name(TaskStatus::Merged), "Concluída");
        // Os grupos que esperam o humano contam no selo; as validadas não.
        let ui = TasksUi { tasks, ..TasksUi::default() };
        assert_eq!((ui.review_count(), ui.validating_count(), ui.attention_count()), (1, 2, 3));
        // Grupo "Aguardando validação" começa aberto: é trabalho para você.
        assert!(!ui.collapsed.contains(&Group::Validating));
    }

    #[test]
    fn cancelled_tasks_are_apart_from_the_merged_ones() {
        assert_eq!(Group::of(TaskStatus::Merged), Group::Done);
        assert_eq!(Group::of(TaskStatus::Discarded), Group::Cancelled);
        assert_eq!(Group::Done.title(), "Concluídas");
        assert_eq!(Group::Cancelled.title(), "Canceladas");
        // Os dois no fim, nessa ordem.
        let n = Group::ORDER.len();
        assert_eq!(Group::ORDER[n - 2..], [Group::Done, Group::Cancelled]);
        // Os grupos resolvidos começam recolhidos.
        let ui = TasksUi::default();
        assert!(ui.collapsed.contains(&Group::Done));
        assert!(ui.collapsed.contains(&Group::Cancelled));
        assert!(!ui.collapsed.contains(&Group::Review));
    }

    #[test]
    fn the_real_list_with_two_discarded_keeps_them_out_of_the_merged_group() {
        // O caso que apareceu na revisão: t8 e t4 descartadas convivendo com t1 e t2 mescladas.
        let tasks = vec![
            task("t1", TaskStatus::Merged, 100),
            task("t2", TaskStatus::Merged, 200),
            task("t3", TaskStatus::Review, 300),
            task("t4", TaskStatus::Discarded, 400),
            task("t5", TaskStatus::Running, 500),
            task("t6", TaskStatus::Review, 600),
            task("t7", TaskStatus::Running, 700),
            task("t8", TaskStatus::Discarded, 800),
        ];
        let groups = group_tasks(&tasks);
        let ids = |g: Group| -> Vec<&str> {
            groups.iter().find(|(group, _)| *group == g).map(|(_, l)| l.iter().map(|t| t.id.as_str()).collect()).unwrap_or_default()
        };
        assert_eq!(ids(Group::Done), ["t2", "t1"]);
        assert_eq!(ids(Group::Cancelled), ["t8", "t4"]);
        // Nenhuma descartada pode sobrar em "Concluídas".
        let done = groups.iter().find(|(g, _)| *g == Group::Done).expect("concluídas");
        assert!(done.1.iter().all(|t| t.status == TaskStatus::Merged));
    }

    #[test]
    fn ties_in_activity_put_the_newest_number_first() {
        let tasks = vec![task("t2", TaskStatus::Draft, 5), task("t10", TaskStatus::Draft, 5), task("t9", TaskStatus::Draft, 5)];
        let ids: Vec<&str> = group_tasks(&tasks)[0].1.iter().map(|t| t.id.as_str()).collect();
        assert_eq!(ids, ["t10", "t9", "t2"]);
    }

    #[test]
    fn the_diff_stat_becomes_a_one_line_summary() {
        let stat = " a.rs | 10 +++++++---\n b.rs | 2 +-\n 3 files changed, 120 insertions(+), 8 deletions(-)\n";
        let s = parse_diff_stat(stat).expect("resumo");
        assert_eq!(s, DiffSummary { files: 3, insertions: 120, deletions: 8 });
        assert_eq!(s.text(), "3 arquivos, +120 −8");
        // Singular e só um dos lados.
        assert_eq!(parse_diff_stat(" a.rs | 1 +\n 1 file changed, 1 insertion(+)\n").map(|s| s.text()).as_deref(), Some("1 arquivo, +1"));
        assert_eq!(parse_diff_stat(" 2 files changed, 4 deletions(-)").map(|s| s.text()).as_deref(), Some("2 arquivos, −4"));
        // Sem diferenças (texto do core) e lixo.
        assert_eq!(parse_diff_stat("(nenhuma alteração em relação à base)").map(|s| s.text()).as_deref(), Some("sem alterações"));
        assert_eq!(parse_diff_stat(""), None);
        assert_eq!(parse_diff_stat("alguma outra coisa"), None);
    }

    #[test]
    fn the_stat_lists_one_line_per_file() {
        let stat = " crates/app/src/tasks_ui.rs | 412 +++++++++\n a b/c.rs => d.rs |  3 +-\n logo.png | Bin 0 -> 1234 bytes\n 3 files changed, 415 insertions(+), 1 deletion(-)\n";
        let lines = parse_stat_lines(stat);
        let shape: Vec<(&str, &str)> = lines.iter().map(|l| (l.path.as_str(), l.changes.as_str())).collect();
        assert_eq!(shape, [("crates/app/src/tasks_ui.rs", "412"), ("a b/c.rs => d.rs", "3"), ("logo.png", "Bin 0 -> 1234 bytes")]);
        assert!(parse_stat_lines("(nenhuma alteração em relação à base)").is_empty());
        assert!(parse_stat_lines("").is_empty());
    }

    #[test]
    fn ages_read_naturally() {
        let now = 10_000_000_000;
        assert_eq!(age_text(now, now), "agora");
        assert_eq!(age_text(now, now + 5_000), "agora");
        assert_eq!(age_text(now, now - 59_000), "agora");
        assert_eq!(age_text(now, now - 60_000), "há 1 min");
        assert_eq!(age_text(now, now - 59 * 60_000), "há 59 min");
        assert_eq!(age_text(now, now - 3_600_000), "há 1 h");
        assert_eq!(age_text(now, now - 23 * 3_600_000), "há 23 h");
        assert_eq!(age_text(now, now - 3 * 86_400_000), "há 3 d");
    }

    #[test]
    fn the_status_line_says_what_happened_and_when() {
        let now = 1_000_000_000;
        let mut t = task("t1", TaskStatus::Review, now - 1000);
        t.finished_ms = Some(now - 3 * 60_000);
        assert_eq!(status_age(&t, now), "pronta há 3 min");
        t.status = TaskStatus::Running;
        t.started_ms = Some(now - 2 * 3_600_000);
        assert_eq!(status_age(&t, now), "iniciada há 2 h");
        t.status = TaskStatus::Draft;
        t.created_ms = now - 10_000;
        assert_eq!(status_age(&t, now), "criada agora");
        t.status = TaskStatus::Failed;
        t.created_ms = now - 3_600_000;
        t.events = vec![TaskEvent { at_ms: now - 120_000, text: "erro".into() }];
        assert_eq!(status_age(&t, now), "falhou há 2 min");
        t.status = TaskStatus::Validating;
        t.finished_ms = Some(now - 5 * 60_000);
        assert_eq!(status_age(&t, now), "mesclada há 5 min");
        t.status = TaskStatus::Merged;
        assert_eq!(status_age(&t, now), "concluída há 5 min");
    }

    #[test]
    fn models_are_shown_without_the_provider() {
        let mut t = Task::default();
        assert_eq!(model_text(&t), None);
        t.model = Some("opencode/glm-5.3".into());
        assert_eq!(model_text(&t), Some("glm-5.3"));
        t.model = Some("opus".into());
        assert_eq!(model_text(&t), Some("opus"));
    }

    #[test]
    fn each_status_has_its_buttons() {
        let t = |status, report: bool| Task { status, base: "dev".into(), report: report.then(|| "feito".to_owned()), ..Task::default() };
        assert_eq!(actions_for(&t(TaskStatus::Draft, false), false), [TaskAct::Start, TaskAct::Discard]);
        assert_eq!(actions_for(&t(TaskStatus::Running, false), true), [TaskAct::GoToTab, TaskAct::Discard]);
        assert_eq!(actions_for(&t(TaskStatus::Running, false), false), [TaskAct::ReopenTab, TaskAct::Discard]);
        assert_eq!(actions_for(&t(TaskStatus::Review, true), true), [TaskAct::Merge, TaskAct::Return, TaskAct::GoToTab, TaskAct::Discard]);
        assert_eq!(
            actions_for(&t(TaskStatus::Validating, true), true),
            [TaskAct::Validate, TaskAct::Reject, TaskAct::GoToTab, TaskAct::Discard]
        );
        assert_eq!(
            actions_for(&t(TaskStatus::Validating, true), false),
            [TaskAct::Validate, TaskAct::Reject, TaskAct::ReopenTab, TaskAct::Discard]
        );
        assert_eq!(actions_for(&t(TaskStatus::Failed, false), false), [TaskAct::Retry, TaskAct::Discard]);
        assert!(actions_for(&t(TaskStatus::Merged, true), false).is_empty());
        assert!(actions_for(&t(TaskStatus::Discarded, false), false).is_empty());
        // Só a ação principal vem cheia; mesclar mostra a base.
        assert!(TaskAct::Merge.is_primary() && !TaskAct::Discard.is_primary() && !TaskAct::Return.is_primary());
        assert!(TaskAct::Validate.is_primary() && !TaskAct::Reject.is_primary());
        assert_eq!(
            (TaskAct::Validate.label(&t(TaskStatus::Validating, true)), TaskAct::Reject.label(&t(TaskStatus::Validating, true))),
            ("Validar".to_owned(), "Reprovar".to_owned())
        );
        assert_eq!(TaskAct::Merge.label(&t(TaskStatus::Review, true)), "Mesclar em dev");
    }

    #[test]
    fn destructive_actions_ask_first() {
        let review = Task { status: TaskStatus::Review, report: Some("x".into()), ..Task::default() };
        assert!(TaskAct::Discard.needs_confirm(&review) && TaskAct::Merge.needs_confirm(&review));
        assert!(!TaskAct::Return.needs_confirm(&review) && !TaskAct::Start.needs_confirm(&review));
        assert!(!TaskAct::Validate.needs_confirm(&review) && !TaskAct::Reject.needs_confirm(&review));
        assert!(!TaskAct::GoToTab.needs_confirm(&review) && !TaskAct::ReopenTab.needs_confirm(&review));
        // Tentar de novo: iniciar não pergunta; mesclar (já tem relatório) pergunta.
        let failed_start = Task { status: TaskStatus::Failed, ..Task::default() };
        let failed_merge = Task { status: TaskStatus::Failed, report: Some("x".into()), ..Task::default() };
        assert!(!TaskAct::Retry.needs_confirm(&failed_start));
        assert!(TaskAct::Retry.needs_confirm(&failed_merge));
    }

    #[test]
    fn the_diff_is_available_only_while_the_branch_exists() {
        let t = |status, started: bool| Task { status, started_ms: started.then_some(1), ..Task::default() };
        assert!(can_show_diff(&t(TaskStatus::Review, true)));
        assert!(can_show_diff(&t(TaskStatus::Running, true)));
        assert!(can_show_diff(&t(TaskStatus::Failed, true)));
        assert!(!can_show_diff(&t(TaskStatus::Failed, false)));
        assert!(!can_show_diff(&t(TaskStatus::Draft, false)));
        assert!(!can_show_diff(&t(TaskStatus::Merged, true)));
        assert!(!can_show_diff(&t(TaskStatus::Validating, true)), "mesclada: a branch é a própria base, o diff é vazio");
        assert!(!can_show_diff(&t(TaskStatus::Discarded, true)));
    }

    #[test]
    fn upsert_replaces_by_id() {
        let mut list = vec![task("t1", TaskStatus::Draft, 1)];
        upsert(&mut list, task("t1", TaskStatus::Running, 2));
        upsert(&mut list, task("t2", TaskStatus::Draft, 3));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].status, TaskStatus::Running);
    }

    #[test]
    fn long_texts_are_clipped_for_the_screen() {
        assert_eq!(clip_text("curto", 10), "curto");
        assert_eq!(clip_text("exato", 5), "exato");
        let clipped = clip_text("ação longa demais", 4);
        assert!(clipped.starts_with("ação\n\n[… texto cortado"), "{clipped}");
        assert!(clipped.contains("4 de 17"), "{clipped}");
        // O horário local tem a forma dd/mm hh:mm (o fuso depende da máquina).
        let t = event_time(1_760_000_000_000);
        assert_eq!((t.len(), t.as_bytes()[2], t.as_bytes()[8]), (11, b'/', b':'), "{t}");
    }

    #[test]
    fn history_lists_the_newest_first() {
        let t = Task {
            events: vec![
                TaskEvent { at_ms: 1, text: "a".into() },
                TaskEvent { at_ms: 3, text: "c".into() },
                TaskEvent { at_ms: 2, text: "b".into() },
            ],
            ..Task::default()
        };
        assert_eq!(events_newest_first(&t).iter().map(|e| e.text.as_str()).collect::<Vec<_>>(), ["c", "b", "a"]);
    }

    #[test]
    fn the_demo_has_one_task_per_status() {
        let tasks = demo_tasks();
        for status in [
            TaskStatus::Draft,
            TaskStatus::Running,
            TaskStatus::Review,
            TaskStatus::Validating,
            TaskStatus::Merged,
            TaskStatus::Discarded,
            TaskStatus::Failed,
        ] {
            assert!(tasks.iter().any(|t| t.status == status), "{status:?}");
        }
    }
}

//! Estado e ações da aba "Tarefas" da barra direita (a pintura fica em `ui/tasks.rs`).
//!
//! Toda chamada de tarefa do core roda em git (criar, iniciar, mesclar, descartar, diff), então vai
//! para uma thread (`Deck::spawn_bg`) e a resposta volta como `UiMsg::Tasks`. A lista é relida quando
//! a aba abre, quando o core avisa (`CoreEvent::TasksChanged`) e depois de cada ação. Aqui também
//! ficam as funções puras que decidem o que a tela mostra (grupos, textos, ações por estado), com testes.

use std::collections::HashSet;
use std::path::PathBuf;

use agentdeck_core::models::{self, Effort};
use agentdeck_core::{MergeBlock, Task, TaskError, TaskEvent, TaskSpec, TaskStatus};

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

    #[cfg(test)]
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

/// Nome curto do estado para o cartão e o histórico (`em revisão`, `aguarda validação`, `concluída`…).
pub fn card_state_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Draft => "rascunho",
        TaskStatus::Running => "em andamento",
        TaskStatus::Review => "em revisão",
        TaskStatus::Validating => "aguarda validação",
        TaskStatus::Merged => "concluída",
        TaskStatus::Discarded => "descartada",
        TaskStatus::Failed => "falhou",
    }
}

/// O cartão pede ação imediata do usuário (`Review` ou `Validating`) e recebe destaque visual.
pub fn is_focused_card(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Review | TaskStatus::Validating)
}

/// Etapa atual da barra de progresso de 5 segmentos (`1..=5`; `0` para descartada).
pub fn task_step(status: TaskStatus) -> usize {
    match status {
        TaskStatus::Draft => 1,
        TaskStatus::Running | TaskStatus::Failed => 2,
        TaskStatus::Review => 3,
        TaskStatus::Validating => 4,
        TaskStatus::Merged => 5,
        TaskStatus::Discarded => 0,
    }
}

/// Filtro da seção "HISTÓRICO" (tarefas encerradas).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum HistoryFilter {
    #[default]
    All,
    Done,
    Discarded,
}

impl HistoryFilter {
    pub const ALL: [HistoryFilter; 3] = [HistoryFilter::All, HistoryFilter::Done, HistoryFilter::Discarded];

    pub fn label(self) -> &'static str {
        match self {
            HistoryFilter::All => "Concluídas e descartadas",
            HistoryFilter::Done => "Só concluídas",
            HistoryFilter::Discarded => "Só descartadas",
        }
    }

    pub fn matches(self, status: TaskStatus) -> bool {
        match self {
            HistoryFilter::All => matches!(status, TaskStatus::Merged | TaskStatus::Discarded),
            HistoryFilter::Done => status == TaskStatus::Merged,
            HistoryFilter::Discarded => status == TaskStatus::Discarded,
        }
    }
}

/// Tarefas em andamento (não encerradas: `Draft`, `Running`, `Review`, `Validating`, `Failed`),
/// ordenadas por prioridade do grupo (quem pede ação do humano vem primeiro) e atividade recente.
pub fn active_tasks(tasks: &[Task]) -> Vec<&Task> {
    group_tasks(tasks).into_iter().filter(|(g, _)| !matches!(g, Group::Done | Group::Cancelled)).flat_map(|(_, list)| list).collect()
}

/// Tarefas encerradas (`Merged`, `Discarded`) filtradas por `filter`, da mais recente para a mais antiga.
pub fn history_tasks(tasks: &[Task], filter: HistoryFilter) -> Vec<&Task> {
    let mut list: Vec<&Task> = tasks.iter().filter(|t| filter.matches(t.status)).collect();
    list.sort_by(|a, b| last_activity(b).cmp(&last_activity(a)).then_with(|| task_number(&b.id).cmp(&task_number(&a.id))));
    list
}

/// Tempo relativo da última atividade da tarefa (`agora`, `há 6 min`, `há 2 h`).
pub fn task_age(task: &Task, now_ms: u64) -> String {
    age_text(now_ms, last_activity(task))
}

/// Data curta `dd/mm` do encerramento (ou última atividade) para a linha do histórico.
pub fn task_date_ddmm(task: &Task) -> String {
    use chrono::TimeZone;
    let at_ms = task.finished_ms.unwrap_or_else(|| last_activity(task));
    match chrono::Local.timestamp_millis_opt(at_ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format("%d/%m").to_string(),
        chrono::LocalResult::None => String::new(),
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

/// Esforço para mostrar ao lado do modelo ("alto"); vazio no padrão. No Antigravity o esforço já aparece no
/// nome do modelo (`…-high`), então não se repete.
pub fn effort_text(task: &Task) -> Option<&'static str> {
    let effort = task.effort?;
    let in_name = task.model.as_deref().is_some_and(|m| m.ends_with(&format!("-{}", effort.as_str())));
    (!in_name).then(|| effort.label())
}

/// "sonnet · alto", "alto", "sonnet" ou nada: o que acompanha o agente no cartão e no detalhe da tarefa.
pub fn model_effort_text(task: &Task) -> Option<String> {
    let parts: Vec<&str> = [model_text(task), effort_text(task)].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
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
        // Com relatório, a mesclagem falhou (de uma versão anterior): dá para devolver ao agente como em revisão.
        TaskStatus::Failed if task.report.is_some() => vec![TaskAct::Retry, TaskAct::Return, tab, TaskAct::Discard],
        TaskStatus::Failed => vec![TaskAct::Retry, TaskAct::Discard],
        TaskStatus::Merged | TaskStatus::Discarded => Vec::new(),
    }
}

/// Aguardando validação ou concluída, com o commit da mesclagem guardado: o diff é o que a mesclagem trouxe
/// (a branch já é a própria base, e `base...branch` ficaria vazio).
pub fn shows_merged_diff(task: &Task) -> bool {
    matches!(task.status, TaskStatus::Validating | TaskStatus::Merged) && task.merge_commit.as_deref().is_some_and(|h| !h.is_empty())
}

/// Dá para ver o diff: a branch existe (a tarefa foi iniciada e não terminou) ou, depois da mesclagem, o
/// commit dela está guardado.
pub fn can_show_diff(task: &Task) -> bool {
    shows_merged_diff(task)
        || (task.started_ms.is_some() && matches!(task.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Failed))
}

/// O rótulo do botão que abre o diff.
pub fn diff_button_label(task: &Task) -> &'static str {
    if shows_merged_diff(task) {
        "Ver o que foi mesclado"
    } else {
        "Ver diff completo"
    }
}

/// A dica do botão que abre o diff.
pub fn diff_button_tip(task: &Task) -> &'static str {
    if shows_merged_diff(task) {
        "Abre o diff do commit de mesclagem: o que a tarefa trouxe para a base"
    } else {
        "Abre o git diff da branch da tarefa em relação à base"
    }
}

/// O título da tela do diff.
pub fn diff_title(task: &Task) -> String {
    if shows_merged_diff(task) {
        format!("{} · o que a mesclagem trouxe para {}", task.id, task.base)
    } else {
        format!("{} · alterações em relação a {}", task.id, task.base)
    }
}

/// O hash curto (7 caracteres) do commit da última mesclagem, se a tarefa já foi mesclada.
pub fn merge_short(task: &Task) -> Option<&str> {
    task.merge_commit.as_deref().filter(|h| !h.is_empty()).map(|h| h.get(..7).unwrap_or(h))
}

/// `branch → base`, com o hash curto da mesclagem ao lado quando há.
pub fn branch_line(task: &Task) -> String {
    match merge_short(task) {
        Some(h) => format!("{} → {} · {h}", task.branch, task.base),
        None => format!("{} → {}", task.branch, task.base),
    }
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
    /// `None` = "Padrão" (nada vai ao CLI). Só vale onde `models::effort_unavailable` não objeta.
    pub effort: Option<Effort>,
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
            effort: None,
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
    pub history_filter: HistoryFilter,
    pub history_menu: bool,
    pub closed_sections: HashSet<Section>,
    pub confirm: Option<Confirm>,
    /// O campo de feedback do "Devolver" (em revisão) ou do "Reprovar" (aguardando validação) está aberto.
    pub return_open: bool,
    pub feedback: String,
    pub busy: Option<Busy>,
    /// Erro da última ação (faixa vermelha no detalhe) ou, com `notice_info`, o aviso de uma ação que deu certo.
    pub notice: Option<String>,
    /// O aviso é só informação (a mesclagem passou, mas há o que o usuário precisa saber): faixa neutra.
    pub notice_info: bool,
    /// O motivo estruturado da recusa da mesclagem, quando há saída para oferecer num botão.
    pub notice_block: Option<MergeBlock>,
    /// O aviso do stash que veio com a recusa (`MergeRefusal::stash_note`), mostrado depois da explicação.
    pub notice_stash: Option<String>,
    /// A lista completa de arquivos da recusa está aberta ("Ver o que está sem commit").
    pub notice_files: bool,
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
            history_filter: HistoryFilter::All,
            history_menu: false,
            closed_sections: HashSet::new(),
            confirm: None,
            return_open: false,
            feedback: String::new(),
            busy: None,
            notice: None,
            notice_info: false,
            notice_block: None,
            notice_stash: None,
            notice_files: false,
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
    /// "Guardar minhas alterações, mesclar e devolvê-las" (`Core::task_merge_assisted` com o stash ligado).
    MergeStash,
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
            TaskOp::MergeStash => "Guardando e mesclando…",
            TaskOp::Validate => "Validando…",
            TaskOp::Discard => "Descartando…",
            TaskOp::Return(_) => "Devolvendo…",
            TaskOp::Reject(_) => "Reprovando…",
        }
    }
}

/// O que a mesclagem devolve além da tarefa.
#[derive(Debug, Default)]
pub struct MergeExtra {
    pub block: Option<MergeBlock>,
    pub note: Option<String>,
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
        /// Só na mesclagem: o motivo da recusa que tem saída e o aviso sobre as alterações guardadas.
        merge: Box<MergeExtra>,
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
        self.tasks_clear_notice();
    }

    /// Tira o aviso da tela, com o motivo da recusa e a lista de arquivos que iam com ele.
    pub fn tasks_clear_notice(&mut self) {
        self.tasks.notice = None;
        self.tasks.notice_info = false;
        self.tasks.notice_block = None;
        self.tasks.notice_stash = None;
        self.tasks.notice_files = false;
    }

    /// Mostra um erro simples (faixa vermelha, sem botão de saída).
    fn tasks_set_error(&mut self, msg: String) {
        self.tasks_clear_notice();
        self.tasks.notice = Some(msg);
    }

    fn tasks_clear_detail_state(&mut self) {
        self.tasks.confirm = None;
        self.tasks.return_open = false;
        self.tasks.feedback.clear();
    }

    /// "←": do detalhe e do formulário à lista.
    pub fn tasks_back(&mut self) {
        self.tasks.view = TasksView::List;
        self.tasks_clear_detail_state();
        self.tasks_clear_notice();
        self.tasks.history_menu = false;
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
            form.effort = None;
        }
        if form.base.trim().is_empty() {
            form.base = self.git.status().and_then(|st| st.branch.clone()).unwrap_or_default();
        }
        let kind = self.tasks.form.kind.clone();
        self.request_models(&kind);
        self.tasks.form.focus_title = true;
        self.tasks.view = TasksView::New;
        self.tasks_clear_notice();
    }

    /// Escolheu outro agente no formulário: o modelo e o esforço voltam ao padrão.
    pub fn tasks_form_set_kind(&mut self, kind: String) {
        if self.tasks.form.kind != kind {
            self.tasks.form.model = None;
            self.tasks.form.effort = None;
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
            // O esforço escolhido antes de trocar de modelo pode já não valer (Antigravity sem versões por esforço).
            effort: f.effort.filter(|_| models::effort_unavailable(&f.kind, f.model.as_deref()).is_none()),
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
        self.tasks_clear_notice();
        self.tasks.confirm = None;
        let (generation, project) = (self.tasks.generation, self.active_project.clone());
        self.spawn_bg("task-op", move |core| {
            let mut merge = Box::new(MergeExtra::default());
            let result = Box::new(match &op {
                TaskOp::Start => core.task_start(&project, &id),
                TaskOp::Merge | TaskOp::MergeStash => match core.task_merge_assisted(&project, &id, matches!(op, TaskOp::MergeStash)) {
                    Ok(done) => {
                        merge.note = done.note;
                        Ok(done.task)
                    }
                    Err(refusal) => {
                        merge.block = refusal.block;
                        merge.note = refusal.stash_note;
                        Err(refusal.error)
                    }
                },
                TaskOp::Validate => core.task_validate(&project, &id),
                TaskOp::Discard => core.task_discard(&project, &id),
                TaskOp::Return(feedback) => core.task_return(&project, &id, feedback),
                TaskOp::Reject(feedback) => core.task_reject(&project, &id, feedback),
            });
            Some(UiMsg::Tasks(generation, TaskReply::Done { id, op, result, merge }))
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
                    self.tasks_set_error(e);
                }
            }
        }
    }

    /// "Guardar minhas alterações, mesclar e devolvê-las" no aviso da recusa: sem outra confirmação, o clique já é o pedido.
    pub fn tasks_merge_stash(&mut self, id: &str) {
        self.tasks_run(id.to_owned(), TaskOp::MergeStash);
    }

    /// "Pedir ao agente para fazer o commit" / "…resolver o conflito": devolve a tarefa com o pedido pronto do motivo da recusa.
    pub fn tasks_ask_agent(&mut self, id: &str) {
        let Some(task) = self.tasks.task(id) else { return };
        let Some(feedback) = self.tasks.notice_block.as_ref().and_then(|b| b.return_feedback(task)) else { return };
        self.tasks_run(id.to_owned(), TaskOp::Return(feedback));
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
            TaskReply::Done { id, op, result, merge } => {
                if self.tasks.busy.as_ref().is_some_and(|b| b.id == id) {
                    self.tasks.busy = None;
                }
                match *result {
                    Ok(task) => {
                        upsert(&mut self.tasks.tasks, task);
                        self.tasks_clear_notice();
                        // A mesclagem passou, mas há o que dizer sobre as alterações guardadas (voltaram, ou ficaram no stash).
                        if let Some(note) = merge.note {
                            if self.tasks.view != TasksView::Detail(id.clone()) {
                                self.tasks_open_detail(&id);
                            }
                            self.tasks.notice = Some(note);
                            self.tasks.notice_info = true;
                        }
                        if matches!(op, TaskOp::Return(_) | TaskOp::Reject(_)) {
                            self.tasks.return_open = false;
                            self.tasks.feedback.clear();
                        }
                    }
                    Err(e) => {
                        // O aviso só aparece no detalhe da tarefa: uma ação recusada pelo botão do cartão (na
                        // lista), ou enquanto se olhava outra tarefa ou o formulário, abre o detalhe dela, senão a
                        // recusa passa sem ninguém ver (ou aparece na tarefa errada).
                        if self.tasks.view != TasksView::Detail(id.clone()) {
                            self.tasks_open_detail(&id);
                        }
                        self.tasks.notice = Some(e.message().to_owned());
                        self.tasks.notice_block = merge.block;
                        self.tasks.notice_stash = merge.note;
                    }
                }
                self.tasks_refresh();
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
// Estados fictícios (capturas de tela, `AGENTDECK_DEBUG_UI=tasks-*`)
// ---------------------------------------------------------------------------

/// Uma tarefa de cada estado, com dados realistas.
pub fn demo_tasks() -> Vec<Task> {
    let now = agentdeck_core::util::now_ms();
    let min = 60_000u64;
    let ev = |ago_min: u64, text: &str| TaskEvent { at_ms: now - ago_min * min, text: text.to_owned() };
    let base = |id: &str, title: &str, status: TaskStatus, kind: &str, model: Option<&str>| Task {
        id: id.to_owned(),
        title: title.to_owned(),
        status,
        base: "dev".into(),
        branch: format!("tarefa/{id}-{}", agentdeck_core::tasks::slugify(title, 40)),
        worktree: format!(r"C:\Users\voce\agentdeck-dev\.agentdeck\worktrees\{id}").into(),
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
            ev(90, "iniciada: worktree C:\\Users\\voce\\agentdeck-dev\\.agentdeck\\worktrees\\t4 na branch tarefa/t4-aba-tarefas-na-barra-direita"),
            ev(90, "brief entregue ao agente"),
            ev(6, "relatório recebido; em revisão"),
        ],
        effort: Some(Effort::High),
        ..base("t4", "Aba Tarefas na barra direita", TaskStatus::Review, "claude", Some("claude-opus-5-5"))
    };
    let review2 = Task {
        created_ms: now - 50 * min,
        started_ms: Some(now - 48 * min),
        finished_ms: Some(now - 2 * 60 * min),
        report: Some("Corrigi o vazamento de handle no PTY.".into()),
        diff_stat: Some(" crates/app/src/terminal/mod.rs | 6 +++---\n 1 file changed, 3 insertions(+), 3 deletions(-)\n".into()),
        events: vec![ev(150, "relatório recebido; em revisão")],
        effort: Some(Effort::High),
        ..base("t5", "Corrigir vazamento de handle ao fechar a aba", TaskStatus::Review, "antigravity", Some("gemini-4-argon-high"))
    };
    let running = Task {
        created_ms: now - 25 * min,
        started_ms: Some(now - 24 * min),
        events: vec![ev(25, "criada"), ev(24, "iniciada: worktree … na branch tarefa/t6")],
        effort: Some(Effort::Medium),
        ..base("t6", "Documentar as rotas /tasks da ponte", TaskStatus::Running, "claude", Some("claude-sonnet-5-5"))
    };
    let draft = Task {
        brief: "Rodar o clippy com -D warnings e corrigir o que aparecer.".into(),
        created_ms: now - 3 * 60 * min,
        events: vec![ev(180, "criada")],
        ..base("t7", "Limpar os avisos do clippy no crate app", TaskStatus::Draft, "opencode", None)
    };
    let failed = Task {
        created_ms: now - 5 * 60 * min,
        error: Some("não consegui criar a worktree: a branch tarefa/t3-nova-branch já existe neste repositório".into()),
        events: vec![
            ev(300, "criada"),
            ev(299, "erro: não consegui criar a worktree: a branch tarefa/t3-nova-branch já existe neste repositório"),
        ],
        ..base("t3", "Trocar o parser de markdown do relatório", TaskStatus::Failed, "opencode", Some("opencode/deepseek-v4.1-flash-free"))
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
        merge_commit: Some("a1b2c3d4e5f60123456789abcdef0123456789ab".into()),
        events: vec![
            ev(180, "criada"),
            ev(170, "iniciada: worktree … na branch tarefa/t8"),
            ev(20, "relatório recebido; em revisão"),
            ev(8, "mesclada em dev (a1b2c3d); aguardando validação"),
        ],
        ..base("t8", "Estado da mesclagem no cabeçalho do detalhe", TaskStatus::Validating, "claude", Some("claude-sonnet-5-5"))
    };
    let merged = Task {
        created_ms: now - 26 * 60 * min,
        started_ms: Some(now - 26 * 60 * min),
        finished_ms: Some(now - 24 * 60 * min),
        events: vec![ev(24 * 60, "validada; concluída; worktree e branch removidas")],
        ..base("t2", "Rotas /tasks na ponte", TaskStatus::Merged, "claude", Some("claude-opus-5-5"))
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
    fn effort_is_shown_next_to_the_model_unless_the_model_name_has_it() {
        let mut t = Task::default();
        assert_eq!((effort_text(&t), model_effort_text(&t)), (None, None));
        t.model = Some("opencode/glm-5.3".into());
        assert_eq!(model_effort_text(&t).as_deref(), Some("glm-5.3"));
        t.model = Some("sonnet".into());
        t.effort = Some(Effort::High);
        assert_eq!(model_effort_text(&t).as_deref(), Some("sonnet · alto"));
        t.model = None;
        assert_eq!(model_effort_text(&t).as_deref(), Some("alto"));
        // Antigravity: o esforço já está no nome do modelo.
        t.model = Some("gemini-4-argon-medium".into());
        t.effort = Some(Effort::Medium);
        assert_eq!((effort_text(&t), model_effort_text(&t).as_deref()), (None, Some("gemini-4-argon-medium")));
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
        // Mesclagem que falhou antes (Failed com relatório): também se devolve ao agente.
        assert_eq!(actions_for(&t(TaskStatus::Failed, true), true), [TaskAct::Retry, TaskAct::Return, TaskAct::GoToTab, TaskAct::Discard]);
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
    fn the_diff_is_available_while_the_branch_exists_or_after_the_merge_commit_is_kept() {
        let t = |status, started: bool| Task { status, started_ms: started.then_some(1), ..Task::default() };
        let merged = |status| Task { merge_commit: Some("0123456789abcdef0123456789abcdef01234567".into()), ..t(status, true) };
        assert!(can_show_diff(&t(TaskStatus::Review, true)));
        assert!(can_show_diff(&t(TaskStatus::Running, true)));
        assert!(can_show_diff(&t(TaskStatus::Failed, true)));
        assert!(!can_show_diff(&t(TaskStatus::Failed, false)));
        assert!(!can_show_diff(&t(TaskStatus::Draft, false)));
        assert!(!can_show_diff(&t(TaskStatus::Discarded, true)));
        // Sem o commit da mesclagem (tarefa antiga), aguardando validação ou concluída não tem diff: a branch é a própria base.
        assert!(!can_show_diff(&t(TaskStatus::Validating, true)));
        assert!(!can_show_diff(&t(TaskStatus::Merged, true)));
        // Com o commit, os dois estados mostram o que foi mesclado.
        assert!(can_show_diff(&merged(TaskStatus::Validating)) && can_show_diff(&merged(TaskStatus::Merged)));
        // Descartada continua sem diff, mesmo que já tenha sido mesclada antes.
        assert!(!can_show_diff(&merged(TaskStatus::Discarded)));
    }

    #[test]
    fn the_diff_button_says_what_it_shows_in_each_state() {
        let hash = "0123456789abcdef0123456789abcdef01234567";
        let t = |status, commit: Option<&str>| Task {
            status,
            started_ms: Some(1),
            id: "t5".into(),
            base: "dev".into(),
            branch: "tarefa/t5-x".into(),
            merge_commit: commit.map(str::to_owned),
            ..Task::default()
        };
        for status in [TaskStatus::Running, TaskStatus::Review, TaskStatus::Failed] {
            assert_eq!(diff_button_label(&t(status, None)), "Ver diff completo");
            assert_eq!(diff_title(&t(status, None)), "t5 · alterações em relação a dev");
        }
        // Reprovada e de novo em revisão, o commit antigo continua guardado: o diff é o da branch.
        assert_eq!(diff_button_label(&t(TaskStatus::Review, Some(hash))), "Ver diff completo");
        for status in [TaskStatus::Validating, TaskStatus::Merged] {
            assert_eq!(diff_button_label(&t(status, Some(hash))), "Ver o que foi mesclado");
            assert_eq!(diff_title(&t(status, Some(hash))), "t5 · o que a mesclagem trouxe para dev");
        }
        // O detalhe mostra o hash curto ao lado de `branch → base`.
        assert_eq!(branch_line(&t(TaskStatus::Validating, Some(hash))), "tarefa/t5-x → dev · 0123456");
        assert_eq!(branch_line(&t(TaskStatus::Running, None)), "tarefa/t5-x → dev");
        assert_eq!(merge_short(&t(TaskStatus::Merged, Some(""))), None);
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

    #[test]
    fn active_tasks_and_history_split_and_filter_demo_tasks() {
        let tasks = demo_tasks();
        let active = active_tasks(&tasks);
        let active_ids: Vec<&str> = active.iter().map(|t| t.id.as_str()).collect();
        // Só as não encerradas (Review, Validating, Running, Draft, Failed), na ordem de prioridade e recência.
        assert_eq!(active_ids, ["t4", "t5", "t8", "t6", "t7", "t3"]);
        assert!(active.iter().all(|t| !matches!(t.status, TaskStatus::Merged | TaskStatus::Discarded)));

        // Histórico: todas (Merged + Discarded), da mais recente (t2, há 24h) para a mais antiga (t1, há 48h).
        let all_hist = history_tasks(&tasks, HistoryFilter::All);
        assert_eq!(all_hist.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);

        // Filtro só concluídas / só descartadas.
        let done_only = history_tasks(&tasks, HistoryFilter::Done);
        assert_eq!(done_only.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
        let discarded_only = history_tasks(&tasks, HistoryFilter::Discarded);
        assert_eq!(discarded_only.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1"]);

        // Etapas de 5 segmentos e destaque dos cartões que pedem ação do humano.
        assert_eq!(task_step(TaskStatus::Draft), 1);
        assert_eq!(task_step(TaskStatus::Running), 2);
        assert_eq!(task_step(TaskStatus::Failed), 2);
        assert_eq!(task_step(TaskStatus::Review), 3);
        assert_eq!(task_step(TaskStatus::Validating), 4);
        assert_eq!(task_step(TaskStatus::Merged), 5);
        assert_eq!(task_step(TaskStatus::Discarded), 0);
        assert!(is_focused_card(TaskStatus::Review) && is_focused_card(TaskStatus::Validating));
        assert!(!is_focused_card(TaskStatus::Running) && !is_focused_card(TaskStatus::Draft));
        assert_eq!(card_state_label(TaskStatus::Review), "em revisão");
        assert_eq!(card_state_label(TaskStatus::Validating), "aguarda validação");
        assert_eq!(card_state_label(TaskStatus::Merged), "concluída");
        assert_eq!(card_state_label(TaskStatus::Discarded), "descartada");

        // Data dd/mm tem 5 caracteres com '/' no meio.
        let ddmm = task_date_ddmm(&tasks[0]);
        assert_eq!((ddmm.len(), ddmm.as_bytes()[2]), (5, b'/'), "{ddmm}");
    }
}

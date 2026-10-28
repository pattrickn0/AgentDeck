//! State and actions of the right bar's "Tasks" tab (the painting lives in `ui/tasks.rs`).
//!
//! Every task call into the core runs git (create, start, merge, discard, diff), so it goes to a
//! thread (`Deck::spawn_bg`) and the answer comes back as `UiMsg::Tasks`. The list is re-read when
//! the tab opens, when the core says so (`CoreEvent::TasksChanged`) and after every action. The pure
//! functions that decide what the screen shows (groups, texts, actions per status) live here too, with tests.

use std::collections::HashSet;
use std::path::PathBuf;

use agentdeck_core::capability::{self, Level};
use agentdeck_core::models::{self, Effort};
use agentdeck_core::{t, FeedKind, MergeBlock, Task, TaskError, TaskEvent, TaskMessage, TaskSpec, TaskStatus};

use crate::deck::{AttachTarget, Deck, UiMsg};

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

/// Groups of the list, in the order they appear (what needs the human comes first).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Group {
    Review,
    /// Merged, waiting for the user to test them.
    Validating,
    Running,
    /// Started, but waiting for the scope to free up (see `agentdeck_core::scope`).
    Queued,
    Draft,
    Failed,
    /// Validated (completed).
    Done,
    /// Discarded (cancelled): kept apart from the completed ones.
    Cancelled,
}

impl Group {
    pub const ORDER: [Group; 8] =
        [Group::Review, Group::Validating, Group::Running, Group::Queued, Group::Draft, Group::Failed, Group::Done, Group::Cancelled];

    #[cfg(test)]
    pub fn title(self) -> &'static str {
        match self {
            Group::Review => t!("In review"),
            Group::Validating => t!("Awaiting validation"),
            Group::Running => t!("In progress"),
            Group::Queued => t!("Queued"),
            Group::Draft => t!("Draft"),
            Group::Failed => t!("Failed"),
            Group::Done => t!("Finished"),
            Group::Cancelled => t!("Cancelled"),
        }
    }

    pub fn of(status: TaskStatus) -> Group {
        match status {
            TaskStatus::Review => Group::Review,
            TaskStatus::Validating => Group::Validating,
            TaskStatus::Running => Group::Running,
            TaskStatus::Queued => Group::Queued,
            TaskStatus::Draft => Group::Draft,
            TaskStatus::Failed => Group::Failed,
            TaskStatus::Merged => Group::Done,
            TaskStatus::Discarded => Group::Cancelled,
        }
    }
}

/// Status name for the chip (capitalized, unlike `TaskStatus::label`).
pub fn status_name(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Draft => t!("Draft"),
        TaskStatus::Queued => t!("Queued"),
        TaskStatus::Running => t!("In progress"),
        TaskStatus::Review => t!("In review"),
        TaskStatus::Validating => t!("Awaiting validation"),
        TaskStatus::Merged => t!("Completed"),
        TaskStatus::Discarded => t!("Discarded"),
        TaskStatus::Failed => t!("Failed"),
    }
}

/// Short status name for the card and the history (`in review`, `awaiting validation`, `completed`…).
pub fn card_state_label(status: TaskStatus) -> &'static str {
    match status {
        TaskStatus::Draft => t!("draft"),
        TaskStatus::Queued => t!("queued"),
        TaskStatus::Running => t!("in progress"),
        TaskStatus::Review => t!("in review"),
        TaskStatus::Validating => t!("awaiting validation"),
        TaskStatus::Merged => t!("completed"),
        TaskStatus::Discarded => t!("discarded"),
        TaskStatus::Failed => t!("failed"),
    }
}

/// The card asks for the user's immediate action (`Review` or `Validating`) and gets a visual highlight.
pub fn is_focused_card(status: TaskStatus) -> bool {
    matches!(status, TaskStatus::Review | TaskStatus::Validating)
}

/// Current step of the 5-segment progress bar (`1..=5`; `0` for discarded).
pub fn task_step(status: TaskStatus) -> usize {
    match status {
        TaskStatus::Draft | TaskStatus::Queued => 1,
        TaskStatus::Running | TaskStatus::Failed => 2,
        TaskStatus::Review => 3,
        TaskStatus::Validating => 4,
        TaskStatus::Merged => 5,
        TaskStatus::Discarded => 0,
    }
}

/// Filter of the "HISTORY" section (finished tasks).
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
            HistoryFilter::All => t!("Completed and discarded"),
            HistoryFilter::Done => t!("Completed only"),
            HistoryFilter::Discarded => t!("Discarded only"),
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

/// Tasks in progress (not finished: `Draft`, `Running`, `Review`, `Validating`, `Failed`),
/// sorted by group priority (what asks for the human's action comes first) and recent activity.
pub fn active_tasks(tasks: &[Task]) -> Vec<&Task> {
    group_tasks(tasks).into_iter().filter(|(g, _)| !matches!(g, Group::Done | Group::Cancelled)).flat_map(|(_, list)| list).collect()
}

/// Finished tasks (`Merged`, `Discarded`) filtered by `filter`, from the newest to the oldest.
pub fn history_tasks(tasks: &[Task], filter: HistoryFilter) -> Vec<&Task> {
    let mut list: Vec<&Task> = tasks.iter().filter(|t| filter.matches(t.status)).collect();
    list.sort_by(|a, b| last_activity(b).cmp(&last_activity(a)).then_with(|| task_number(&b.id).cmp(&task_number(&a.id))));
    list
}

/// Relative time of the task's last activity (`now`, `6 min ago`, `2 h ago`).
pub fn task_age(task: &Task, now_ms: u64) -> String {
    age_text(now_ms, last_activity(task))
}

/// Short `dd/mm` date of the end (or last activity) for the history row.
pub fn task_date_ddmm(task: &Task) -> String {
    use chrono::TimeZone;
    let at_ms = task.finished_ms.unwrap_or_else(|| last_activity(task));
    match chrono::Local.timestamp_millis_opt(at_ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format("%d/%m").to_string(),
        chrono::LocalResult::None => String::new(),
    }
}

/// The last thing that happened to the task (ms): the last history event, or the creation.
pub fn last_activity(task: &Task) -> u64 {
    task.events.iter().map(|e| e.at_ms).max().unwrap_or(0).max(task.created_ms)
}

/// Tasks per group (only the groups that have any), each from the most recent activity to the oldest.
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

/// `t12` → 12 (0 if it is not a task id).
fn task_number(id: &str) -> u64 {
    id.trim_start_matches('t').parse().unwrap_or(0)
}

/// How many tasks wait for the human's review.
pub fn review_count(tasks: &[Task]) -> usize {
    tasks.iter().filter(|t| t.status == TaskStatus::Review).count()
}

/// How many tasks, already merged, wait for the human's test.
pub fn validating_count(tasks: &[Task]) -> usize {
    tasks.iter().filter(|t| t.status == TaskStatus::Validating).count()
}

/// The `git diff --stat` summary: files, added lines and removed lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiffSummary {
    pub files: u32,
    pub insertions: u32,
    pub deletions: u32,
}

impl DiffSummary {
    /// `3 files` (singular with 1; "no changes" with 0).
    pub fn files_text(&self) -> String {
        match self.files {
            0 => t!("no changes").to_owned(),
            1 => t!("1 file").to_owned(),
            n => t!("{n} files", n),
        }
    }

    /// `3 files, +120 −8` (without the side that is zero).
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

/// Reads the last line of `git diff --stat` ("3 files changed, 120 insertions(+), 8 deletions(-)").
/// "(no changes against the base)" (what the core stores when there are no differences; tasks saved by an
/// older version have the Portuguese "(nenhuma alteração em relação à base)") becomes zero files.
pub fn parse_diff_stat(stat: &str) -> Option<DiffSummary> {
    if stat.contains("nenhuma alteração") || stat.trim_start().starts_with("(no changes") {
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

/// One line per file of `git diff --stat`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatLine {
    pub path: String,
    /// Changed lines (`412`), or `Bin 0 -> 1234 bytes` for a binary file.
    pub changes: String,
}

/// The files of `git diff --stat` (` path | 12 ++++--`); the total line is not included.
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

/// `now`, `5 min ago`, `3 h ago`, `2 d ago` (a `then` in the future counts as now).
pub fn age_text(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => t!("now").to_owned(),
        60..=3599 => t!("{n} min ago", n = secs / 60),
        3600..=86_399 => t!("{n} h ago", n = secs / 3600),
        _ => t!("{n} d ago", n = secs / 86_400),
    }
}

/// What happened and when, for the card's second line (`ready 3 min ago`).
pub fn status_age(task: &Task, now_ms: u64) -> String {
    let finished = || task.finished_ms.unwrap_or_else(|| last_activity(task));
    let at = match task.status {
        TaskStatus::Draft => task.created_ms,
        TaskStatus::Queued => task.queued_ms.unwrap_or(task.created_ms),
        TaskStatus::Running => task.started_ms.unwrap_or(task.created_ms),
        TaskStatus::Failed => last_activity(task),
        TaskStatus::Review | TaskStatus::Validating | TaskStatus::Merged | TaskStatus::Discarded => finished(),
    };
    let age = age_text(now_ms, at);
    match task.status {
        TaskStatus::Draft => t!("created {age}", age),
        TaskStatus::Queued => t!("queued {age}", age),
        TaskStatus::Running => t!("started {age}", age),
        TaskStatus::Review => t!("ready {age}", age),
        TaskStatus::Failed => t!("failed {age}", age),
        TaskStatus::Validating => t!("merged {age}", age),
        TaskStatus::Merged => t!("completed {age}", age),
        TaskStatus::Discarded => t!("discarded {age}", age),
    }
}

/// LLM model to show (`opencode/glm-5.3` → `glm-5.3`); empty when it is the CLI's default.
pub fn model_text(task: &Task) -> Option<&str> {
    task.model.as_deref().map(models::short_model).filter(|m| !m.is_empty())
}

/// Effort to show next to the model ("high"); empty for the default. On Antigravity the effort already shows in the
/// model name (`…-high`), so it is not repeated.
pub fn effort_text(task: &Task) -> Option<&'static str> {
    let effort = task.effort?;
    let in_name = task.model.as_deref().is_some_and(|m| m.ends_with(&format!("-{}", effort.as_str())));
    (!in_name).then(|| effort.label())
}

/// "sonnet · high", "high", "sonnet" or nothing: what goes with the agent on the card and in the task detail.
pub fn model_effort_text(task: &Task) -> Option<String> {
    let parts: Vec<&str> = [model_text(task), effort_text(task)].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

/// The scope typed in the form: one path per line or separated by commas; empty lines disappear. The
/// normalization and the refusal of what is not a path (wildcards, absolute paths, `..`) belong to the core, when the task is created.
pub fn parse_scope(text: &str) -> Vec<String> {
    text.split(['\n', ',']).map(str::trim).filter(|p| !p.is_empty()).map(str::to_owned).collect()
}

/// Buttons of a task's detail. The order is the one on screen (the first button is the main action).
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
            TaskAct::Start => t!("Start").to_owned(),
            TaskAct::Retry => t!("Try again").to_owned(),
            TaskAct::Merge => t!("Merge into {base}", base = task.base),
            TaskAct::Return => t!("Return").to_owned(),
            TaskAct::Validate => t!("Validate").to_owned(),
            TaskAct::Reject => t!("Reject").to_owned(),
            TaskAct::GoToTab => t!("Go to tab").to_owned(),
            TaskAct::ReopenTab => t!("Reopen tab").to_owned(),
            TaskAct::Discard => t!("Discard").to_owned(),
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            TaskAct::Start => t!("Creates the worktree and the branch, opens the agent and hands over the request"),
            TaskAct::Retry => t!("Tries again what failed (start, or merge if the agent already delivered the report)"),
            TaskAct::Merge => {
                t!("Merges the task branch into the base (git merge --no-ff); the agent and the worktree stay until you validate")
            }
            TaskAct::Return => t!("Sends the task back to the agent with a request for changes"),
            TaskAct::Validate => t!("You tested it and it is right: completes the task and deletes the worktree and the branch"),
            TaskAct::Reject => t!("You tested it and it failed: sends the task back to the same agent, with the reason"),
            TaskAct::GoToTab => t!("Shows the terminal of this task's agent"),
            TaskAct::ReopenTab => t!("Opens the agent's tab again, in the terminal of the task's worktree"),
            TaskAct::Discard => t!("Discards the task: deletes the worktree and the branch (without merging)"),
        }
    }

    /// Main action (filled button).
    pub fn is_primary(self) -> bool {
        matches!(self, TaskAct::Start | TaskAct::Retry | TaskAct::Merge | TaskAct::Validate)
    }

    /// Asks for confirmation inside the panel before running.
    pub fn needs_confirm(self, task: &Task) -> bool {
        match self {
            TaskAct::Discard | TaskAct::Merge => true,
            // Trying again with a report is a merge.
            TaskAct::Retry => task.report.is_some(),
            _ => false,
        }
    }
}

/// The buttons of each status. `tab_open`: the agent's tab is open (otherwise, "Reopen tab").
pub fn actions_for(task: &Task, tab_open: bool) -> Vec<TaskAct> {
    let tab = if tab_open { TaskAct::GoToTab } else { TaskAct::ReopenTab };
    match task.status {
        TaskStatus::Draft => vec![TaskAct::Start, TaskAct::Discard],
        // Starts by itself when the scope frees up; no tab or worktree yet.
        TaskStatus::Queued => vec![TaskAct::Discard],
        TaskStatus::Running => vec![tab, TaskAct::Discard],
        TaskStatus::Review => vec![TaskAct::Merge, TaskAct::Return, tab, TaskAct::Discard],
        TaskStatus::Validating => vec![TaskAct::Validate, TaskAct::Reject, tab, TaskAct::Discard],
        // With a report, the merge failed (in an earlier version): it can be sent back to the agent as in review.
        TaskStatus::Failed if task.report.is_some() => vec![TaskAct::Retry, TaskAct::Return, tab, TaskAct::Discard],
        TaskStatus::Failed => vec![TaskAct::Retry, TaskAct::Discard],
        TaskStatus::Merged | TaskStatus::Discarded => Vec::new(),
    }
}

/// Awaiting validation or completed, with the merge commit stored: the diff is what the merge brought
/// (the branch is already the base itself, and `base...branch` would be empty).
pub fn shows_merged_diff(task: &Task) -> bool {
    matches!(task.status, TaskStatus::Validating | TaskStatus::Merged) && task.merge_commit.as_deref().is_some_and(|h| !h.is_empty())
}

/// The diff can be viewed: the branch exists (the task was started and has not finished) or, after the merge, its
/// commit is stored.
pub fn can_show_diff(task: &Task) -> bool {
    shows_merged_diff(task)
        || (task.started_ms.is_some() && matches!(task.status, TaskStatus::Running | TaskStatus::Review | TaskStatus::Failed))
}

/// The label of the button that opens the diff.
pub fn diff_button_label(task: &Task) -> &'static str {
    if shows_merged_diff(task) {
        t!("View what was merged")
    } else {
        t!("View full diff")
    }
}

/// The tooltip of the button that opens the diff.
pub fn diff_button_tip(task: &Task) -> &'static str {
    if shows_merged_diff(task) {
        t!("Opens the diff of the merge commit: what the task brought into the base")
    } else {
        t!("Opens the git diff of the task branch against the base")
    }
}

/// The title of the diff screen.
pub fn diff_title(task: &Task) -> String {
    if shows_merged_diff(task) {
        t!("{id} · what the merge brought into {base}", id = task.id, base = task.base)
    } else {
        t!("{id} · changes against {base}", id = task.id, base = task.base)
    }
}

/// The short hash (7 characters) of the last merge commit, if the task has already been merged.
pub fn merge_short(task: &Task) -> Option<&str> {
    task.merge_commit.as_deref().filter(|h| !h.is_empty()).map(|h| h.get(..7).unwrap_or(h))
}

/// `branch → base`, with the short merge hash next to it when there is one.
pub fn branch_line(task: &Task) -> String {
    match merge_short(task) {
        Some(h) => format!("{} → {} · {h}", task.branch, task.base),
        None => format!("{} → {}", task.branch, task.base),
    }
}

/// Replaces the task with the same id in the list (or appends it).
pub fn upsert(tasks: &mut Vec<Task>, task: Task) {
    match tasks.iter_mut().find(|t| t.id == task.id) {
        Some(slot) => *slot = task,
        None => tasks.push(task),
    }
}

/// Local date and time of a history event (`17/10 14:32`).
pub fn event_time(at_ms: u64) -> String {
    use chrono::TimeZone;
    match chrono::Local.timestamp_millis_opt(at_ms as i64) {
        chrono::LocalResult::Single(t) | chrono::LocalResult::Ambiguous(t, _) => t.format("%d/%m %H:%M").to_string(),
        chrono::LocalResult::None => String::new(),
    }
}

/// Text cut at `max` characters, with an ellipsis and the notice (reports and requests can have 200 thousand).
pub fn clip_text(text: &str, max: usize) -> std::borrow::Cow<'_, str> {
    match text.char_indices().nth(max) {
        Some((i, _)) => {
            let notice = t!("[… text cut on screen: {shown} of {total} characters]", shown = max, total = text.chars().count());
            std::borrow::Cow::Owned(format!("{}\n\n{notice}", &text[..i]))
        }
        None => std::borrow::Cow::Borrowed(text),
    }
}

/// The task's history, from the newest to the oldest.
pub fn events_newest_first(task: &Task) -> Vec<&TaskEvent> {
    let mut list: Vec<&TaskEvent> = task.events.iter().collect();
    list.sort_by_key(|e| std::cmp::Reverse(e.at_ms));
    list
}

/// Timeline messages shown at once (the newest ones); the earlier ones become a notice line.
pub const MESSAGES_SHOWN: usize = 40;
/// A collapsed message shows at most this much (in characters); a click opens it whole.
pub const MESSAGE_PREVIEW_CHARS: usize = 280;

/// The task's timeline, from the oldest to the newest (the order of arrival is already that; the stable `sort` only
/// protects against a file edited by hand).
pub fn messages_in_order(task: &Task) -> Vec<&TaskMessage> {
    let mut list: Vec<&TaskMessage> = task.messages.iter().collect();
    list.sort_by_key(|m| m.at_ms);
    list
}

/// Label of the message kind in the timeline.
pub fn message_kind_label(kind: FeedKind) -> &'static str {
    match kind {
        FeedKind::Ask => t!("question"),
        FeedKind::Answer => t!("answer"),
        FeedKind::Error => t!("error"),
        FeedKind::User => t!("you"),
        FeedKind::System => t!("notice"),
        FeedKind::Thinking => t!("progress"),
    }
}

/// `from → to` of a message (a missing side becomes `…`).
pub fn message_route(m: &TaskMessage) -> String {
    format!("{} → {}", m.from.as_deref().unwrap_or("…"), m.to.as_deref().unwrap_or("…"))
}

/// The collapsed text of the message: the first lines, up to [`MESSAGE_PREVIEW_CHARS`], with an ellipsis. The second value
/// says whether something was left out (that is, whether there is something to open).
pub fn message_preview(text: &str) -> (String, bool) {
    let text = text.trim();
    let lines: Vec<&str> = text.lines().collect();
    let by_lines = lines.len() > 4;
    let head = if by_lines { lines[..4].join("\n") } else { text.to_owned() };
    match head.char_indices().nth(MESSAGE_PREVIEW_CHARS) {
        Some((i, _)) => (format!("{}…", head[..i].trim_end()), true),
        None if by_lines => (format!("{}…", head.trim_end()), true),
        None => (head, false),
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// What the tab shows.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum TasksView {
    #[default]
    List,
    /// The task's detail (id).
    Detail(String),
    /// New task form.
    New,
}

/// Collapsible sections of the detail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Section {
    Brief,
    Report,
    Changes,
    Messages,
    History,
}

/// Dangerous action waiting for the "yes" in the panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Confirm {
    Merge(String),
    Discard(String),
}

/// A lifecycle operation in progress (buttons off, spinner turning).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Busy {
    pub id: String,
    pub label: &'static str,
}

/// Open menu of the form.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormMenu {
    Agent,
    Model,
}

/// "New task" form.
#[derive(Debug, Clone)]
pub struct NewForm {
    pub title: String,
    pub brief: String,
    /// Files attached to the request (absolute paths in `dados/anexos`).
    pub attachments: Vec<PathBuf>,
    /// Agent (`claude`); empty until the form opens.
    pub kind: String,
    /// The current agent came from the automatic recommendation, not from a choice by the user.
    pub kind_is_auto: bool,
    /// `None` = "Default".
    pub model: Option<String>,
    /// `None` = "Default" (nothing goes to the CLI). Only applies where `models::effort_unavailable` does not object.
    pub effort: Option<Effort>,
    /// The size of the work; decides which models can handle it and, with the automatic agent, who is recommended.
    pub level: Level,
    /// The capacity notice for the chosen agent and model (the task is above what they reach); recalculated
    /// by `tasks_form_refresh`, not on every frame (it reads the tasks and the capacity table).
    pub capacity_note: Option<String>,
    /// Provider of the model menu step (`opencode`): `None` = the list of providers shows
    /// first. Only applies while the menu is open (OpenCode mixes several providers).
    pub provider: Option<String>,
    pub base: String,
    /// Write scope: one path per line (or separated by commas). Empty = no scope. See [`parse_scope`].
    pub scope: String,
    pub start_now: bool,
    pub error: Option<String>,
    /// Creating (the button is off).
    pub busy: bool,
    pub menu: Option<FormMenu>,
    /// The title asks for keyboard focus on the next frame (the form just opened).
    pub focus_title: bool,
}

impl Default for NewForm {
    fn default() -> Self {
        Self {
            title: String::new(),
            brief: String::new(),
            attachments: Vec::new(),
            kind: String::new(),
            kind_is_auto: true,
            model: None,
            effort: None,
            level: Level::default(),
            capacity_note: None,
            provider: None,
            base: String::new(),
            scope: String::new(),
            start_now: true,
            error: None,
            busy: false,
            menu: None,
            focus_title: false,
        }
    }
}

/// State of the Tasks tab in the `Deck`.
pub struct TasksUi {
    pub view: TasksView,
    pub tasks: Vec<Task>,
    /// The first read has already arrived.
    pub loaded: bool,
    pub refreshing: bool,
    /// Another read was requested while one was in progress.
    dirty: bool,
    pub collapsed: HashSet<Group>,
    pub history_filter: HistoryFilter,
    pub history_menu: bool,
    pub closed_sections: HashSet<Section>,
    /// Timeline messages opened whole (`task id`, message time).
    pub open_messages: HashSet<(String, u64)>,
    pub confirm: Option<Confirm>,
    /// The feedback field of "Return" (in review) or "Reject" (awaiting validation) is open.
    pub return_open: bool,
    pub feedback: String,
    pub busy: Option<Busy>,
    /// Error of the last action (red strip in the detail) or, with `notice_info`, the notice of an action that worked.
    pub notice: Option<String>,
    /// The notice is only information (the merge went through, but there is something the user needs to know): neutral strip.
    pub notice_info: bool,
    /// The structured reason for the merge refusal, when there is a way out to offer on a button.
    pub notice_block: Option<MergeBlock>,
    /// The stash notice that came with the refusal (`MergeRefusal::stash_note`), shown after the explanation.
    pub notice_stash: Option<String>,
    /// The full list of files of the refusal is open ("See what is uncommitted").
    pub notice_files: bool,
    pub form: NewForm,
    /// Changes on every project switch: answers to requests from the previous project are discarded.
    pub generation: u64,
    /// DEBUGGING ONLY: a fake state that re-reads do not replace.
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
            // The completed and cancelled ones are already resolved: they start collapsed.
            collapsed: HashSet::from([Group::Done, Group::Cancelled]),
            history_filter: HistoryFilter::All,
            history_menu: false,
            closed_sections: HashSet::new(),
            open_messages: HashSet::new(),
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

    /// The number on the tab icon's badge: what waits for a human decision (review or validate).
    pub fn attention_count(&self) -> usize {
        self.review_count() + self.validating_count()
    }
}

/// What a lifecycle operation does.
#[derive(Debug, Clone)]
pub enum TaskOp {
    Start,
    Merge,
    /// "Stash my changes, merge and give them back" (`Core::task_merge_assisted` with the stash on).
    MergeStash,
    Validate,
    Discard,
    Return(String),
    Reject(String),
}

impl TaskOp {
    fn busy_label(&self) -> &'static str {
        match self {
            TaskOp::Start => t!("Starting…"),
            TaskOp::Merge => t!("Merging…"),
            TaskOp::MergeStash => t!("Stashing and merging…"),
            TaskOp::Validate => t!("Validating…"),
            TaskOp::Discard => t!("Discarding…"),
            TaskOp::Return(_) => t!("Returning…"),
            TaskOp::Reject(_) => t!("Rejecting…"),
        }
    }
}

/// What the merge returns besides the task.
#[derive(Debug, Default)]
pub struct MergeExtra {
    pub block: Option<MergeBlock>,
    pub note: Option<String>,
}

/// Answers of the background calls.
pub enum TaskReply {
    List(Vec<Task>),
    Created {
        result: Box<Result<Task, TaskError>>,
        /// The result of starting right after (only with "Start now").
        started: Option<Box<Result<Task, TaskError>>>,
    },
    Done {
        id: String,
        op: TaskOp,
        result: Box<Result<Task, TaskError>>,
        /// Only on the merge: the reason for a refusal that has a way out and the notice about the stashed changes.
        merge: Box<MergeExtra>,
    },
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

impl Deck {
    /// The project on screen changed: forgets the previous one's list, detail and form and ignores the
    /// answers that still come from it. The collapsed groups stay.
    pub fn tasks_reset_for_project(&mut self) {
        let generation = self.tasks.generation + 1;
        let collapsed = std::mem::take(&mut self.tasks.collapsed);
        self.tasks = TasksUi { generation, collapsed, ..TasksUi::default() };
    }

    /// Re-reads the list (one at a time; a request in the middle repeats the read when it finishes).
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

    /// The core saved the tasks of `project`.
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

    /// Takes the notice off the screen, with the refusal reason and the list of files that went with it.
    pub fn tasks_clear_notice(&mut self) {
        self.tasks.notice = None;
        self.tasks.notice_info = false;
        self.tasks.notice_block = None;
        self.tasks.notice_stash = None;
        self.tasks.notice_files = false;
    }

    /// Shows a plain error (red strip, no way-out button).
    fn tasks_set_error(&mut self, msg: String) {
        self.tasks_clear_notice();
        self.tasks.notice = Some(msg);
    }

    fn tasks_clear_detail_state(&mut self) {
        self.tasks.confirm = None;
        self.tasks.return_open = false;
        self.tasks.feedback.clear();
    }

    /// "←": from the detail and from the form to the list.
    pub fn tasks_back(&mut self) {
        self.tasks.view = TasksView::List;
        self.tasks_clear_detail_state();
        self.tasks_clear_notice();
        self.tasks.history_menu = false;
        self.tasks.form.menu = None;
        self.tasks.form.provider = None;
    }

    /// Agents a task can use: installed, with a tab (no plain terminal).
    pub fn tasks_agent_choices(&self) -> Vec<String> {
        self.agents.available.iter().filter(|a| a.installed && a.peer && a.id != "shell").map(|a| a.id.clone()).collect()
    }

    /// Opens the form: default agent `claude` (or the first available) and the current branch as the base.
    pub fn tasks_open_new(&mut self) {
        let choices = self.tasks_agent_choices();
        let form = &mut self.tasks.form;
        form.menu = None;
        form.provider = None;
        form.error = None;
        if form.kind_is_auto || form.kind.is_empty() || (!choices.is_empty() && !choices.contains(&form.kind)) {
            form.kind = choices.first().cloned().unwrap_or_else(|| "claude".into());
            form.kind_is_auto = true;
            form.model = None;
            form.effort = None;
            form.level = Level::default();
        }
        if form.base.trim().is_empty() {
            form.base = self.git.status().and_then(|st| st.branch.clone()).unwrap_or_default();
        }
        self.tasks_form_refresh();
        let kind = self.tasks.form.kind.clone();
        self.request_models(&kind);
        self.tasks.form.focus_title = true;
        self.tasks.view = TasksView::New;
        self.tasks_clear_notice();
    }

    /// Recalculates what depends on the form's level, agent, model and effort: the recommended agent
    /// (while the agent is the automatic one: the one capable of the level with the most quota to spare) and the capacity notice.
    pub fn tasks_form_refresh(&mut self) {
        let choices = self.tasks_agent_choices();
        let (level, effort) = (self.tasks.form.level, self.tasks.form.effort);
        if self.tasks.form.kind_is_auto {
            let recommended = capability::select(&self.core.capability_candidates(&choices, effort), level).map(|s| s.kind);
            if let Some(kind) = recommended.filter(|k| *k != self.tasks.form.kind) {
                self.tasks.form.kind = kind.clone();
                self.tasks.form.model = None;
                self.request_models(&kind);
            }
        }
        let f = &self.tasks.form;
        let effort = f.effort.filter(|_| models::effort_unavailable(&f.kind, f.model.as_deref()).is_none());
        let note = self.core.capability_advisory(&f.kind, f.model.as_deref(), f.level, effort);
        self.tasks.form.capacity_note = note;
    }

    /// Chose another size of work in the form.
    pub fn tasks_form_set_level(&mut self, level: Level) {
        self.tasks.form.level = level;
        self.tasks_form_refresh();
    }

    /// Chose another agent in the form: the model and the effort go back to the default.
    pub fn tasks_form_set_kind(&mut self, kind: String) {
        if self.tasks.form.kind != kind {
            self.tasks.form.model = None;
            self.tasks.form.effort = None;
            self.tasks.form.kind = kind.clone();
        }
        self.tasks.form.kind_is_auto = false;
        self.tasks.form.menu = None;
        self.tasks.form.provider = None;
        self.request_models(&kind);
        self.tasks_form_refresh();
    }

    /// "Attach files" button of the "New task" form.
    pub fn tasks_pick_attachments(&self) {
        self.pick_attachments_for(AttachTarget::TaskForm(self.active_project.clone()));
    }

    /// "×" of the attachment chip in the "New task" form: removes it from the list and deletes the copy in `dados/anexos`.
    pub fn tasks_remove_attachment(&mut self, i: usize) {
        if i >= self.tasks.form.attachments.len() {
            return;
        }
        let path = self.tasks.form.attachments.remove(i);
        self.delete_attachment_copy(path);
    }

    /// Creates the task (and starts it, with "Start now") in a thread.
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
            level: f.level,
            model: f.model.clone(),
            // The effort chosen before switching model may no longer apply (Antigravity without per-effort versions).
            effort: f.effort.filter(|_| models::effort_unavailable(&f.kind, f.model.as_deref()).is_none()),
            base: Some(f.base.trim().to_owned()).filter(|b| !b.is_empty()),
            creator: None,
            scope: parse_scope(&f.scope),
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

    /// Runs a lifecycle operation in a thread.
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

    /// Click on a button of the detail.
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

    /// "Stash my changes, merge and give them back" in the refusal notice: with no other confirmation, the click is already the request.
    pub fn tasks_merge_stash(&mut self, id: &str) {
        self.tasks_run(id.to_owned(), TaskOp::MergeStash);
    }

    /// "Ask the agent to commit" / "…resolve the conflict": sends the task back with the request ready from the refusal reason.
    pub fn tasks_ask_agent(&mut self, id: &str) {
        let Some(task) = self.tasks.task(id) else { return };
        let Some(feedback) = self.tasks.notice_block.as_ref().and_then(|b| b.return_feedback(task)) else { return };
        self.tasks_run(id.to_owned(), TaskOp::Return(feedback));
    }

    /// The "yes" of the confirmation panel.
    pub fn tasks_confirm_accepted(&mut self) {
        match self.tasks.confirm.take() {
            Some(Confirm::Merge(id)) => self.tasks_run(id, TaskOp::Merge),
            Some(Confirm::Discard(id)) => self.tasks_run(id, TaskOp::Discard),
            None => {}
        }
    }

    /// Sends the form's feedback: "Return" (in review) or "Reject" (awaiting validation).
    pub fn tasks_send_return(&mut self, id: &str) {
        let feedback = self.tasks.feedback.trim().to_owned();
        let Some(task) = self.tasks.task(id) else { return };
        if feedback.is_empty() {
            return;
        }
        let op = if task.status == TaskStatus::Validating { TaskOp::Reject(feedback) } else { TaskOp::Return(feedback) };
        self.tasks_run(id.to_owned(), op);
    }

    /// Is the task's agent tab open?
    pub fn tasks_tab_open(&self, task: &Task) -> bool {
        self.tabs.iter().any(|t| t.project == self.active_project && t.ident.id() == task.instance)
    }

    pub fn on_tasks(&mut self, generation: u64, reply: TaskReply) {
        if generation != self.tasks.generation || self.tasks.debug_hold {
            return; // requested in the previous project (or a fake screenshot state)
        }
        match reply {
            TaskReply::List(list) => {
                self.tasks.refreshing = false;
                // On the project's first read: if something stayed in the queue (the app closed between the task's turn
                // arriving and it starting), the core checks whether it can start now.
                if !self.tasks.loaded && list.iter().any(|t| t.status == TaskStatus::Queued) {
                    let project = self.active_project.clone();
                    self.spawn_bg("tasks-queue", move |core| {
                        core.tasks_advance_queue(&project);
                        None
                    });
                }
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
                        // Done: the form goes back empty (agent, base and "Start now" stay).
                        let form = &mut self.tasks.form;
                        form.title.clear();
                        form.brief.clear();
                        form.scope.clear();
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
                        // The merge went through, but there is something to say about the stashed changes (they came back, or stayed in the stash).
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
                        // The notice only shows in the task's detail: an action refused through the card's button (in the
                        // list), or while looking at another task or the form, opens its detail, otherwise the
                        // refusal goes by unseen (or shows on the wrong task).
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

    /// The open detail points to a task that disappeared from the list: back to the list.
    fn tasks_drop_stale_view(&mut self) {
        if let TasksView::Detail(id) = &self.tasks.view {
            if self.tasks.task(id).is_none() {
                self.tasks.view = TasksView::List;
                self.tasks_clear_detail_state();
            }
        }
    }

    /// Called when the right bar starts showing the Tasks tab.
    pub fn tasks_on_open(&mut self) {
        self.tasks_refresh();
    }
}

// ---------------------------------------------------------------------------
// Fake states (screenshots, `AGENTDECK_DEBUG_UI=tasks-*`)
// ---------------------------------------------------------------------------

/// One task of each status, with realistic data.
pub fn demo_tasks() -> Vec<Task> {
    let now = agentdeck_core::util::now_ms();
    let min = 60_000u64;
    let ev = |ago_min: u64, text: &str| TaskEvent { at_ms: now - ago_min * min, text: text.to_owned() };
    let msg = |ago_min: u64, kind: FeedKind, from: &str, to: &str, text: &str| TaskMessage {
        at_ms: now - ago_min * min,
        kind,
        from: Some(from.to_owned()),
        to: Some(to.to_owned()),
        text: text.to_owned(),
    };
    let base = |id: &str, title: &str, status: TaskStatus, kind: &str, model: Option<&str>| Task {
        id: id.to_owned(),
        title: title.to_owned(),
        status,
        base: "dev".into(),
        branch: format!("tarefa/{id}-{}", agentdeck_core::tasks::slugify(title, 40)),
        worktree: format!(r"C:\Users\you\agentdeck-dev\.agentdeck\worktrees\{id}").into(),
        kind: kind.to_owned(),
        model: model.map(str::to_owned),
        instance: format!("{kind}@{id}"),
        ..Task::default()
    };

    let review = Task {
        brief: "Show a **Tasks** tab in the right bar with the list grouped by status.\n\n- card with the title, the agent and the time\n- detail with the report and the diff\n- merge, return and discard buttons".into(),
        created_ms: now - 95 * min,
        started_ms: Some(now - 90 * min),
        finished_ms: Some(now - 6 * min),
        report: Some("I built the whole Tasks tab.\n\nFiles: crates/app/src/ui/tasks.rs (painting), crates/app/src/tasks_ui.rs (state and actions), right_panel.rs (icon and badge).\n\nTests: cargo test --workspace passes; the pure functions (groups, diff summary, actions per status) have tests.\n\nMissing: check the tab on Windows with several tasks at the same time.".into()),
        diff_stat: Some(" crates/app/src/tasks_ui.rs       | 412 +++++++++++++++++++++++++++\n crates/app/src/ui/tasks.rs      | 688 ++++++++++++++++++++++++++++++++++++\n crates/app/src/ui/right_panel.rs |  14 +-\n crates/core/src/tasks.rs        |  23 ++\n 4 files changed, 1129 insertions(+), 8 deletions(-)\n".into()),
        events: vec![
            ev(95, "created"),
            ev(90, "started: worktree C:\\Users\\you\\agentdeck-dev\\.agentdeck\\worktrees\\t4 on branch tarefa/t4-tasks-tab-in-the-right-bar"),
            ev(90, "brief delivered to the agent"),
            ev(6, "report received; in review"),
        ],
        effort: Some(Effort::High),
        ..base("t4", "Tasks tab in the right bar", TaskStatus::Review, "claude", Some("claude-opus-5-5"))
    };
    let review = Task {
        messages: vec![
            msg(92, FeedKind::Ask, "claude@t4", "antigravity", "How does egui clip the text of a galley that was already measured? I need an ellipsis at the end without measuring again on every frame."),
            msg(91, FeedKind::Answer, "antigravity", "claude@t4", "Use `galley_trunc` from ui/mod.rs: it already cuts with an ellipsis and returns the final galley.\n\nIf the text changes on every frame, cache the galley by the text's fingerprint.\n\nThe big cost is measuring; painting is cheap. For long lists, only measure the visible rows (ScrollArea::show_rows) and keep a fixed height per row.\n\nExample:\n  let g = galley_trunc(painter, text, font, color, max_w);\n  place(painter, rect, Align2::LEFT_CENTER, g, color);"),
            msg(40, FeedKind::Ask, "claude@t4", "opencode", "Can you review the app crate diff?"),
            msg(39, FeedKind::Error, "opencode", "claude@t4", "No quota: the deepseek-v4.1-flash-free model used up the week."),
        ],
        ..review
    };
    let review2 = Task {
        created_ms: now - 50 * min,
        started_ms: Some(now - 48 * min),
        finished_ms: Some(now - 2 * 60 * min),
        report: Some("Fixed the handle leak in the PTY.".into()),
        diff_stat: Some(" crates/app/src/terminal/mod.rs | 6 +++---\n 1 file changed, 3 insertions(+), 3 deletions(-)\n".into()),
        events: vec![ev(150, "report received; in review")],
        effort: Some(Effort::High),
        ..base("t5", "Fix handle leak when closing the tab", TaskStatus::Review, "antigravity", Some("gemini-4-argon-high"))
    };
    let running = Task {
        created_ms: now - 25 * min,
        started_ms: Some(now - 24 * min),
        events: vec![ev(25, "created"), ev(24, "started: worktree … on branch tarefa/t6")],
        effort: Some(Effort::Medium),
        scope: vec!["crates/core".into(), "memory/ARCHITECTURE.md".into()],
        ..base("t6", "Document the bridge /tasks routes", TaskStatus::Running, "claude", Some("claude-sonnet-5-5"))
    };
    let queued = Task {
        brief: "Replace the git executor with one that reuses the process.".into(),
        created_ms: now - 12 * min,
        queued_ms: Some(now - 12 * min),
        waiting_for: Some("t6".into()),
        scope: vec!["crates/core/src/git.rs".into()],
        events: vec![ev(12, "created"), ev(12, "queued: the scope overlaps t6's; starts when it finishes")],
        ..base("t9", "Reuse the git process", TaskStatus::Queued, "claude", Some("claude-sonnet-5-5"))
    };
    let draft = Task {
        brief: "Run clippy with -D warnings and fix whatever shows up.".into(),
        created_ms: now - 3 * 60 * min,
        events: vec![ev(180, "created")],
        ..base("t7", "Clean up the clippy warnings in the app crate", TaskStatus::Draft, "opencode", None)
    };
    let failed = Task {
        created_ms: now - 5 * 60 * min,
        error: Some("could not create the worktree: branch tarefa/t3-new-branch already exists in this repository".into()),
        events: vec![
            ev(300, "created"),
            ev(299, "error: could not create the worktree: branch tarefa/t3-new-branch already exists in this repository"),
        ],
        ..base("t3", "Swap the report's markdown parser", TaskStatus::Failed, "opencode", Some("opencode/deepseek-v4.1-flash-free"))
    };
    let validating = Task {
        brief: "Show the merge status in the detail header.".into(),
        created_ms: now - 3 * 60 * min,
        started_ms: Some(now - 170 * min),
        finished_ms: Some(now - 8 * min),
        report: Some(
            "The detail header shows the merge.\n\nFiles: crates/app/src/ui/tasks.rs.\n\nTests: cargo test --workspace passes.".into(),
        ),
        diff_stat: Some(" crates/app/src/ui/tasks.rs | 18 ++++++++++++++----\n 1 file changed, 14 insertions(+), 4 deletions(-)\n".into()),
        merge_commit: Some("a1b2c3d4e5f60123456789abcdef0123456789ab".into()),
        events: vec![
            ev(180, "created"),
            ev(170, "started: worktree … on branch tarefa/t8"),
            ev(20, "report received; in review"),
            ev(8, "merged into dev (a1b2c3d); awaiting validation"),
        ],
        ..base("t8", "Merge status in the detail header", TaskStatus::Validating, "claude", Some("claude-sonnet-5-5"))
    };
    let merged = Task {
        created_ms: now - 26 * 60 * min,
        started_ms: Some(now - 26 * 60 * min),
        finished_ms: Some(now - 24 * 60 * min),
        events: vec![ev(24 * 60, "validated; completed; worktree and branch removed")],
        ..base("t2", "/tasks routes on the bridge", TaskStatus::Merged, "claude", Some("claude-opus-5-5"))
    };
    let discarded = Task {
        created_ms: now - 50 * 60 * min,
        finished_ms: Some(now - 48 * 60 * min),
        events: vec![ev(48 * 60, "discarded; worktree and branch removed")],
        ..base("t1", "Galley cache experiment", TaskStatus::Discarded, "claude", None)
    };
    vec![discarded, merged, failed, review, running, queued, draft, review2, validating]
}

#[cfg(test)]
mod tests {
    use super::*;
    use agentdeck_core::i18n::{with_lang, Lang};

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
        // Empty groups do not show up.
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
        assert_eq!(Group::Validating.title(), "Awaiting validation");
        assert_eq!(Group::of(TaskStatus::Validating), Group::Validating);
        assert_eq!(status_name(TaskStatus::Validating), "Awaiting validation");
        // "Completed" is the name of the validated task (the stored status is still `Merged`).
        assert_eq!(status_name(TaskStatus::Merged), "Completed");
        // The groups that wait for the human count on the badge; the validated ones do not.
        let ui = TasksUi { tasks, ..TasksUi::default() };
        assert_eq!((ui.review_count(), ui.validating_count(), ui.attention_count()), (1, 2, 3));
        // The "Awaiting validation" group starts open: it is work for you.
        assert!(!ui.collapsed.contains(&Group::Validating));
    }

    #[test]
    fn cancelled_tasks_are_apart_from_the_merged_ones() {
        assert_eq!(Group::of(TaskStatus::Merged), Group::Done);
        assert_eq!(Group::of(TaskStatus::Discarded), Group::Cancelled);
        assert_eq!(Group::Done.title(), "Finished");
        assert_eq!(Group::Cancelled.title(), "Cancelled");
        // Both at the end, in that order.
        let n = Group::ORDER.len();
        assert_eq!(Group::ORDER[n - 2..], [Group::Done, Group::Cancelled]);
        // The resolved groups start collapsed.
        let ui = TasksUi::default();
        assert!(ui.collapsed.contains(&Group::Done));
        assert!(ui.collapsed.contains(&Group::Cancelled));
        assert!(!ui.collapsed.contains(&Group::Review));
    }

    #[test]
    fn the_real_list_with_two_discarded_keeps_them_out_of_the_merged_group() {
        // The case that showed up in the review: t8 and t4 discarded living alongside t1 and t2 merged.
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
        // No discarded task can be left in "Finished".
        let done = groups.iter().find(|(g, _)| *g == Group::Done).expect("finished group");
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
        let s = parse_diff_stat(stat).expect("summary");
        assert_eq!(s, DiffSummary { files: 3, insertions: 120, deletions: 8 });
        assert_eq!(s.text(), "3 files, +120 −8");
        // Singular and only one of the sides.
        assert_eq!(parse_diff_stat(" a.rs | 1 +\n 1 file changed, 1 insertion(+)\n").map(|s| s.text()).as_deref(), Some("1 file, +1"));
        assert_eq!(parse_diff_stat(" 2 files changed, 4 deletions(-)").map(|s| s.text()).as_deref(), Some("2 files, −4"));
        // No differences (the core's text, in both languages) and garbage.
        assert_eq!(parse_diff_stat("(no changes against the base)").map(|s| s.text()).as_deref(), Some("no changes"));
        assert_eq!(parse_diff_stat("(nenhuma alteração em relação à base)").map(|s| s.text()).as_deref(), Some("no changes"));
        assert_eq!(parse_diff_stat(""), None);
        assert_eq!(parse_diff_stat("something else"), None);
    }

    #[test]
    fn the_stat_lists_one_line_per_file() {
        let stat = " crates/app/src/tasks_ui.rs | 412 +++++++++\n a b/c.rs => d.rs |  3 +-\n logo.png | Bin 0 -> 1234 bytes\n 3 files changed, 415 insertions(+), 1 deletion(-)\n";
        let lines = parse_stat_lines(stat);
        let shape: Vec<(&str, &str)> = lines.iter().map(|l| (l.path.as_str(), l.changes.as_str())).collect();
        assert_eq!(shape, [("crates/app/src/tasks_ui.rs", "412"), ("a b/c.rs => d.rs", "3"), ("logo.png", "Bin 0 -> 1234 bytes")]);
        assert!(parse_stat_lines("(no changes against the base)").is_empty());
        assert!(parse_stat_lines("").is_empty());
    }

    #[test]
    fn ages_read_naturally() {
        let now = 10_000_000_000;
        assert_eq!(age_text(now, now), "now");
        assert_eq!(age_text(now, now + 5_000), "now");
        assert_eq!(age_text(now, now - 59_000), "now");
        assert_eq!(age_text(now, now - 60_000), "1 min ago");
        assert_eq!(age_text(now, now - 59 * 60_000), "59 min ago");
        assert_eq!(age_text(now, now - 3_600_000), "1 h ago");
        assert_eq!(age_text(now, now - 23 * 3_600_000), "23 h ago");
        assert_eq!(age_text(now, now - 3 * 86_400_000), "3 d ago");
    }

    #[test]
    fn the_status_line_says_what_happened_and_when() {
        let now = 1_000_000_000;
        let mut t = task("t1", TaskStatus::Review, now - 1000);
        t.finished_ms = Some(now - 3 * 60_000);
        assert_eq!(status_age(&t, now), "ready 3 min ago");
        t.status = TaskStatus::Running;
        t.started_ms = Some(now - 2 * 3_600_000);
        assert_eq!(status_age(&t, now), "started 2 h ago");
        t.status = TaskStatus::Draft;
        t.created_ms = now - 10_000;
        assert_eq!(status_age(&t, now), "created now");
        t.status = TaskStatus::Failed;
        t.created_ms = now - 3_600_000;
        t.events = vec![TaskEvent { at_ms: now - 120_000, text: "error".into() }];
        assert_eq!(status_age(&t, now), "failed 2 min ago");
        t.status = TaskStatus::Validating;
        t.finished_ms = Some(now - 5 * 60_000);
        assert_eq!(status_age(&t, now), "merged 5 min ago");
        t.status = TaskStatus::Merged;
        assert_eq!(status_age(&t, now), "completed 5 min ago");
        // The same lines in Portuguese.
        with_lang(Lang::PtBr, || {
            assert_eq!(status_age(&t, now), "concluída há 5 min");
            t.status = TaskStatus::Draft;
            assert_eq!(status_age(&t, now), "criada há 1 h");
            assert_eq!(age_text(now, now), "agora");
        });
    }

    #[test]
    fn effort_is_shown_next_to_the_model_unless_the_model_name_has_it() {
        let mut t = Task::default();
        assert_eq!((effort_text(&t), model_effort_text(&t)), (None, None));
        t.model = Some("opencode/glm-5.3".into());
        assert_eq!(model_effort_text(&t).as_deref(), Some("glm-5.3"));
        t.model = Some("sonnet".into());
        t.effort = Some(Effort::High);
        let high = Effort::High.label();
        assert_eq!(model_effort_text(&t), Some(format!("sonnet · {high}")));
        t.model = None;
        assert_eq!(model_effort_text(&t).as_deref(), Some(high));
        // Antigravity: the effort is already in the model name.
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
    fn a_queued_task_is_its_own_group_with_only_the_discard_button() {
        let q = Task {
            status: TaskStatus::Queued,
            waiting_for: Some("t2".into()),
            queued_ms: Some(5_000),
            created_ms: 1_000,
            ..Task::default()
        };
        assert_eq!(actions_for(&q, false), [TaskAct::Discard]);
        assert_eq!((Group::of(TaskStatus::Queued), Group::Queued.title()), (Group::Queued, "Queued"));
        // Between "In progress" and "Draft": it already asked to start, but still waits.
        let order: Vec<Group> = Group::ORDER.to_vec();
        let at = |g| order.iter().position(|x| *x == g).expect("group");
        assert!(at(Group::Running) < at(Group::Queued) && at(Group::Queued) < at(Group::Draft));
        assert_eq!(
            (status_name(TaskStatus::Queued), card_state_label(TaskStatus::Queued), task_step(TaskStatus::Queued)),
            ("Queued", "queued", 1)
        );
        assert!(!is_focused_card(TaskStatus::Queued));
        // How long it has been in the queue counts from entering it, not from the creation.
        assert_eq!(status_age(&q, 65_000), "queued 1 min ago");
    }

    #[test]
    fn the_scope_field_takes_one_path_per_line_or_commas() {
        assert!(parse_scope("").is_empty() && parse_scope(" \n , \n").is_empty());
        assert_eq!(
            parse_scope(" crates/core \n\nmemory/GERAL.md, crates/cli\r\nCargo.lock"),
            ["crates/core", "memory/GERAL.md", "crates/cli", "Cargo.lock"]
        );
    }

    #[test]
    fn each_status_has_its_buttons() {
        let t = |status, report: bool| Task { status, base: "dev".into(), report: report.then(|| "done".to_owned()), ..Task::default() };
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
        // A merge that failed earlier (Failed with a report): it is also sent back to the agent.
        assert_eq!(actions_for(&t(TaskStatus::Failed, true), true), [TaskAct::Retry, TaskAct::Return, TaskAct::GoToTab, TaskAct::Discard]);
        assert!(actions_for(&t(TaskStatus::Merged, true), false).is_empty());
        assert!(actions_for(&t(TaskStatus::Discarded, false), false).is_empty());
        // Only the main action comes filled; merge shows the base.
        assert!(TaskAct::Merge.is_primary() && !TaskAct::Discard.is_primary() && !TaskAct::Return.is_primary());
        assert!(TaskAct::Validate.is_primary() && !TaskAct::Reject.is_primary());
        assert_eq!(
            (TaskAct::Validate.label(&t(TaskStatus::Validating, true)), TaskAct::Reject.label(&t(TaskStatus::Validating, true))),
            ("Validate".to_owned(), "Reject".to_owned())
        );
        assert_eq!(TaskAct::Merge.label(&t(TaskStatus::Review, true)), "Merge into dev");
        with_lang(Lang::PtBr, || assert_eq!(TaskAct::Merge.label(&t(TaskStatus::Review, true)), "Mesclar em dev"));
    }

    #[test]
    fn destructive_actions_ask_first() {
        let review = Task { status: TaskStatus::Review, report: Some("x".into()), ..Task::default() };
        assert!(TaskAct::Discard.needs_confirm(&review) && TaskAct::Merge.needs_confirm(&review));
        assert!(!TaskAct::Return.needs_confirm(&review) && !TaskAct::Start.needs_confirm(&review));
        assert!(!TaskAct::Validate.needs_confirm(&review) && !TaskAct::Reject.needs_confirm(&review));
        assert!(!TaskAct::GoToTab.needs_confirm(&review) && !TaskAct::ReopenTab.needs_confirm(&review));
        // Try again: starting does not ask; merging (it already has a report) does.
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
        // Without the merge commit (an old task), awaiting validation or completed has no diff: the branch is the base itself.
        assert!(!can_show_diff(&t(TaskStatus::Validating, true)));
        assert!(!can_show_diff(&t(TaskStatus::Merged, true)));
        // With the commit, both statuses show what was merged.
        assert!(can_show_diff(&merged(TaskStatus::Validating)) && can_show_diff(&merged(TaskStatus::Merged)));
        // Discarded still has no diff, even if it was merged before.
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
            assert_eq!(diff_button_label(&t(status, None)), "View full diff");
            assert_eq!(diff_title(&t(status, None)), "t5 · changes against dev");
        }
        // Rejected and back in review, the old commit is still stored: the diff is the branch's.
        assert_eq!(diff_button_label(&t(TaskStatus::Review, Some(hash))), "View full diff");
        for status in [TaskStatus::Validating, TaskStatus::Merged] {
            assert_eq!(diff_button_label(&t(status, Some(hash))), "View what was merged");
            assert_eq!(diff_title(&t(status, Some(hash))), "t5 · what the merge brought into dev");
        }
        // The detail shows the short hash next to `branch → base`.
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
        assert_eq!(clip_text("short", 10), "short");
        assert_eq!(clip_text("exact", 5), "exact");
        let clipped = clip_text("an action too long", 4);
        assert!(clipped.starts_with("an a\n\n[… text cut on screen"), "{clipped}");
        assert!(clipped.contains("4 of 18"), "{clipped}");
        // The local time has the form dd/mm hh:mm (the time zone depends on the machine).
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
            TaskStatus::Queued,
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
        // Only the unfinished ones (Review, Validating, Running, Queued, Draft, Failed), in order of priority and recency.
        assert_eq!(active_ids, ["t4", "t5", "t8", "t6", "t9", "t7", "t3"]);
        assert!(active.iter().all(|t| !matches!(t.status, TaskStatus::Merged | TaskStatus::Discarded)));

        // History: all of them (Merged + Discarded), from the most recent (t2, 24h ago) to the oldest (t1, 48h ago).
        let all_hist = history_tasks(&tasks, HistoryFilter::All);
        assert_eq!(all_hist.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2", "t1"]);

        // Filter completed only / discarded only.
        let done_only = history_tasks(&tasks, HistoryFilter::Done);
        assert_eq!(done_only.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t2"]);
        let discarded_only = history_tasks(&tasks, HistoryFilter::Discarded);
        assert_eq!(discarded_only.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["t1"]);

        // 5-segment steps and the highlight of the cards that ask for the human's action.
        assert_eq!(task_step(TaskStatus::Draft), 1);
        assert_eq!(task_step(TaskStatus::Running), 2);
        assert_eq!(task_step(TaskStatus::Failed), 2);
        assert_eq!(task_step(TaskStatus::Review), 3);
        assert_eq!(task_step(TaskStatus::Validating), 4);
        assert_eq!(task_step(TaskStatus::Merged), 5);
        assert_eq!(task_step(TaskStatus::Discarded), 0);
        assert!(is_focused_card(TaskStatus::Review) && is_focused_card(TaskStatus::Validating));
        assert!(!is_focused_card(TaskStatus::Running) && !is_focused_card(TaskStatus::Draft));
        assert_eq!(card_state_label(TaskStatus::Review), "in review");
        assert_eq!(card_state_label(TaskStatus::Validating), "awaiting validation");
        assert_eq!(card_state_label(TaskStatus::Merged), "completed");
        assert_eq!(card_state_label(TaskStatus::Discarded), "discarded");
        with_lang(Lang::PtBr, || {
            assert_eq!(card_state_label(TaskStatus::Validating), "aguarda validação");
            assert_eq!(status_name(TaskStatus::Merged), "Concluída");
        });

        // The dd/mm date has 5 characters with a '/' in the middle.
        let ddmm = task_date_ddmm(&tasks[0]);
        assert_eq!((ddmm.len(), ddmm.as_bytes()[2]), (5, b'/'), "{ddmm}");
    }

    fn message(at_ms: u64, text: &str) -> TaskMessage {
        TaskMessage { at_ms, kind: FeedKind::Ask, from: Some("claude@t1".into()), to: None, text: text.into() }
    }

    #[test]
    fn the_timeline_goes_oldest_first_and_names_both_sides() {
        let t = Task { messages: vec![message(30, "c"), message(10, "a"), message(20, "b")], ..Task::default() };
        assert_eq!(messages_in_order(&t).iter().map(|m| m.text.as_str()).collect::<Vec<_>>(), ["a", "b", "c"]);
        assert_eq!(message_route(&t.messages[0]), "claude@t1 → …");
        assert_eq!(
            (message_kind_label(FeedKind::Ask), message_kind_label(FeedKind::Answer), message_kind_label(FeedKind::Error)),
            ("question", "answer", "error")
        );
    }

    #[test]
    fn a_long_message_is_collapsed_by_size_or_by_lines() {
        assert_eq!(message_preview("  short \n"), ("short".into(), false));
        let (cut, more) = message_preview(&"word ".repeat(200));
        assert!(more && cut.ends_with('…') && cut.chars().count() <= MESSAGE_PREVIEW_CHARS + 1, "{cut}");
        let (cut, more) = message_preview("1\n2\n3\n4\n5\n6");
        assert_eq!((cut.as_str(), more), ("1\n2\n3\n4…", true));
        assert!(!message_preview("1\n2\n3\n4").1);
    }
}

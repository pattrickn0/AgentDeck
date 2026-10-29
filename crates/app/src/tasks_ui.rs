//! State and actions of the right bar's "Tasks" tab (the painting lives in `ui/tasks.rs`).
//!
//! Every task call into the core runs git (create, start, merge, discard, diff), so it goes to a
//! thread (`Deck::spawn_bg`) and the answer comes back as `UiMsg::Tasks`. The list is re-read when
//! the tab opens, when the core says so (`CoreEvent::TasksChanged`) and after every action. The pure
//! functions that decide what the screen shows (groups, texts, actions per status) live here too, with tests.

use std::collections::HashSet;
use std::path::PathBuf;

use holt_core::capability::{self, Level};
use holt_core::models::{self, Effort};
use holt_core::{t, FeedKind, MergeBlock, Task, TaskError, TaskEvent, TaskMessage, TaskSpec, TaskStatus};

use crate::deck::{AttachTarget, Deck, UiMsg};

// ---------------------------------------------------------------------------
// Pure functions
// ---------------------------------------------------------------------------

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

/// Filter of the "Finished" lane.
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
#[derive(Debug, Clone, PartialEq)]
pub struct StatLine {
    pub path: String,
    /// Changed lines (`412`), or `Bin 0 -> 1234 bytes` for a binary file.
    pub changes: String,
    /// The share of insertions among the changed lines (see [`stat_plus_share`]).
    pub plus: Option<f32>,
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
            (!path.is_empty() && !changes.is_empty()).then(|| StatLine {
                path: path.to_owned(),
                changes: changes.to_owned(),
                plus: stat_plus_share(rest),
            })
        })
        .collect()
}

/// The +/− split of a file's line in `git diff --stat` (the `++++--` graph): the share of insertions, `None` for a binary
/// file or a line without a graph.
pub fn stat_plus_share(stat_line_rest: &str) -> Option<f32> {
    let graph = stat_line_rest.split_whitespace().nth(1)?;
    let plus = graph.chars().filter(|c| *c == '+').count();
    let minus = graph.chars().filter(|c| *c == '-').count();
    (plus + minus > 0 && plus + minus == graph.chars().count()).then(|| plus as f32 / (plus + minus) as f32)
}

// ---------------------------------------------------------------------------
// The queue: lanes, marks, the lifecycle track and the timeline
// ---------------------------------------------------------------------------

/// The list's sections, by who has the next move.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Lane {
    /// The person: review, test, a merge that stopped, a failure.
    NeedsYou,
    /// An agent: working, resolving a conflict, or queued behind another task.
    Working,
    /// Nobody yet.
    Drafts,
    /// Completed or discarded.
    Finished,
}

impl Lane {
    pub const ORDER: [Lane; 4] = [Lane::NeedsYou, Lane::Working, Lane::Drafts, Lane::Finished];

    pub fn title(self) -> &'static str {
        match self {
            Lane::NeedsYou => t!("Needs you"),
            Lane::Working => t!("Agents working"),
            Lane::Drafts => t!("Drafts"),
            Lane::Finished => t!("Finished"),
        }
    }

    pub fn of(status: TaskStatus) -> Lane {
        match status {
            TaskStatus::Review | TaskStatus::Validating | TaskStatus::Failed => Lane::NeedsYou,
            TaskStatus::Running | TaskStatus::Queued => Lane::Working,
            TaskStatus::Draft => Lane::Drafts,
            TaskStatus::Merged | TaskStatus::Discarded => Lane::Finished,
        }
    }
}

/// Since when the task waits for its next move (ms): what orders a lane.
pub fn waiting_since(task: &Task) -> u64 {
    match task.status {
        TaskStatus::Review | TaskStatus::Validating | TaskStatus::Failed => task.finished_ms.unwrap_or_else(|| last_activity(task)),
        TaskStatus::Running => task.started_ms.unwrap_or(task.created_ms),
        TaskStatus::Queued => task.queued_ms.unwrap_or(task.created_ms),
        TaskStatus::Draft => task.created_ms,
        TaskStatus::Merged | TaskStatus::Discarded => task.finished_ms.unwrap_or_else(|| last_activity(task)),
    }
}

/// The order of what needs you: decisions first (review, then test), then what went wrong (a merge that stopped, a
/// failure).
fn needs_rank(task: &Task) -> u8 {
    match mark_of(task) {
        Mark::Review => 0,
        Mark::Validate => 1,
        Mark::Stopped => 2,
        _ => 3,
    }
}

/// The open lanes (only those with tasks; `Finished` filtered by `filter`), in [`Lane::ORDER`]. What needs you goes by
/// [`needs_rank`] and then from the oldest wait (the first is the next to handle); what works, from the oldest (a
/// conflict being resolved first, the queue last); drafts and finished go from the newest.
pub fn lanes(tasks: &[Task], filter: HistoryFilter) -> Vec<(Lane, Vec<&Task>)> {
    Lane::ORDER
        .iter()
        .filter_map(|&lane| {
            let mut list: Vec<&Task> =
                tasks.iter().filter(|t| Lane::of(t.status) == lane && (lane != Lane::Finished || filter.matches(t.status))).collect();
            match lane {
                Lane::NeedsYou => list.sort_by_key(|t| (needs_rank(t), waiting_since(t), task_number(&t.id))),
                Lane::Working => {
                    list.sort_by_key(|t| (t.status == TaskStatus::Queued, !t.merge_approved, waiting_since(t), task_number(&t.id)))
                }
                Lane::Drafts | Lane::Finished => {
                    list.sort_by_key(|t| (std::cmp::Reverse(waiting_since(t)), std::cmp::Reverse(task_number(&t.id))))
                }
            }
            (!list.is_empty() || lane == Lane::Finished && tasks.iter().any(|t| Lane::of(t.status) == Lane::Finished))
                .then_some((lane, list))
        })
        .collect()
}

/// The 16 px state glyph of a row (one per state, see `ui/tasks.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mark {
    /// Filled ring: your turn to review.
    Review,
    /// Ring with a check: merged, test it.
    Validate,
    /// Amber "!": the automatic merge stopped.
    Stopped,
    /// Red "!".
    Failed,
    /// Pulsing dot in the agent's colour.
    Running,
    /// Amber pulse: resolving a conflict, merges by itself.
    Conflict,
    /// Dashed ring.
    Queued,
    /// Hollow ring.
    Draft,
    /// Green check.
    Done,
    /// Faint ring.
    Discarded,
}

pub fn mark_of(task: &Task) -> Mark {
    match task.status {
        TaskStatus::Review if task.merge_stopped.is_some() => Mark::Stopped,
        TaskStatus::Review => Mark::Review,
        TaskStatus::Validating => Mark::Validate,
        TaskStatus::Failed => Mark::Failed,
        TaskStatus::Running if task.merge_approved => Mark::Conflict,
        TaskStatus::Running => Mark::Running,
        TaskStatus::Queued => Mark::Queued,
        TaskStatus::Draft => Mark::Draft,
        TaskStatus::Merged => Mark::Done,
        TaskStatus::Discarded => Mark::Discarded,
    }
}

/// The state word on a row (short: the row also carries the agent, the model and the diff).
pub fn state_word(task: &Task) -> &'static str {
    match mark_of(task) {
        Mark::Review => t!("Review"),
        Mark::Validate => t!("Test it"),
        Mark::Stopped => t!("Not merged"),
        Mark::Failed => t!("Failed"),
        Mark::Running => t!("Working"),
        Mark::Conflict => t!("Resolving a conflict"),
        Mark::Queued => t!("Queued"),
        Mark::Draft => t!("Draft"),
        Mark::Done => t!("Completed"),
        Mark::Discarded => t!("Discarded"),
    }
}

/// The state sentence at the top of the detail.
pub fn state_sentence(task: &Task) -> &'static str {
    match mark_of(task) {
        Mark::Review => t!("Waiting for your review"),
        Mark::Validate => t!("Merged · test it"),
        Mark::Stopped => t!("Not merged by itself"),
        Mark::Failed => t!("Failed"),
        Mark::Running => t!("Agent working"),
        Mark::Conflict => t!("Resolving a conflict"),
        Mark::Queued => t!("Queued"),
        Mark::Draft => t!("Draft"),
        Mark::Done => t!("Completed"),
        Mark::Discarded => t!("Discarded"),
    }
}

/// `1 h 24`, `12 min`, `<1 min`: how long a stage took.
pub fn duration_text(ms: u64) -> String {
    let min = ms / 60_000;
    match min {
        0 => t!("<1 min").to_owned(),
        1..=59 => t!("{n} min", n = min),
        _ => format!("{} h {:02}", min / 60, min % 60),
    }
}

/// Local `HH:MM` of a moment, or `dd/mm` when it was not today.
pub fn clock_text(at_ms: u64, now_ms: u64) -> String {
    use chrono::TimeZone;
    let (Some(at), Some(now)) =
        (chrono::Local.timestamp_millis_opt(at_ms as i64).single(), chrono::Local.timestamp_millis_opt(now_ms as i64).single())
    else {
        return String::new();
    };
    if at.date_naive() == now.date_naive() {
        at.format("%H:%M").to_string()
    } else {
        at.format("%d/%m").to_string()
    }
}

/// How a stop of the lifecycle track is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopState {
    Done,
    /// Where the task is; `Warn` / `Fail` colour it amber / red.
    Now,
    Warn,
    Fail,
    Later,
}

/// One of the five stops of the track: Created, Worked, Review, Merged, Validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Stop {
    pub label: &'static str,
    pub state: StopState,
    /// The time under it (`12:31`, `1 h 24` for the work, empty when it has not happened).
    pub when: String,
}

/// The lifecycle track of the detail: where the task is, with real times. A conflict being resolved, a queue, a failure
/// or a discard rename the stop where they happened.
pub fn lifecycle(task: &Task, now_ms: u64) -> [Stop; 5] {
    use StopState::*;
    let at = |ms: Option<u64>| ms.map(|m| clock_text(m, now_ms)).unwrap_or_default();
    let worked = task.started_ms.map(|s| duration_text(task.finished_ms.unwrap_or(now_ms).saturating_sub(s))).unwrap_or_default();
    let stop = |label, state, when: String| Stop { label, state, when };
    let created = stop(t!("Created"), Done, at(Some(task.created_ms)));
    let later = |label| stop(label, Later, String::new());
    match task.status {
        TaskStatus::Draft => [
            stop(t!("Created"), Now, at(Some(task.created_ms))),
            later(t!("Worked")),
            later(t!("Review")),
            later(t!("Merged")),
            later(t!("Validated")),
        ],
        TaskStatus::Queued => {
            [created, stop(t!("Queued"), Now, at(task.queued_ms)), later(t!("Review")), later(t!("Merged")), later(t!("Validated"))]
        }
        TaskStatus::Running if task.merge_approved => [
            created,
            stop(t!("Worked"), Done, at(task.started_ms)),
            stop(t!("Conflict"), Warn, at(Some(last_activity(task)))),
            later(t!("Merged")),
            later(t!("Validated")),
        ],
        TaskStatus::Running => {
            [created, stop(t!("Working"), Now, worked), later(t!("Review")), later(t!("Merged")), later(t!("Validated"))]
        }
        TaskStatus::Failed if task.report.is_some() => [
            created,
            stop(t!("Worked"), Done, worked),
            stop(t!("Failed"), Fail, at(Some(last_activity(task)))),
            later(t!("Merged")),
            later(t!("Validated")),
        ],
        TaskStatus::Failed => [
            created,
            stop(t!("Failed"), Fail, at(Some(last_activity(task)))),
            later(t!("Review")),
            later(t!("Merged")),
            later(t!("Validated")),
        ],
        TaskStatus::Review => [
            created,
            stop(t!("Worked"), Done, worked),
            stop(t!("Review"), if task.merge_stopped.is_some() { Warn } else { Now }, at(task.finished_ms)),
            later(t!("Merged")),
            later(t!("Validated")),
        ],
        TaskStatus::Validating => [
            created,
            stop(t!("Worked"), Done, String::new()),
            stop(t!("Review"), Done, String::new()),
            stop(t!("Merged"), Now, at(task.finished_ms)),
            later(t!("Validated")),
        ],
        TaskStatus::Merged => [
            created,
            stop(t!("Worked"), Done, String::new()),
            stop(t!("Review"), Done, String::new()),
            stop(t!("Merged"), Done, String::new()),
            stop(t!("Validated"), Done, at(task.finished_ms)),
        ],
        TaskStatus::Discarded => {
            // The stops it reached stay done; the discard takes the next one.
            let reached = if task.report.is_some() {
                3
            } else if task.started_ms.is_some() {
                2
            } else {
                1
            };
            let mut stops = [created, later(t!("Worked")), later(t!("Review")), later(t!("Merged")), later(t!("Validated"))];
            for s in stops.iter_mut().take(reached).skip(1) {
                s.state = Done;
            }
            if let Some(s) = stops.get_mut(reached) {
                *s = Stop { label: t!("Discarded"), state: Fail, when: at(task.finished_ms) };
            }
            stops
        }
    }
}

/// An item of the detail's timeline: an event of the task or a message between agents.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Moment<'a> {
    Event(&'a TaskEvent),
    Message(&'a TaskMessage),
}

impl Moment<'_> {
    pub fn at_ms(&self) -> u64 {
        match self {
            Moment::Event(e) => e.at_ms,
            Moment::Message(m) => m.at_ms,
        }
    }
}

/// Events and messages in one stream, newest first (at equal times, the message after the event it caused).
pub fn timeline(task: &Task) -> Vec<Moment<'_>> {
    let mut list: Vec<Moment> = task.events.iter().map(Moment::Event).chain(task.messages.iter().map(Moment::Message)).collect();
    list.sort_by_key(|m| (std::cmp::Reverse(m.at_ms()), matches!(m, Moment::Event(_))));
    list
}

/// The agent's last move for a working row: "asked antigravity" / "antigravity answered", with when.
pub fn last_move(task: &Task) -> Option<(String, u64)> {
    let m = task.messages.iter().max_by_key(|m| m.at_ms)?;
    let me = |who: Option<&str>| who == Some(task.instance.as_str());
    let text = match m.kind {
        FeedKind::Ask if me(m.from.as_deref()) => t!("asked {who}", who = m.to.as_deref().unwrap_or("…")),
        FeedKind::Answer if me(m.to.as_deref()) => t!("{who} answered", who = m.from.as_deref().unwrap_or("…")),
        FeedKind::Error => t!("{who} failed", who = m.from.as_deref().unwrap_or("…")),
        _ => t!("{from} → {to}", from = m.from.as_deref().unwrap_or("…"), to = m.to.as_deref().unwrap_or("…")),
    };
    Some((text, m.at_ms))
}

/// Which pane of the detail shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DetailTab {
    Report,
    Changes,
    Timeline,
    Request,
}

impl DetailTab {
    pub const ALL: [DetailTab; 4] = [DetailTab::Report, DetailTab::Changes, DetailTab::Timeline, DetailTab::Request];

    pub fn label(self) -> &'static str {
        match self {
            DetailTab::Report => t!("Report"),
            DetailTab::Changes => t!("Changes"),
            DetailTab::Timeline => t!("Timeline"),
            DetailTab::Request => t!("Request"),
        }
    }

    /// The pane a task opens on: the report once there is one, the timeline while an agent works, the request before.
    pub fn default_for(task: &Task) -> DetailTab {
        if task.report.as_deref().is_some_and(|r| !r.trim().is_empty()) {
            DetailTab::Report
        } else if task.started_ms.is_some() {
            DetailTab::Timeline
        } else {
            DetailTab::Request
        }
    }
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

/// `now`, `5 min`, `3 h`, `2 d`: the age on a row, where the room is short.
pub fn age_short(now_ms: u64, then_ms: u64) -> String {
    let secs = now_ms.saturating_sub(then_ms) / 1000;
    match secs {
        0..=59 => t!("now").to_owned(),
        60..=3599 => t!("{n} min", n = secs / 60),
        3600..=86_399 => t!("{n} h", n = secs / 3600),
        _ => t!("{n} d", n = secs / 86_400),
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

/// A collapsed message shows at most this much (in characters); a click opens it whole.
pub const MESSAGE_PREVIEW_CHARS: usize = 280;

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
    /// Files attached to the request (absolute paths in `data/attachments`).
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
    /// "Branch and scope" is unfolded.
    pub more_open: bool,
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
            more_open: false,
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
    pub history_filter: HistoryFilter,
    pub history_menu: bool,
    /// The pane the person picked in the detail (`None` = [`DetailTab::default_for`] the task).
    pub detail_tab: Option<DetailTab>,
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
            history_filter: HistoryFilter::All,
            history_menu: false,
            detail_tab: None,
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
    /// "Cancel automatic merge" on a task whose conflict was handed to the agent.
    CancelAutoMerge,
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
            TaskOp::CancelAutoMerge => t!("Cancelling…"),
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
    /// answers that still come from it. The "Finished" filter stays.
    pub fn tasks_reset_for_project(&mut self) {
        let generation = self.tasks.generation + 1;
        let history_filter = self.tasks.history_filter;
        self.tasks = TasksUi { generation, history_filter, ..TasksUi::default() };
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
        self.tasks.detail_tab = None;
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

    /// "×" of the attachment chip in the "New task" form: removes it from the list and deletes the copy in `data/attachments`.
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
                TaskOp::CancelAutoMerge => core.task_cancel_auto_merge(&project, &id),
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

    /// "Cancel automatic merge": the task stays with the agent and waits for the review when it delivers.
    pub fn tasks_cancel_auto_merge(&mut self, id: &str) {
        self.tasks_run(id.to_owned(), TaskOp::CancelAutoMerge);
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
// Fake states (screenshots, `HOLT_DEBUG_UI=tasks-*`)
// ---------------------------------------------------------------------------

/// One task of each status, with realistic data.
pub fn demo_tasks() -> Vec<Task> {
    let now = holt_core::util::now_ms();
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
        branch: format!("task/{id}-{}", holt_core::tasks::slugify(title, 40)),
        worktree: format!(r"C:\Users\you\holt-dev\.holt\worktrees\{id}").into(),
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
            ev(90, "started: worktree C:\\Users\\you\\holt-dev\\.holt\\worktrees\\t4 on branch task/t4-tasks-tab-in-the-right-bar"),
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
        events: vec![ev(150, "report received; in review"), ev(120, "merge refused: conflict in crates/app/src/terminal/mod.rs")],
        effort: Some(Effort::High),
        merge_stopped: Some(holt_core::tasks::MergeStop {
            reason: "conflict with dev in crates/app/src/terminal/mod.rs".into(),
            conflict_files: Some(vec!["crates/app/src/terminal/mod.rs".into()]),
        }),
        ..base("t5", "Fix handle leak when closing the tab", TaskStatus::Review, "antigravity", Some("gemini-4-argon-high"))
    };
    let running = Task {
        created_ms: now - 25 * min,
        started_ms: Some(now - 24 * min),
        events: vec![ev(25, "created"), ev(24, "started: worktree … on branch task/t6")],
        messages: vec![msg(
            2,
            FeedKind::Ask,
            "claude@t6",
            "antigravity",
            "Which status codes does the bridge return for a task action from another agent?",
        )],
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
        error: Some("could not create the worktree: branch task/t3-new-branch already exists in this repository".into()),
        events: vec![
            ev(300, "created"),
            ev(299, "error: could not create the worktree: branch task/t3-new-branch already exists in this repository"),
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
            ev(170, "started: worktree … on branch task/t8"),
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
    let conflict = Task {
        brief: "Long messages between agents are saved to a file and only a notice is typed.".into(),
        created_ms: now - 150 * min,
        started_ms: Some(now - 148 * min),
        finished_ms: Some(now - 20 * min),
        report: Some("Mail is saved under data/mail and the tab gets a one-line notice.".into()),
        diff_stat: Some(" crates/core/src/mail.rs   | 210 ++++++++++++++++++++\n crates/core/src/bridge.rs |  34 +++---\n 2 files changed, 230 insertions(+), 14 deletions(-)\n".into()),
        merge_approved: true,
        conflict_asks: 1,
        events: vec![
            ev(150, "created"),
            ev(148, "started: worktree … on branch task/t10"),
            ev(20, "report received; in review"),
            ev(3, "merge conflict with dev in crates/core/src/mail.rs; the agent was asked to resolve it (1 of 3)"),
        ],
        messages: vec![
            msg(60, FeedKind::Ask, "claude@t10", "antigravity", "Is a rename atomic on Windows if the target exists?"),
            msg(58, FeedKind::Answer, "antigravity", "claude@t10", "Use the inbox file per instance; renames are atomic on NTFS when both paths are on the same volume."),
        ],
        ..base("t10", "Durable mail between agents", TaskStatus::Running, "claude", Some("claude-opus-5-5"))
    };
    vec![discarded, merged, failed, review, running, queued, draft, review2, validating, conflict]
}

#[cfg(test)]
mod tests {
    use super::*;
    use holt_core::i18n::{with_lang, Lang};

    fn task(id: &str, status: TaskStatus, activity: u64) -> Task {
        Task { id: id.into(), status, created_ms: 1, events: vec![TaskEvent { at_ms: activity, text: "x".into() }], ..Task::default() }
    }

    #[test]
    fn the_diff_stat_becomes_a_one_line_summary() {
        let stat = " a.rs | 10 +++++++---\n b.rs | 2 +-\n 3 files changed, 120 insertions(+), 8 deletions(-)\n";
        let s = parse_diff_stat(stat).expect("summary");
        assert_eq!(s, DiffSummary { files: 3, insertions: 120, deletions: 8 });
        // Singular and only one of the sides.
        let one = DiffSummary { files: 1, insertions: 1, deletions: 0 };
        assert_eq!(parse_diff_stat(" a.rs | 1 +\n 1 file changed, 1 insertion(+)\n"), Some(one));
        assert_eq!(parse_diff_stat(" 2 files changed, 4 deletions(-)"), Some(DiffSummary { files: 2, insertions: 0, deletions: 4 }));
        // No differences (the core's text, in both languages) and garbage.
        let none = DiffSummary { files: 0, insertions: 0, deletions: 0 };
        assert_eq!(parse_diff_stat("(no changes against the base)"), Some(none));
        assert_eq!(parse_diff_stat("(nenhuma alteração em relação à base)"), Some(none));
        assert_eq!(parse_diff_stat(""), None);
        assert_eq!(parse_diff_stat("something else"), None);
    }

    #[test]
    fn the_stat_lists_one_line_per_file() {
        let stat = " crates/app/src/tasks_ui.rs | 412 +++++++++\n a b/c.rs => d.rs |  3 +-\n logo.png | Bin 0 -> 1234 bytes\n 3 files changed, 415 insertions(+), 1 deletion(-)\n";
        let lines = parse_stat_lines(stat);
        let shape: Vec<(&str, &str)> = lines.iter().map(|l| (l.path.as_str(), l.changes.as_str())).collect();
        assert_eq!(shape, [("crates/app/src/tasks_ui.rs", "412"), ("a b/c.rs => d.rs", "3"), ("logo.png", "Bin 0 -> 1234 bytes")]);
        // The +/− split comes from the graph; a binary file has none.
        let shares: Vec<Option<f32>> = lines.iter().map(|l| l.plus).collect();
        assert_eq!(shares, [Some(1.0), Some(0.5), None]);
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
    fn a_queued_task_works_in_the_agents_lane_with_only_the_discard_button() {
        let q = Task {
            status: TaskStatus::Queued,
            waiting_for: Some("t2".into()),
            queued_ms: Some(5_000),
            created_ms: 1_000,
            ..Task::default()
        };
        assert_eq!(actions_for(&q, false), [TaskAct::Discard]);
        assert_eq!((Lane::of(TaskStatus::Queued), mark_of(&q), state_word(&q)), (Lane::Working, Mark::Queued, "Queued"));
        assert_eq!(status_name(TaskStatus::Queued), "Queued");
        // How long it has been in the queue counts from entering it, not from the creation.
        assert_eq!(status_age(&q, 65_000), "queued 1 min ago");
        assert_eq!(waiting_since(&q), 5_000);
    }
    #[test]
    fn the_scope_field_takes_one_path_per_line_or_commas() {
        assert!(parse_scope("").is_empty() && parse_scope(" \n , \n").is_empty());
        assert_eq!(
            parse_scope(" crates/core \n\nmemory/OVERVIEW.md, crates/cli\r\nCargo.lock"),
            ["crates/core", "memory/OVERVIEW.md", "crates/cli", "Cargo.lock"]
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
        // Merge shows the base.
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
            branch: "task/t5-x".into(),
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
        assert_eq!(branch_line(&t(TaskStatus::Validating, Some(hash))), "task/t5-x → dev · 0123456");
        assert_eq!(branch_line(&t(TaskStatus::Running, None)), "task/t5-x → dev");
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
    fn the_demo_falls_into_lanes_by_who_moves_next() {
        let tasks = demo_tasks();
        let shape = |filter| -> Vec<(Lane, Vec<String>)> {
            lanes(&tasks, filter).into_iter().map(|(l, list)| (l, list.iter().map(|t| t.id.clone()).collect())).collect()
        };
        let ids = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        // Needs you: the review first, then the test, the merge that stopped and the failure. Working: the conflict
        // being resolved, the plain work, the queue last.
        assert_eq!(
            shape(HistoryFilter::All),
            vec![
                (Lane::NeedsYou, ids(&["t4", "t8", "t5", "t3"])),
                (Lane::Working, ids(&["t10", "t6", "t9"])),
                (Lane::Drafts, ids(&["t7"])),
                (Lane::Finished, ids(&["t2", "t1"])),
            ]
        );
        // The filter only touches "Finished"; an empty filter keeps the lane (with its sentence).
        assert_eq!(shape(HistoryFilter::Done).last(), Some(&(Lane::Finished, ids(&["t2"]))));
        assert_eq!(shape(HistoryFilter::Discarded).last(), Some(&(Lane::Finished, ids(&["t1"]))));
        let open: Vec<Task> = tasks.iter().filter(|t| Lane::of(t.status) != Lane::Finished).cloned().collect();
        assert!(lanes(&open, HistoryFilter::All).iter().all(|(l, _)| *l != Lane::Finished));
        assert!(lanes(&[], HistoryFilter::All).is_empty());

        // The marks and words of each state.
        let by_id = |id: &str| tasks.iter().find(|t| t.id == id).expect("task");
        let marks: Vec<(Mark, &str)> = ["t4", "t8", "t5", "t3", "t10", "t6", "t9", "t7", "t2", "t1"]
            .iter()
            .map(|id| (mark_of(by_id(id)), state_word(by_id(id))))
            .collect();
        assert_eq!(
            marks,
            [
                (Mark::Review, "Review"),
                (Mark::Validate, "Test it"),
                (Mark::Stopped, "Not merged"),
                (Mark::Failed, "Failed"),
                (Mark::Conflict, "Resolving a conflict"),
                (Mark::Running, "Working"),
                (Mark::Queued, "Queued"),
                (Mark::Draft, "Draft"),
                (Mark::Done, "Completed"),
                (Mark::Discarded, "Discarded"),
            ]
        );
        with_lang(Lang::PtBr, || {
            assert_eq!(state_word(by_id("t8")), "Teste");
            assert_eq!(status_name(TaskStatus::Merged), "Concluída");
        });

        // The dd/mm date has 5 characters with a '/' in the middle.
        let ddmm = task_date_ddmm(&tasks[0]);
        assert_eq!((ddmm.len(), ddmm.as_bytes()[2]), (5, b'/'), "{ddmm}");
    }

    #[test]
    fn the_track_shows_where_each_task_is() {
        let tasks = demo_tasks();
        let now = holt_core::util::now_ms();
        let track = |id: &str| -> Vec<(&'static str, StopState)> {
            lifecycle(tasks.iter().find(|t| t.id == id).expect("task"), now).into_iter().map(|s| (s.label, s.state)).collect()
        };
        use StopState::*;
        assert_eq!(track("t7"), [("Created", Now), ("Worked", Later), ("Review", Later), ("Merged", Later), ("Validated", Later)]);
        assert_eq!(track("t9")[1], ("Queued", Now));
        assert_eq!(track("t6")[1], ("Working", Now));
        assert_eq!(track("t4")[2], ("Review", Now));
        assert_eq!(track("t5")[2], ("Review", Warn));
        assert_eq!(track("t10")[2], ("Conflict", Warn));
        assert_eq!(track("t3")[1], ("Failed", Fail));
        assert_eq!(track("t8")[3], ("Merged", Now));
        assert!(track("t2").iter().all(|(_, s)| *s == Done));
        // A discard takes the stop after the last one reached.
        assert_eq!(track("t1")[1], ("Discarded", Fail));
        // The work's stop carries how long it took.
        let t4 = tasks.iter().find(|t| t.id == "t4").expect("t4");
        assert_eq!(lifecycle(t4, now)[1].when, "1 h 24");
        assert_eq!(
            (duration_text(30_000), duration_text(12 * 60_000), duration_text(125 * 60_000)),
            ("<1 min".into(), "12 min".into(), "2 h 05".into())
        );
    }

    fn message(at_ms: u64, text: &str) -> TaskMessage {
        TaskMessage { at_ms, kind: FeedKind::Ask, from: Some("claude@t1".into()), to: None, text: text.into() }
    }

    #[test]
    fn the_timeline_mixes_events_and_messages_newest_first() {
        let t = Task {
            instance: "claude@t1".into(),
            messages: vec![message(30, "c"), message(10, "a")],
            events: vec![TaskEvent { at_ms: 20, text: "b".into() }, TaskEvent { at_ms: 30, text: "c-event".into() }],
            ..Task::default()
        };
        let texts: Vec<&str> = timeline(&t)
            .iter()
            .map(|m| match m {
                Moment::Event(e) => e.text.as_str(),
                Moment::Message(m) => m.text.as_str(),
            })
            .collect();
        // At the same time, the message goes after the event that caused it (newest first: above it).
        assert_eq!(texts, ["c", "c-event", "b", "a"]);
        assert_eq!(last_move(&t), Some(("asked …".to_owned(), 30)));
        let answer = TaskMessage {
            at_ms: 40,
            kind: FeedKind::Answer,
            from: Some("antigravity".into()),
            to: Some("claude@t1".into()),
            text: "x".into(),
        };
        let t = Task { messages: vec![answer], ..t };
        assert_eq!(last_move(&t), Some(("antigravity answered".to_owned(), 40)));
        assert_eq!(
            (message_kind_label(FeedKind::Ask), message_kind_label(FeedKind::Answer), message_kind_label(FeedKind::Error)),
            ("question", "answer", "error")
        );
        // The pane a task opens on.
        assert_eq!(DetailTab::default_for(&Task::default()), DetailTab::Request);
        assert_eq!(DetailTab::default_for(&Task { started_ms: Some(1), ..Task::default() }), DetailTab::Timeline);
        assert_eq!(DetailTab::default_for(&Task { report: Some("done".into()), ..Task::default() }), DetailTab::Report);
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

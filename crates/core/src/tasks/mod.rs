//! The tasks desk: Holt's unit of work.
//!
//! A **task** has a request (the brief), an agent with a model, its own worktree + branch, a report, a diff and a
//! lifecycle that ends in validation (completed) or discard:
//!
//! ```text
//! Draft ──start──▶ Running ──done──▶ Review ──merge──▶ Validating ──validate──▶ Merged (completed)
//!   │  │              ▲                 │ └─discard──▶ Discarded     │ └─discard──▶ Discarded
//!   │  │              ├───return────────┘                            │
//!   │  │              └───────────────reject─────────────────────────┘
//!   │  └─ scope taken ─▶ Queued ──(the other leaves Running/Review)──▶ Running
//!   └─ start with an error ─▶ Failed ──(start again)──▶ Running
//! ```
//!
//! **Scope and queue:** a task can declare the paths it will touch ([`Task::scope`], see [`crate::scope`]). On
//! start, if its scope overlaps that of a task that is running or in review (or of another one already waiting in
//! the queue, which arrived earlier), it goes to `Queued`, saying which task it waits for ([`Task::waiting_for`]),
//! and starts by itself when that task is merged, discarded or fails ([`Core::advance_queue`]), departing from the
//! base as it is at that moment. `Validating` does not hold the queue: the code is already in the base. Without a
//! scope, none of this applies.
//!
//! Merging is "the diff looked right"; the real test is done by the user afterwards, in the compiled app. That is
//! why merging does **not** end the agent: the task stays in `Validating` (tab, worktree and branch alive, the
//! branch aligned with the base) until the user validates (cleans everything up) or rejects (back to the same agent).
//!
//! - **Storage:** `data/tasks/<project>.json`, written through a temporary file + `rename` (one write at a
//!   time), outside the user's repository. The counter (`t1`, `t2`…) never reuses a number.
//! - **Worktrees:** `<repository root>/.holt/worktrees/<id>`, inside the repository on purpose (agents confined
//!   to the workspace, like Antigravity, can reach it). The `/.holt/` folder goes once into
//!   `<common git dir>/info/exclude` (never into `.gitignore`).
//! - **Git:** the same safe executor as [`crate::git`] (no shell, no prompts, with a time limit).
//! - **Threads:** the methods that run git block; the interface calls them on a thread. One lifecycle operation at
//!   a time (`TaskStore::op`); queries only take a short lock.
//! - **Agent:** the task's instance is `<model>@t<n>` (`claude@t3`), with the terminal in the worktree
//!   ([`Core::task_cwd`], used by `agents::spawn_spec_for`). The brief and the feedback arrive by the same path as
//!   messages between agents (`CoreEvent::Deliver` with `request: 0`, queue and delivery rules).
//!
//! Who can do what on the bridge is in `bridge.rs`: the task's agent only delivers the report (`done`) of its own
//! task and queries; the rest belongs to whoever created the task (another agent or the user).
//!
//! **Language:** the texts the desk shows (statuses, history events, refusals, notices) go through `t!`; the texts
//! only agents read (the brief, the rules, the feedback messages, the boot line) are plain English. The stored
//! `Feedback::origin` and `Task::creator` values are English (`review`, `validation`, `user`); older files are read too.

use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::capability::Level;
use crate::events::{FeedEntry, FeedKind};
use crate::git::GitError;
use crate::models;
use crate::t;
use crate::util::now_ms;

/// Events kept per task (the oldest leave).
pub const MAX_EVENTS: usize = 200;
/// Feedbacks kept per task (the oldest leave).
pub const MAX_FEEDBACKS: usize = 20;
/// Messages kept per task (the oldest leave).
pub const MAX_MESSAGES: usize = 300;
/// Size of each kept message (in characters); the rest is cut with a notice.
pub const MAX_MESSAGE_CHARS: usize = 6_000;
/// Size of each earlier feedback when it reappears in a message to the agent (in characters).
const HISTORY_ITEM_CHARS: usize = 700;
/// Messages to the agent show only the latest earlier feedbacks.
const HISTORY_SHOWN: usize = 5;
/// Maximum size of the title (in characters).
pub const MAX_TITLE: usize = 120;
/// Maximum size of the title slug in the branch.
pub const MAX_SLUG: usize = 40;
/// A report or brief larger than this is cut (in characters).
pub const MAX_TEXT: usize = 200_000;
/// Times the app asks the agent, by itself, to resolve a conflict found at merge time (the first request and two
/// repeats); after that a human looks ([`Task::merge_stopped`]).
pub const MAX_CONFLICT_ASKS: u32 = 3;
/// Files outside the scope kept per task.
const MAX_OUTSIDE: usize = 100;
/// Size of the kept `git diff --stat` text (in characters).
const MAX_STAT: usize = 8_000;
/// Name of the folder, at the repository root, that holds the worktrees.
pub const HOLT_DIR: &str = ".holt";
/// Attempts (and wait between them) to delete a worktree folder while the agent's terminal still holds it
/// (on Windows the process takes a moment to die after the tab closes).
const REMOVE_TRIES: u32 = 10;
const REMOVE_WAIT: Duration = Duration::from_millis(300);

// ---------- Types ----------

/// Where the task is in its lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskStatus {
    /// Created; still without a worktree or an agent.
    #[default]
    Draft,
    /// Started, but its scope overlaps that of another task in progress: waits its turn, with no worktree or agent.
    /// Starts by itself when the scope is free.
    Queued,
    /// The agent is working in the worktree.
    Running,
    /// The agent delivered the report: waits for the review (merge, return or discard).
    Review,
    /// Merged into the base and tested by the user: completed; worktree and branch removed. (The name `Merged`
    /// stays in the JSON so old tasks do not break; for the user it is "completed".)
    Merged,
    /// Merged into the base, waiting for the user to test it: the agent, the worktree and the branch remain.
    Validating,
    /// Discarded; worktree and branch removed.
    Discarded,
    /// Error on start (or an infrastructure error on merge); the message stays in `Task::error`.
    Failed,
}

impl TaskStatus {
    /// Stable name (the same as in the JSON and on the bridge).
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

    /// Name to show to people.
    pub fn label(self) -> &'static str {
        match self {
            TaskStatus::Draft => t!("draft"),
            TaskStatus::Queued => t!("queued"),
            TaskStatus::Running => t!("in progress"),
            TaskStatus::Review => t!("in review"),
            TaskStatus::Merged => t!("completed"),
            TaskStatus::Validating => t!("awaiting validation"),
            TaskStatus::Discarded => t!("discarded"),
            TaskStatus::Failed => t!("errored"),
        }
    }

    /// Completed or discarded: nothing more to do with it. `Validating` is not final: the agent stays alive.
    pub fn is_terminal(self) -> bool {
        matches!(self, TaskStatus::Merged | TaskStatus::Discarded)
    }
}

/// One line of the task's short history.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskEvent {
    pub at_ms: u64,
    pub text: String,
}

/// A feedback given to the agent: the review's request for changes or the reason for the rejection in validation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Feedback {
    pub at_ms: u64,
    /// `review` or `validation` (a `validation` counts as a rejection in `capability`). Files saved before the English
    /// rename have `revisão` / `validação`; they are read as the English values.
    #[serde(deserialize_with = "legacy_origin")]
    pub origin: String,
    pub text: String,
}

/// Reads a feedback origin, turning the Portuguese values of older files into the English ones.
fn legacy_origin<'de, D: serde::Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    let origin = String::deserialize(d)?;
    Ok(match origin.as_str() {
        "revisão" => "review".to_owned(),
        "validação" => "validation".to_owned(),
        _ => origin,
    })
}

/// Reads a task creator, turning the Portuguese user value of older files into [`crate::instances::USER`].
fn legacy_creator<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    let creator = Option::<String>::deserialize(d)?;
    Ok(creator.map(|c| if c == crate::instances::LEGACY_USER { crate::instances::USER.to_owned() } else { c }))
}

/// A message on the task's timeline: a question, an answer or an error in which its instance speaks or listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskMessage {
    pub at_ms: u64,
    pub kind: FeedKind,
    /// Id of the speaking instance, without the project.
    pub from: Option<String>,
    pub to: Option<String>,
    pub text: String,
}

/// A task. The fields are those of the JSON on disk and of the bridge (`GET /tasks/<id>`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Task {
    /// `t<n>`, a counter per project that never reuses a number.
    pub id: String,
    pub title: String,
    /// The request, in markdown.
    pub brief: String,
    pub status: TaskStatus,
    /// Branch the task departs from and is merged into.
    pub base: String,
    /// `task/<id>-<title slug>`.
    pub branch: String,
    /// Folder (absolute) of the worktree: `<repository root>/.holt/worktrees/<id>`.
    pub worktree: PathBuf,
    /// The size of the work (`simples`, `media`, `complexa`), which decides which models can handle it; it is not the effort.
    /// Old tasks, without the field, are `media`.
    pub level: Level,
    /// Agent (`claude`, `antigravity`, `opencode`, `codex`…).
    pub kind: String,
    /// LLM model (by the rules of `models.rs`); `None` = the CLI's default.
    pub model: Option<String>,
    /// Requested effort (`models::Effort`); `None` = the CLI's default. In Claude it goes in `--effort` when the
    /// tab opens; in Antigravity it is already in the suffix of `model`.
    pub effort: Option<models::Effort>,
    /// Id of the instance (`claude@t3`).
    pub instance: String,
    /// Who created it: the id of an agent (`claude`) or `user` ([`crate::instances::USER`]). It is the instance's
    /// boss when it is an open agent.
    #[serde(default, deserialize_with = "legacy_creator")]
    pub creator: Option<String>,
    pub created_ms: u64,
    pub started_ms: Option<u64>,
    /// When the report arrived (`Review`), when it was merged (`Validating`) or when the task ended (`Merged`/`Discarded`).
    pub finished_ms: Option<u64>,
    /// What the agent delivered in `ask-agent --task done`.
    pub report: Option<String>,
    /// `git diff --stat base...branch` at the moment of the report.
    pub diff_stat: Option<String>,
    /// The error message, in `Failed`.
    pub error: Option<String>,
    /// Full hash of the commit of the last merge into the base (`git rev-parse HEAD` right after `merge --no-ff`);
    /// `None` if the task was never merged. A new merge (after a rejection) overwrites it. It serves the "what was
    /// merged" diff in `Validating`/`Merged` and lets `task_merge` recognize a task merged before.
    pub merge_commit: Option<String>,
    /// Short history (at most [`MAX_EVENTS`]).
    pub events: Vec<TaskEvent>,
    /// The feedbacks already given to the agent, oldest to newest (at most [`MAX_FEEDBACKS`]); they come back in
    /// the return and reject messages so the agent does not repeat what it was already told.
    pub feedbacks: Vec<Feedback>,
    /// Timeline of the `ask-agent` messages in which the task's instance speaks or listens, oldest to newest (at
    /// most [`MAX_MESSAGES`]). See [`Task::record_message`].
    pub messages: Vec<TaskMessage>,
    /// The paths (files and folders, relative to the repository root, already normalized) that the task declared it
    /// will touch; empty = no scope (never queues and never holds anyone). See [`crate::scope`].
    pub scope: Vec<String>,
    /// When it entered the queue (`Queued`); defines the order among those waiting. `None` outside the queue.
    pub queued_ms: Option<u64>,
    /// In `Queued`, the id of the task it waits for.
    pub waiting_for: Option<String>,
    /// The files of the delivered diff that are outside the scope (without `memory/**` and `Cargo.lock`); the review
    /// shows them as a warning, never as a block. Empty without a scope.
    pub outside_scope: Vec<String>,
    /// The merge was already approved (the reviewer clicked "Merge" or ran `--task merge`) but hit a conflict with the
    /// base: the agent was asked to resolve it and the app merges by itself when the agent delivers again. Cleared by
    /// the merge, by [`Core::task_cancel_auto_merge`] and by any return with feedback.
    pub merge_approved: bool,
    /// How many times the app asked the agent, by itself, to resolve a conflict ([`MAX_CONFLICT_ASKS`]).
    pub conflict_asks: u32,
    /// Why the automatic merge of a delivery stopped and a human has to look; the card shows it. `None` otherwise.
    pub merge_stopped: Option<MergeStop>,
}

/// An automatic merge that did not happen ([`Task::merge_stopped`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MergeStop {
    /// The refusal, as the reviewer reads it.
    pub reason: String,
    /// The conflicting files, when the stop was a conflict (the card then offers "ask the agent to resolve").
    pub conflict_files: Option<Vec<String>>,
}

impl Task {
    /// Does the feed entry belong to this task's timeline? Only the questions, answers and errors in which the
    /// task's instance is the sender or the receiver (the "thinking" indicator is transient; the system notice has
    /// no sender and the task already records what matters in its history).
    pub fn owns_message(&self, entry: &FeedEntry) -> bool {
        matches!(entry.kind, FeedKind::Ask | FeedKind::Answer | FeedKind::Error)
            && !self.instance.is_empty()
            && (entry.from.as_deref() == Some(self.instance.as_str()) || entry.to.as_deref() == Some(self.instance.as_str()))
    }

    /// Stores the entry on the timeline (text cut at [`MAX_MESSAGE_CHARS`]; the oldest leave beyond
    /// [`MAX_MESSAGES`]). Returns `false`, without storing, if it is not this task's.
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

    /// Stores a feedback delivered to the agent (the oldest leave beyond [`MAX_FEEDBACKS`]).
    pub fn add_feedback(&mut self, origin: &str, text: &str) {
        self.feedbacks.push(Feedback { at_ms: now_ms(), origin: origin.to_owned(), text: text.to_owned() });
        if self.feedbacks.len() > MAX_FEEDBACKS {
            let extra = self.feedbacks.len() - MAX_FEEDBACKS;
            self.feedbacks.drain(..extra);
        }
    }

    /// Adds a line to the history.
    pub fn log(&mut self, text: impl Into<String>) {
        self.events.push(TaskEvent { at_ms: now_ms(), text: text.into() });
        if self.events.len() > MAX_EVENTS {
            let extra = self.events.len() - MAX_EVENTS;
            self.events.drain(..extra);
        }
    }

    /// Slim version (without brief, report or history) for lists.
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
            "merge_approved": self.merge_approved,
        })
    }
}

/// What is given to create a task ([`Core::task_create`]).
#[derive(Debug, Clone, Default)]
pub struct TaskSpec {
    pub title: String,
    /// The request (markdown). It may stay empty: the title goes in its place when delivering to the agent.
    pub brief: String,
    /// Agent; empty = choose the official CLI with the most quota available. Accepts the aliases (`agy`).
    pub kind: String,
    /// Requested model; `None` = the default of the role or of the CLI.
    pub model: Option<String>,
    /// Requested effort; `None` = the CLI's default. Only valid where [`models::effort_unavailable`] does not object.
    pub effort: Option<models::Effort>,
    /// The size of the work ([`Task::level`]); without it, `media`.
    pub level: Level,
    /// Branch to depart from and merge into; `None` = the current branch of the project's repository.
    pub base: Option<String>,
    /// Who asked (agent id or `user`).
    pub creator: Option<String>,
    /// The paths the task will touch ([`Task::scope`]); empty = no scope. Normalized in `task_create`.
    pub scope: Vec<String>,
}

/// Why an operation on tasks was not done. `http_status` is the bridge's code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskError {
    /// A project or task that does not exist (404).
    NotFound(String),
    /// Malformed request: empty title, invalid agent or model, project without git… (400).
    Invalid(String),
    /// Out of the right moment or with something to resolve: wrong state, dirty main repository, conflict (409).
    Conflict(String),
    /// Infrastructure error (git, disk) (500).
    Failed(String),
}

impl TaskError {
    /// The message, without the kind.
    pub fn message(&self) -> &str {
        match self {
            TaskError::NotFound(m) | TaskError::Invalid(m) | TaskError::Conflict(m) | TaskError::Failed(m) => m,
        }
    }

    /// The bridge's HTTP code for this error.
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

/// A merge refusal that has a way out the screen can offer on a button (the error message remains the text for the
/// command line and for the agents).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeBlock {
    /// The main repository has uncommitted changes of the user's (the files): "stash, merge and give back".
    MainDirty(Vec<String>),
    /// The task's folder has uncommitted work (the files): "ask the agent to commit".
    TaskDirty(Vec<String>),
    /// The merge conflicted and was undone (the files): "ask the agent to resolve the conflict".
    Conflict(Vec<String>),
}

impl MergeBlock {
    /// The files, complete (the error message cuts at five).
    pub fn files(&self) -> &[String] {
        match self {
            MergeBlock::MainDirty(f) | MergeBlock::TaskDirty(f) | MergeBlock::Conflict(f) => f,
        }
    }

    /// The explanation for someone who is not a developer, in the screen's notice.
    pub fn explain(&self, task: &Task) -> String {
        match self {
            MergeBlock::MainDirty(f) => t!(
                "You have changes of your own, not yet committed, in the project's main folder ({files}). Git does not merge over them. \
                 You can set them aside for a moment: Holt merges the task and puts each change back in place.",
                files = short_list(f)
            ),
            MergeBlock::TaskDirty(f) => t!(
                "The agent left uncommitted work in the task's folder ({files}). Only what was committed enters the merge. \
                 Ask it to commit and deliver again.",
                files = short_list(f)
            ),
            MergeBlock::Conflict(f) => t!(
                "The task and the branch \"{base}\" changed the same lines ({files}). Nothing was merged and the task is still in review. \
                 Ask the agent to combine the two versions and deliver again.",
                base = task.base,
                files = if f.is_empty() { t!("unidentified files").to_owned() } else { short_list(f) }
            ),
        }
    }

    /// The request ready to return the task to the agent (`task_return`); `None` for what the agent does not resolve.
    /// Agent-facing text: plain English.
    pub fn return_feedback(&self, task: &Task) -> Option<String> {
        let list = |f: &[String]| f.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n");
        match self {
            MergeBlock::MainDirty(_) => None,
            MergeBlock::TaskDirty(f) => Some(format!(
                "I could not merge your task: your folder ({wt}) has uncommitted work, and only what is committed enters the merge. Files:\n\n{files}\n\n\
                 Commit what is part of the task (git add and git commit) and discard what is not (git restore, or delete the file). \
                 Leave nothing pending: check with git status and deliver again.",
                wt = task.worktree.display(),
                files = list(f),
            )),
            MergeBlock::Conflict(f) => Some(format!(
                "Merging your branch ({branch}) into {base} conflicted and was undone. {fix}",
                branch = task.branch,
                base = task.base,
                fix = conflict_fix(task, f),
            )),
        }
    }
}

/// What the agent has to do about a conflict with the base: the files and the steps (`git merge <base>` in its folder,
/// resolve keeping what both sides brought, run the checks, commit, deliver again). Shared by the merge request, the
/// refused delivery ([`Core::task_done`]) and the automatic request. Agent-facing text: plain English.
pub(super) fn conflict_fix(task: &Task, files: &[String]) -> String {
    let list = if files.is_empty() {
        "- (git did not report them; check with git merge)".to_owned()
    } else {
        files.iter().map(|p| format!("- {p}")).collect::<Vec<_>>().join("\n")
    };
    format!(
        "Conflicting files:\n\n{list}\n\n\
         In your folder ({wt}), run git merge {base}, resolve the conflicts keeping what both versions brought that is useful, \
         run the tests and checks, commit the merge and deliver again (ask-agent --task done {id} --file <file>).",
        wt = task.worktree.display(),
        base = task.base,
        id = task.id,
    )
}

/// The refused merge: the usual error and, when there is a way out for the user, the structured reason.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRefusal {
    pub error: TaskError,
    pub block: Option<MergeBlock>,
    /// The stash notice ("stash my changes"), which also goes at the end of the error: the screen that swaps the
    /// error for the explanation of `block` shows it separately, otherwise the user does not know where their
    /// changes went.
    pub stash_note: Option<String>,
}

impl From<TaskError> for MergeRefusal {
    fn from(error: TaskError) -> Self {
        MergeRefusal { error, block: None, stash_note: None }
    }
}

/// The merge done; `note` is the notice about the changes stashed during it (given back, or still in the stash).
#[derive(Debug, Clone)]
pub struct MergeOutcome {
    pub task: Task,
    pub note: Option<String>,
    /// The merge did not happen: it conflicted with the base and the agent was asked to resolve it (the task is
    /// `Running`, with [`Task::merge_approved`]); the merge is retried when the agent delivers. `note` says so.
    pub resolving: bool,
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
pub use lifecycle::exclude_with_holt;
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
            title: "Fix the login".into(),
            brief: "Do X.\n\nThen Y.".into(),
            base: "main".into(),
            branch: "task/t3-fix-the-login".into(),
            worktree: PathBuf::from("/repo/.holt/worktrees/t3"),
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
            t.add_feedback("review", &n.to_string());
        }
        assert_eq!(t.feedbacks.len(), MAX_FEEDBACKS);
        assert_eq!(t.feedbacks.first().map(|f| f.text.as_str()), Some("5"));
        // Tasks saved before the field existed still open.
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
        // Stable: the old JSON (`merged`) still holds and the new state has its own name and label.
        assert_eq!(serde_json::from_str::<TaskStatus>("\"merged\"").ok(), Some(TaskStatus::Merged));
        assert_eq!(serde_json::to_string(&TaskStatus::Validating).ok().as_deref(), Some("\"validating\""));
        assert_eq!((TaskStatus::Validating.as_str(), TaskStatus::Validating.label()), ("validating", "awaiting validation"));
        assert_eq!(TaskStatus::Merged.label(), "completed");
    }

    #[test]
    fn the_labels_follow_the_language() {
        use crate::i18n::{with_lang, Lang};
        assert_eq!(with_lang(Lang::PtBr, || TaskStatus::Validating.label()), "aguardando validação");
        assert_eq!(with_lang(Lang::PtBr, || TaskStatus::Merged.label()), "concluída");
        assert_eq!(with_lang(Lang::PtBr, || TaskStatus::Failed.label()), "com erro");
    }

    #[test]
    fn the_timeline_keeps_only_the_messages_of_the_task_instance() {
        let mut t = Task { instance: "claude@t3".into(), ..sample() };
        // A question asked by the task, an answer received and an error that comes back: all are its own.
        assert!(t.record_message(&entry(FeedKind::Ask, Some("claude@t3"), Some("antigravity"), "and the css?")));
        assert!(t.record_message(&entry(FeedKind::Answer, Some("antigravity"), Some("claude@t3"), "use flex")));
        assert!(t.record_message(&entry(FeedKind::Error, Some("opencode"), Some("claude@t3"), "out of quota")));
        // Another instance, a transient indicator, a user message and a system notice stay out.
        assert!(!t.record_message(&entry(FeedKind::Ask, Some("claude"), Some("antigravity"), "another")));
        assert!(!t.record_message(&entry(FeedKind::Thinking, Some("claude@t3"), None, "⚙️ [read_file] x")));
        assert!(!t.record_message(&entry(FeedKind::User, Some("you"), Some("claude@t3"), "hi")));
        assert!(!t.record_message(&FeedEntry::system("Task t3 started")));
        // `claude@t30` is not `claude@t3`.
        assert!(!t.record_message(&entry(FeedKind::Ask, Some("claude@t30"), Some("claude"), "x")));
        let texts: Vec<&str> = t.messages.iter().map(|m| m.text.as_str()).collect();
        assert_eq!(texts, ["and the css?", "use flex", "out of quota"]);
        assert_eq!(t.messages[0].from.as_deref(), Some("claude@t3"));
        assert_eq!(t.messages[1].to.as_deref(), Some("claude@t3"));
        // A task without an instance (draft) does not adopt messages with no sender or receiver.
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
        assert_eq!(t.messages[0].text, "m5", "the oldest leave");
        t.record_message(&entry(FeedKind::Answer, Some("x"), Some("claude@t3"), &"a".repeat(MAX_MESSAGE_CHARS * 2)));
        let last = &t.messages[t.messages.len() - 1].text;
        assert!(last.starts_with("aaaa") && last.contains("text cut") && last.chars().count() < MAX_MESSAGE_CHARS + 100, "{}", last.len());
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

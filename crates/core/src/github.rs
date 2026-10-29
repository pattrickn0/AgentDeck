//! GitHub integration (PRs, issues and Actions) for the active project's repository.
//!
//! - Finds `owner/repo` from the `origin` remote (`git remote get-url origin`), accepting HTTPS
//!   URLs, `git@github.com:…` and `ssh://…`.
//! - Authenticates without storing a token in the app: it asks `git credential fill`
//!   (`protocol=https`, `host=github.com`) with `GCM_INTERACTIVE=never` and
//!   `GIT_TERMINAL_PROMPT=0` (or `gh auth token`, if the GitHub CLI exists and is signed in).
//!   The token lives only in memory and never goes to logs, configuration or error messages.
//!   Without a credential, public repositories keep working without a token (limit of 60
//!   requests/h).
//! - Queries the GitHub REST API through `ureq` on background threads and returns the result
//!   through the [`CoreEvent::Github`] queue, with an in-memory cache per repository and
//!   *rate limit* control.

use std::collections::HashMap;
use std::fmt;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::events::CoreEvent;
use crate::process::{run_captured_raw, Exit};
use crate::t;
use crate::util::{terminal_text, truncate_chars};
use crate::Core;

/// Timeout of local commands (`git remote`, `git credential fill`, `gh auth token`).
const CMD_TIMEOUT: Duration = Duration::from_secs(5);
/// Overall timeout of each HTTP request to the GitHub API.
const HTTP_TIMEOUT: Duration = Duration::from_secs(15);
/// HTTP connection timeout.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(6);
/// Validity of the in-memory cache when the tab is opened (the Refresh button forces a new read).
const CACHE_TTL: Duration = Duration::from_secs(60);
/// Validity of the `git` / `gh` executable looked up on the system.
const EXE_TTL: Duration = Duration::from_secs(30);
/// Maximum number of lines kept from the end of a failed job's log.
pub const LOG_MAX_LINES: usize = 200;
/// Maximum number of bytes kept from the end of a failed job's log (32 KiB).
pub const LOG_MAX_BYTES: usize = 32 * 1024;
/// Total read limit of the log stream (avoids downloading tens of megabytes indefinitely).
const LOG_STREAM_LIMIT: usize = 8 * 1024 * 1024;

// ---------- Public types ----------

/// `owner/repo` identification of a repository on GitHub.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RepoSlug {
    pub owner: String,
    pub repo: String,
}

impl RepoSlug {
    pub fn new(owner: impl Into<String>, repo: impl Into<String>) -> Self {
        Self { owner: owner.into(), repo: repo.into() }
    }

    /// `"owner/repo"`.
    pub fn full_name(&self) -> String {
        format!("{}/{}", self.owner, self.repo)
    }

    /// `"https://github.com/owner/repo"`.
    pub fn html_url(&self) -> String {
        format!("https://github.com/{}/{}", self.owner, self.repo)
    }
}

impl fmt::Display for RepoSlug {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.owner, self.repo)
    }
}

/// Summary state of the checks / Actions runs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum CheckStatus {
    #[default]
    Neutral,
    Pending,
    Success,
    Failure,
}

impl CheckStatus {
    pub fn label(self) -> &'static str {
        match self {
            CheckStatus::Success => t!("passed"),
            CheckStatus::Failure => t!("failed"),
            CheckStatus::Pending => t!("in progress"),
            CheckStatus::Neutral => t!("no checks"),
        }
    }
}

/// Open Pull Request in the repository.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub head_branch: String,
    pub base_branch: String,
    pub head_sha: String,
    pub draft: bool,
    pub html_url: String,
    pub updated_at: String,
    pub checks: CheckStatus,
}

/// Open issue in the repository (without PRs).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    pub author: String,
    pub labels: Vec<String>,
    pub comments: u64,
    pub html_url: String,
    pub updated_at: String,
    pub body_preview: String,
}

/// Run of a GitHub Actions workflow.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowRun {
    pub id: u64,
    pub name: String,
    pub display_title: String,
    pub head_branch: String,
    pub head_sha: String,
    pub event: String,
    pub status: CheckStatus,
    pub raw_status: String,
    pub conclusion: Option<String>,
    pub html_url: String,
    pub created_at: String,
}

/// Individual job inside a GitHub Actions run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkflowJob {
    pub id: u64,
    pub run_id: u64,
    pub name: String,
    pub status: CheckStatus,
    pub conclusion: Option<String>,
    pub html_url: String,
    /// Names of the steps that failed in this job.
    pub failed_steps: Vec<String>,
}

/// Final part of a failed job's log.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct JobLog {
    pub lines: Vec<String>,
    /// Cut for exceeding [`LOG_MAX_LINES`] or [`LOG_MAX_BYTES`].
    pub truncated: bool,
}

/// Request limit (*rate limit*) information of the GitHub API.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RateLimitInfo {
    pub remaining: Option<u32>,
    pub limit: Option<u32>,
    pub reset_at_epoch_s: Option<u64>,
}

/// Overview of the repository on GitHub: open PRs, open issues and the latest Actions runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GithubOverview {
    pub repo: RepoSlug,
    /// `true` if the requests used a credential (GCM or `gh`); `false` = anonymous (60 req/h).
    pub authenticated: bool,
    pub prs: Vec<PullRequest>,
    pub issues: Vec<Issue>,
    pub runs: Vec<WorkflowRun>,
    pub rate_limit: Option<RateLimitInfo>,
}

/// Checks and jobs detail of a specific PR.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PrChecksDetail {
    pub pr_number: u64,
    pub head_sha: String,
    pub runs: Vec<WorkflowRun>,
    pub jobs: Vec<WorkflowJob>,
    /// Pre-loaded log of the first failed job (`(job_id, result)`), if any.
    pub failed_log: Option<(u64, Result<JobLog, GithubError>)>,
}

impl PrChecksDetail {
    /// PR checks summary: the latest run of each workflow.
    pub fn status(&self) -> CheckStatus {
        aggregate_check_status(latest_per_workflow(&self.runs).into_iter().map(|r| r.status))
    }
}

/// Clear errors of the GitHub integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GithubError {
    NoProject,
    NoGit,
    NotARepo,
    NoRemote,
    NotGithubRemote {
        url: String,
    },
    Unauthorized,
    RateLimited {
        reset_in_secs: Option<u64>,
        authenticated: bool,
    },
    Forbidden(String),
    NotFound {
        repo: String,
        authenticated: bool,
    },
    /// HTTP 410: the feature was disabled in the repository (e.g. issues turned off).
    Gone,
    Network(String),
    Failed(String),
}

/// Strips user name and password/token from a remote URL (`https://user:token@host/...` becomes
/// `https://***@host/...`) before it goes into a message. The scp form (`git@host:owner/repo`)
/// carries no secret and stays as is.
fn redact_remote_url(url: &str) -> String {
    let Some(scheme_end) = url.find("://") else { return url.to_owned() };
    let rest = &url[scheme_end + 3..];
    let host_end = rest.find('/').unwrap_or(rest.len());
    match rest[..host_end].rfind('@') {
        Some(at) => format!("{}***@{}", &url[..scheme_end + 3], &rest[at + 1..]),
        None => url.to_owned(),
    }
}

impl fmt::Display for GithubError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            GithubError::NoProject => write!(f, "{}", t!("Open a folder as a project to see GitHub.")),
            GithubError::NoGit => write!(f, "{}", t!("Git was not found. Install it from git-scm.com and try again.")),
            GithubError::NotARepo => write!(f, "{}", t!("This folder is not a git repository.")),
            GithubError::NoRemote => {
                write!(f, "{}", t!("This repository has no \"origin\" remote configured."))
            }
            GithubError::NotGithubRemote { url } => {
                write!(f, "{}", t!("The \"origin\" remote ({url}) does not point to github.com.", url = url))
            }
            GithubError::Unauthorized => write!(
                f,
                "{}",
                t!("GitHub credential refused (HTTP 401). Push from the Git tab to refresh the Git Credential Manager login, or run \"gh auth login\".")
            ),
            GithubError::RateLimited { reset_in_secs, authenticated } => {
                let wait = match reset_in_secs {
                    Some(s) if *s >= 60 => t!(" (renews in ~{minutes} min)", minutes = (*s).div_ceil(60)),
                    Some(s) if *s > 0 => t!(" (renews in {seconds}s)", seconds = s),
                    _ => String::new(),
                };
                if *authenticated {
                    write!(f, "{}", t!("GitHub API rate limit reached{wait}.", wait = wait))
                } else {
                    write!(
                        f,
                        "{}",
                        t!(
                            "Anonymous rate limit reached (60/h){wait}. Push from the Git tab to sign in with the Git Credential Manager, or install gh (5,000/h).",
                            wait = wait
                        )
                    )
                }
            }
            GithubError::Forbidden(msg) => write!(f, "{}", t!("Access denied by GitHub (HTTP 403): {message}", message = msg)),
            GithubError::NotFound { repo, authenticated } => {
                if *authenticated {
                    write!(
                        f,
                        "{}",
                        t!(
                            "Not found on GitHub (HTTP 404): check that {repo} exists and that your account has permission.",
                            repo = repo
                        )
                    )
                } else {
                    write!(
                        f,
                        "{}",
                        t!(
                            "Repository {repo} not found or private (HTTP 404). To access a private repository, push from the Git tab (opens the Git Credential Manager login) or install gh.",
                            repo = repo
                        )
                    )
                }
            }
            GithubError::Gone => write!(f, "{}", t!("Feature disabled in this GitHub repository (HTTP 410).")),
            GithubError::Network(msg) => write!(f, "{}", t!("Connection to GitHub failed: {error}", error = msg)),
            GithubError::Failed(msg) => write!(f, "{msg}"),
        }
    }
}

impl std::error::Error for GithubError {}

/// Event emitted by the backend to update the GitHub tab of the interface.
#[derive(Debug, Clone)]
pub struct GithubEvent {
    /// Id of the project where the query started.
    pub project: String,
    pub payload: GithubPayload,
}

#[derive(Debug, Clone)]
pub enum GithubPayload {
    Overview(Result<GithubOverview, GithubError>),
    PrDetail { pr_number: u64, result: Result<PrChecksDetail, GithubError> },
    RunJobs { run_id: u64, result: Result<Vec<WorkflowJob>, GithubError>, failed_log: Option<(u64, Result<JobLog, GithubError>)> },
    JobLog { job_id: u64, result: Result<JobLog, GithubError> },
}

// ---------- Protected token in memory ----------

/// Authentication token kept only in memory (never printed in `Debug`).
#[derive(Clone)]
struct SecretToken(String);

impl fmt::Debug for SecretToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SecretToken(***)")
    }
}

// ---------- Pure parse and formatting functions ----------

fn valid_slug_part(s: &str) -> bool {
    !s.is_empty() && s.len() <= 100 && s != "." && s != ".." && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// Extracts `owner/repo` from an `origin` remote URL if it points to `github.com`.
///
/// Supports:
/// - `https://github.com/owner/repo(.git)` (with or without `user@`, trailing `/` or `www.`)
/// - `git@github.com:owner/repo(.git)`
/// - `ssh://git@github.com(:22)/owner/repo(.git)`
/// - `git://github.com/owner/repo(.git)`
pub fn parse_github_remote(raw: &str) -> Option<RepoSlug> {
    let s = raw.trim();
    if s.is_empty() {
        return None;
    }

    let path = if let Some(rest) = s
        .strip_prefix("https://")
        .or_else(|| s.strip_prefix("http://"))
        .or_else(|| s.strip_prefix("ssh://"))
        .or_else(|| s.strip_prefix("git://"))
        .or_else(|| s.strip_prefix("git+ssh://"))
        .or_else(|| s.strip_prefix("git+https://"))
    {
        let (authority, path) = rest.split_once('/')?;
        let host_port = authority.rsplit_once('@').map_or(authority, |(_, h)| h);
        let host = host_port.split_once(':').map_or(host_port, |(h, _)| h);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
            return None;
        }
        path
    } else if let Some((left, right)) = s.split_once(':') {
        // SCP format: `[user@]github.com:owner/repo(.git)`
        if left.contains('/') || left.contains('\\') {
            return None;
        }
        let host = left.rsplit_once('@').map_or(left, |(_, h)| h);
        if !host.eq_ignore_ascii_case("github.com") && !host.eq_ignore_ascii_case("www.github.com") {
            return None;
        }
        right.trim_start_matches('/')
    } else {
        return None;
    };

    let clean = path.trim_matches('/');
    let mut parts = clean.split('/');
    let owner = parts.next()?;
    let repo_raw = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let repo = repo_raw.strip_suffix(".git").unwrap_or(repo_raw);
    if !valid_slug_part(owner) || !valid_slug_part(repo) {
        return None;
    }
    Some(RepoSlug { owner: owner.to_owned(), repo: repo.to_owned() })
}

/// Extracts the token from the output of `git credential fill` (`password=...`), never exposing it in logs.
pub fn parse_credential_fill(output: &str) -> Option<String> {
    for line in output.lines() {
        let line = line.trim_end_matches('\r');
        if let Some(pw) = line.strip_prefix("password=") {
            let trimmed = pw.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_owned());
            }
        }
    }
    None
}

/// Converts the GitHub API `status` + `conclusion` into a [`CheckStatus`].
pub fn map_check_status(status: &str, conclusion: Option<&str>) -> CheckStatus {
    match status {
        "queued" | "in_progress" | "waiting" | "requested" | "pending" => CheckStatus::Pending,
        "completed" => match conclusion.unwrap_or("") {
            "success" => CheckStatus::Success,
            "failure" | "timed_out" | "startup_failure" | "action_required" | "cancelled" => CheckStatus::Failure,
            _ => CheckStatus::Neutral,
        },
        _ => CheckStatus::Neutral,
    }
}

/// Combines several check states into a single summary (failure > pending > success > neutral).
pub fn aggregate_check_status(statuses: impl IntoIterator<Item = CheckStatus>) -> CheckStatus {
    let mut any_pending = false;
    let mut any_success = false;
    for st in statuses {
        match st {
            CheckStatus::Failure => return CheckStatus::Failure,
            CheckStatus::Pending => any_pending = true,
            CheckStatus::Success => any_success = true,
            CheckStatus::Neutral => {}
        }
    }
    if any_pending {
        CheckStatus::Pending
    } else if any_success {
        CheckStatus::Success
    } else {
        CheckStatus::Neutral
    }
}

/// Parses the Pull Request list (`GET /repos/{owner}/{repo}/pulls`).
pub fn parse_pulls(json: &Value) -> Vec<PullRequest> {
    let Some(arr) = json.as_array() else { return Vec::new() };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(number) = item.get("number").and_then(Value::as_u64) else { continue };
        let title = item.get("title").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let author = item.get("user").and_then(|u| u.get("login")).and_then(Value::as_str).unwrap_or(t!("unknown")).to_owned();
        let head = item.get("head");
        let head_branch = head.and_then(|h| h.get("ref")).and_then(Value::as_str).unwrap_or("").to_owned();
        let head_sha = head.and_then(|h| h.get("sha")).and_then(Value::as_str).unwrap_or("").to_owned();
        let base_branch = item.get("base").and_then(|b| b.get("ref")).and_then(Value::as_str).unwrap_or("").to_owned();
        let draft = item.get("draft").and_then(Value::as_bool).unwrap_or(false);
        let html_url = item.get("html_url").and_then(Value::as_str).unwrap_or("").to_owned();
        let updated_at = item.get("updated_at").and_then(Value::as_str).unwrap_or("").to_owned();
        out.push(PullRequest {
            number,
            title,
            author,
            head_branch,
            base_branch,
            head_sha,
            draft,
            html_url,
            updated_at,
            checks: CheckStatus::Neutral,
        });
    }
    out
}

/// Parses the open Issue list (`GET /repos/{owner}/{repo}/issues`), discarding PRs.
pub fn parse_issues(json: &Value) -> Vec<Issue> {
    let Some(arr) = json.as_array() else { return Vec::new() };
    let mut out = Vec::new();
    for item in arr {
        // In the GitHub REST API every PR also comes in `/issues` with the `"pull_request"` key.
        if item.get("pull_request").is_some() {
            continue;
        }
        let Some(number) = item.get("number").and_then(Value::as_u64) else { continue };
        let title = item.get("title").and_then(Value::as_str).unwrap_or("").trim().to_owned();
        let author = item.get("user").and_then(|u| u.get("login")).and_then(Value::as_str).unwrap_or(t!("unknown")).to_owned();
        let labels = item
            .get("labels")
            .and_then(Value::as_array)
            .map(|list| list.iter().filter_map(|l| l.get("name").and_then(Value::as_str).map(str::to_owned)).collect())
            .unwrap_or_default();
        let comments = item.get("comments").and_then(Value::as_u64).unwrap_or(0);
        let html_url = item.get("html_url").and_then(Value::as_str).unwrap_or("").to_owned();
        let updated_at = item.get("updated_at").and_then(Value::as_str).unwrap_or("").to_owned();
        let raw_body = item.get("body").and_then(Value::as_str).unwrap_or("").trim();
        let clean_body = raw_body.lines().map(str::trim).filter(|l| !l.is_empty()).collect::<Vec<_>>().join(" ");
        let (preview, clipped) = truncate_chars(&clean_body, 600);
        let body_preview = if clipped { format!("{preview}…") } else { preview.to_owned() };
        out.push(Issue { number, title, author, labels, comments, html_url, updated_at, body_preview });
    }
    out
}

/// Parses the GitHub Actions runs (`GET /repos/{owner}/{repo}/actions/runs`).
pub fn parse_workflow_runs(json: &Value) -> Vec<WorkflowRun> {
    let arr = json.get("workflow_runs").and_then(Value::as_array).or_else(|| json.as_array());
    let Some(arr) = arr else { return Vec::new() };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_u64) else { continue };
        let name = item.get("name").and_then(Value::as_str).unwrap_or("Workflow").trim().to_owned();
        let display_title = item.get("display_title").and_then(Value::as_str).unwrap_or(&name).trim().to_owned();
        let head_branch = item.get("head_branch").and_then(Value::as_str).unwrap_or("").to_owned();
        let head_sha = item.get("head_sha").and_then(Value::as_str).unwrap_or("").to_owned();
        let event = item.get("event").and_then(Value::as_str).unwrap_or("").to_owned();
        let raw_status = item.get("status").and_then(Value::as_str).unwrap_or("").to_owned();
        let conclusion = item.get("conclusion").and_then(Value::as_str).map(str::to_owned);
        let status = map_check_status(&raw_status, conclusion.as_deref());
        let html_url = item.get("html_url").and_then(Value::as_str).unwrap_or("").to_owned();
        let created_at = item.get("created_at").and_then(Value::as_str).unwrap_or("").to_owned();
        out.push(WorkflowRun {
            id,
            name,
            display_title,
            head_branch,
            head_sha,
            event,
            status,
            raw_status,
            conclusion,
            html_url,
            created_at,
        });
    }
    out
}

/// Parses the jobs of a run (`GET /repos/{owner}/{repo}/actions/runs/{run_id}/jobs`).
pub fn parse_workflow_jobs(json: &Value, default_run_id: u64) -> Vec<WorkflowJob> {
    let arr = json.get("jobs").and_then(Value::as_array).or_else(|| json.as_array());
    let Some(arr) = arr else { return Vec::new() };
    let mut out = Vec::with_capacity(arr.len());
    for item in arr {
        let Some(id) = item.get("id").and_then(Value::as_u64) else { continue };
        let run_id = item.get("run_id").and_then(Value::as_u64).unwrap_or(default_run_id);
        let name = item.get("name").and_then(Value::as_str).unwrap_or("job").trim().to_owned();
        let raw_status = item.get("status").and_then(Value::as_str).unwrap_or("");
        let conclusion = item.get("conclusion").and_then(Value::as_str).map(str::to_owned);
        let status = map_check_status(raw_status, conclusion.as_deref());
        let html_url = item.get("html_url").and_then(Value::as_str).unwrap_or("").to_owned();
        let failed_steps = item
            .get("steps")
            .and_then(Value::as_array)
            .map(|steps| {
                steps
                    .iter()
                    .filter(|s| {
                        s.get("conclusion").and_then(Value::as_str).is_some_and(|c| matches!(c, "failure" | "timed_out" | "cancelled"))
                    })
                    .filter_map(|s| s.get("name").and_then(Value::as_str).map(str::to_owned))
                    .collect()
            })
            .unwrap_or_default();
        out.push(WorkflowJob { id, run_id, name, status, conclusion, html_url, failed_steps });
    }
    out
}

/// Parses `/repos/{owner}/{repo}/commits/{ref}/check-runs` to get the combined status.
pub fn parse_check_runs_status(json: &Value) -> CheckStatus {
    let Some(arr) = json.get("check_runs").and_then(Value::as_array) else {
        return CheckStatus::Neutral;
    };
    aggregate_check_status(arr.iter().map(|item| {
        let st = item.get("status").and_then(Value::as_str).unwrap_or("");
        let conc = item.get("conclusion").and_then(Value::as_str);
        map_check_status(st, conc)
    }))
}

/// Keeps only the latest run of each workflow (the API returns newest to oldest): a rerun that
/// passed must not stay red because of the previous attempt.
pub fn latest_per_workflow(runs: &[WorkflowRun]) -> Vec<&WorkflowRun> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for r in runs {
        if !seen.contains(&r.name.as_str()) {
            seen.push(&r.name);
            out.push(r);
        }
    }
    out
}

/// Fills `pr.checks` from the Actions runs (by `head_sha` or, without it, by the branch).
pub fn attach_pr_checks(prs: &mut [PullRequest], runs: &[WorkflowRun]) {
    for pr in prs {
        let matching: Vec<WorkflowRun> = runs
            .iter()
            .filter(|r| {
                (!pr.head_sha.is_empty() && r.head_sha == pr.head_sha) || (r.head_sha.is_empty() && r.head_branch == pr.head_branch)
            })
            .cloned()
            .collect();
        if !matching.is_empty() {
            pr.checks = aggregate_check_status(latest_per_workflow(&matching).into_iter().map(|r| r.status));
        }
    }
}

/// Removes ANSI CSI sequences (`\x1b[...m`) and control characters from a log line.
fn clean_log_line(raw: &str) -> String {
    let s = raw.trim_end_matches('\r').trim_start_matches('\u{feff}');
    // Strips the GitHub Actions ISO timestamp prefix (`2026-10-16T03:12:34.1234567Z `) when present.
    let s = if s.len() > 29 && s.as_bytes().get(4) == Some(&b'-') && s.as_bytes().get(10) == Some(&b'T') {
        match s.split_once(' ') {
            Some((ts, rest)) if ts.ends_with('Z') && ts.len() >= 20 && ts.len() <= 35 => rest,
            _ => s,
        }
    } else {
        s
    };

    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                chars.next();
                for ch in chars.by_ref() {
                    if ('@'..='~').contains(&ch) {
                        break;
                    }
                }
            }
            continue;
        }
        if c == '\t' || !c.is_control() {
            out.push(c);
        }
    }
    let (short, clipped) = truncate_chars(&out, 600);
    if clipped {
        format!("{short}…")
    } else {
        short.to_owned()
    }
}

/// Extracts only the end of a job log, respecting the byte limit (`max_bytes`) and line limit (`max_lines`).
pub fn tail_log(raw: &str, max_lines: usize, max_bytes: usize) -> JobLog {
    if raw.is_empty() || max_lines == 0 || max_bytes == 0 {
        return JobLog::default();
    }
    let mut byte_truncated = false;
    let slice = if raw.len() > max_bytes {
        byte_truncated = true;
        let mut idx = raw.len() - max_bytes;
        while idx < raw.len() && !raw.is_char_boundary(idx) {
            idx += 1;
        }
        // Starts at the next line break so a line is never cut in half.
        match raw[idx..].find('\n') {
            Some(nl) if idx + nl + 1 < raw.len() => &raw[idx + nl + 1..],
            _ => &raw[idx..],
        }
    } else {
        raw
    };

    let all_lines: Vec<String> = slice.lines().map(clean_log_line).collect();
    let line_truncated = all_lines.len() > max_lines;
    let start = all_lines.len().saturating_sub(max_lines);
    JobLog { lines: all_lines[start..].to_vec(), truncated: byte_truncated || line_truncated }
}

/// Builds the ready-made message asking an agent to review a PR.
pub fn format_pr_prompt(repo: &RepoSlug, pr: &PullRequest) -> String {
    let sha: String = pr.head_sha.chars().take(7).collect();
    let draft = if pr.draft { " (draft)" } else { "" };
    // Agent-facing text is always English, whatever language the interface is in.
    let checks = crate::i18n::english(|| pr.checks.label());
    terminal_text(&format!(
        "Review PR #{}{} (\"{}\") in the {} repository.\nBranch: {} -> {} (commit {})\nChecks: {}\nLink: {}",
        pr.number,
        draft,
        pr.title,
        repo.full_name(),
        pr.head_branch,
        pr.base_branch,
        if sha.is_empty() { "HEAD" } else { &sha },
        checks,
        pr.html_url
    ))
}

/// Builds the ready-made message asking an agent to fix an issue.
pub fn format_issue_prompt(repo: &RepoSlug, issue: &Issue) -> String {
    let mut msg = format!(
        "Analyze and fix issue #{} (\"{}\") in the {} repository.\nLink: {}",
        issue.number,
        issue.title,
        repo.full_name(),
        issue.html_url
    );
    if !issue.labels.is_empty() {
        msg.push_str(&format!("\nLabels: {}", issue.labels.join(", ")));
    }
    if !issue.body_preview.is_empty() {
        msg.push_str(&format!("\nSummary: {}", issue.body_preview));
    }
    terminal_text(&msg)
}

/// Builds the ready-made message asking an agent to fix a job that failed.
pub fn format_failed_job_prompt(
    repo: &RepoSlug,
    pr: Option<&PullRequest>,
    run: Option<&WorkflowRun>,
    job: &WorkflowJob,
    log: Option<&JobLog>,
) -> String {
    let context = match (pr, run) {
        (Some(p), _) => format!("in PR #{} (\"{}\", branch {})", p.number, p.title, p.head_branch),
        (None, Some(r)) => format!("in the \"{}\" run (branch {})", r.display_title, r.head_branch),
        (None, None) => String::from("in GitHub Actions"),
    };
    let mut msg =
        format!("Fix the job \"{}\" that failed {} in the {} repository.\nJob link: {}", job.name, context, repo.full_name(), job.html_url);
    if !job.failed_steps.is_empty() {
        msg.push_str(&format!("\nFailed steps: {}", job.failed_steps.join(", ")));
    }
    if let Some(log) = log.filter(|l| !l.lines.is_empty()) {
        // Sends at most the last 60 lines / 4000 characters so it fits comfortably in the terminal paste.
        let tail_start = log.lines.len().saturating_sub(60);
        let joined = log.lines[tail_start..].join("\n");
        let (snippet, _) = truncate_chars(&joined, 4000);
        msg.push_str("\n\nEnd of log:\n");
        msg.push_str(snippet);
    }
    terminal_text(&msg)
}

// ---------- State in the Core and execution ----------

type PrCacheKey = (RepoSlug, u64, String);
type PrCacheMap = HashMap<PrCacheKey, (Instant, PrChecksDetail)>;
type RunJobsCacheMap = HashMap<(RepoSlug, u64), (Instant, Vec<WorkflowJob>)>;

#[derive(Default)]
pub(crate) struct GithubState {
    git_exe: Mutex<Option<(Instant, Option<PathBuf>)>>,
    gh_exe: Mutex<Option<(Instant, Option<PathBuf>)>>,
    /// Token in memory (`Some(None)` = looked it up recently and there was none).
    token: Mutex<Option<(Instant, Option<SecretToken>)>>,
    /// API limit per quota: `[anonymous (60/h), signed in (5,000/h)]`. These are separate quotas on
    /// GitHub: exhausting one must not block the other (e.g. signing in after spending the 60 anonymous).
    rate_limit: Mutex<[Option<RateLimitInfo>; 2]>,
    overview_cache: Mutex<HashMap<RepoSlug, (Instant, GithubOverview)>>,
    pr_cache: Mutex<PrCacheMap>,
    run_jobs_cache: Mutex<RunJobsCacheMap>,
    job_log_cache: Mutex<HashMap<(RepoSlug, u64), (Instant, JobLog)>>,
}

fn now_epoch_s() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn non_interactive_git(git: &Path, cwd: &Path, args: &[&str]) -> Command {
    let mut c = Command::new(git);
    c.current_dir(cwd).args(["-c", "core.quotepath=false"]).args(args);
    for (k, v) in [
        ("LC_ALL", "C"),
        ("LANGUAGE", "C"),
        ("GIT_TERMINAL_PROMPT", "0"),
        ("GCM_INTERACTIVE", "never"),
        ("GIT_OPTIONAL_LOCKS", "0"),
        ("GIT_PAGER", "cat"),
    ] {
        c.env(k, v);
    }
    for k in ["GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE", "GIT_PREFIX"] {
        c.env_remove(k);
    }
    c
}

fn http_agent(follow_redirects: bool) -> ureq::Agent {
    let mut cfg = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("agentdeck")
        .https_only(true)
        .timeout_global(Some(HTTP_TIMEOUT))
        .timeout_connect(Some(CONNECT_TIMEOUT));
    if !follow_redirects {
        cfg = cfg.max_redirects(0);
    }
    cfg.build().into()
}

impl Core {
    fn github_git_exe(&self) -> Option<PathBuf> {
        let mut cache = self.inner.github.git_exe.lock();
        if let Some((at, exe)) = cache.as_ref() {
            if at.elapsed() < EXE_TTL {
                return exe.clone();
            }
        }
        let mut exe = self.inner.agents.which(if cfg!(windows) { "git.exe" } else { "git" });
        if exe.is_none() && cfg!(windows) {
            exe = self.inner.agents.system_dir("ProgramFiles").map(|p| p.join("Git").join("cmd").join("git.exe")).filter(|p| p.is_file());
        }
        *cache = Some((Instant::now(), exe.clone()));
        exe
    }

    fn github_gh_exe(&self) -> Option<PathBuf> {
        let mut cache = self.inner.github.gh_exe.lock();
        if let Some((at, exe)) = cache.as_ref() {
            if at.elapsed() < EXE_TTL {
                return exe.clone();
            }
        }
        let mut exe = self.inner.agents.which(if cfg!(windows) { "gh.exe" } else { "gh" });
        if exe.is_none() && cfg!(windows) {
            exe = self.inner.agents.system_dir("ProgramFiles").map(|p| p.join("GitHub CLI").join("gh.exe")).filter(|p| p.is_file());
        }
        *cache = Some((Instant::now(), exe.clone()));
        exe
    }

    /// Finds the `owner/repo` repository of the `origin` remote in folder `ws`.
    pub fn github_repo_at(&self, ws: &Path) -> Result<RepoSlug, GithubError> {
        if ws.as_os_str().is_empty() {
            return Err(GithubError::NoProject);
        }
        let git = self.github_git_exe().ok_or(GithubError::NoGit)?;
        let out =
            run_captured_raw(non_interactive_git(&git, ws, &["remote", "get-url", "origin"]), None, CMD_TIMEOUT, None, None, 64 * 1024)
                .map_err(|e| {
                    if e.kind() == std::io::ErrorKind::NotFound {
                        GithubError::NoGit
                    } else {
                        GithubError::Failed(t!("failed to run git: {error}", error = e))
                    }
                })?;

        match out.exit {
            Exit::Code(Some(0)) => {
                let url = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                parse_github_remote(&url).ok_or_else(|| GithubError::NotGithubRemote { url: redact_remote_url(&url) })
            }
            Exit::TimedOut => Err(GithubError::Failed(t!("git took too long to read the origin remote.").to_owned())),
            _ => {
                let err = String::from_utf8_lossy(&out.stderr).to_lowercase();
                if err.contains("not a git repository") {
                    Err(GithubError::NotARepo)
                } else {
                    Err(GithubError::NoRemote)
                }
            }
        }
    }

    /// Gets the GitHub token in memory: first through `git credential fill` (GCM), then through
    /// `gh auth token`. Never writes it to disk nor logs the value.
    fn github_token(&self, ws: &Path, force_refresh: bool) -> Option<SecretToken> {
        if !force_refresh {
            if let Some((at, tok)) = self.inner.github.token.lock().as_ref() {
                if at.elapsed() < Duration::from_secs(120) {
                    return tok.clone();
                }
            }
        }
        let cwd = if ws.is_dir() { ws } else { Path::new(".") };
        let mut found: Option<SecretToken> = None;

        // 1. Main path: `git credential fill` (uses the Git Credential Manager without opening a window).
        if let Some(git) = self.github_git_exe() {
            let cmd = non_interactive_git(&git, cwd, &["credential", "fill"]);
            let input = b"protocol=https\nhost=github.com\n\n".to_vec();
            if let Ok(out) = run_captured_raw(cmd, Some(input), CMD_TIMEOUT, None, None, 16 * 1024) {
                if out.exit == Exit::Code(Some(0)) {
                    if let Some(tok) = parse_credential_fill(&String::from_utf8_lossy(&out.stdout)) {
                        found = Some(SecretToken(tok));
                    }
                }
            }
        }

        // 2. Alternative: `gh auth token` if the GitHub CLI exists and is authenticated.
        if found.is_none() {
            if let Some(gh) = self.github_gh_exe() {
                let mut cmd = Command::new(gh);
                cmd.current_dir(cwd)
                    .args(["auth", "token", "--hostname", "github.com"])
                    .env("GH_PROMPT_DISABLED", "1")
                    .env("GIT_TERMINAL_PROMPT", "0");
                if let Ok(out) = run_captured_raw(cmd, None, CMD_TIMEOUT, None, None, 16 * 1024) {
                    if out.exit == Exit::Code(Some(0)) {
                        let tok = String::from_utf8_lossy(&out.stdout).trim().to_owned();
                        if !tok.is_empty() {
                            found = Some(SecretToken(tok));
                        }
                    }
                }
            }
        }

        *self.inner.github.token.lock() = Some((Instant::now(), found.clone()));
        found
    }

    fn clear_github_token(&self) {
        *self.inner.github.token.lock() = Some((Instant::now(), None));
    }

    fn check_rate_limit(&self, authenticated: bool) -> Result<(), GithubError> {
        let Some(rl) = self.inner.github.rate_limit.lock()[authenticated as usize] else { return Ok(()) };
        if rl.remaining == Some(0) {
            let now = now_epoch_s();
            if let Some(reset) = rl.reset_at_epoch_s {
                if reset > now {
                    return Err(GithubError::RateLimited { reset_in_secs: Some(reset - now), authenticated });
                }
            }
        }
        Ok(())
    }

    fn record_rate_limit(&self, headers: &ureq::http::HeaderMap, authenticated: bool) -> RateLimitInfo {
        let get_u64 = |name: &str| headers.get(name).and_then(|v| v.to_str().ok()).and_then(|s| s.trim().parse::<u64>().ok());
        let info = RateLimitInfo {
            remaining: get_u64("x-ratelimit-remaining").map(|v| v as u32),
            limit: get_u64("x-ratelimit-limit").map(|v| v as u32),
            reset_at_epoch_s: get_u64("x-ratelimit-reset"),
        };
        if info.remaining.is_some() || info.limit.is_some() {
            self.inner.github.rate_limit.lock()[authenticated as usize] = Some(info);
        }
        info
    }

    /// Runs a JSON `GET` on the GitHub REST API (`https://api.github.com/...`).
    fn github_get_json(&self, repo: &RepoSlug, path: &str, token: Option<&SecretToken>) -> Result<Value, GithubError> {
        let authenticated = token.is_some();
        self.check_rate_limit(authenticated)?;
        let url = format!("https://api.github.com{path}");
        let agent = http_agent(true);
        let mut req = agent.get(&url).header("Accept", "application/vnd.github+json").header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(tok) = token {
            req = req.header("Authorization", format!("Bearer {}", tok.0));
        }
        let mut resp = req.call().map_err(|e| GithubError::Network(e.to_string()))?;
        let status = resp.status().as_u16();
        let rl = self.record_rate_limit(resp.headers(), authenticated);

        match status {
            200 => resp
                .body_mut()
                .read_json::<Value>()
                .map_err(|e| GithubError::Failed(t!("invalid JSON response from GitHub: {error}", error = e))),
            401 => {
                self.clear_github_token();
                Err(GithubError::Unauthorized)
            }
            403 | 429 => {
                let now = now_epoch_s();
                let reset_in = rl.reset_at_epoch_s.map(|r| r.saturating_sub(now));
                let body_msg = resp
                    .body_mut()
                    .read_json::<Value>()
                    .ok()
                    .and_then(|v| v.get("message").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                if status == 429 || rl.remaining == Some(0) || body_msg.to_lowercase().contains("rate limit") {
                    Err(GithubError::RateLimited { reset_in_secs: reset_in, authenticated })
                } else {
                    Err(GithubError::Forbidden(if body_msg.is_empty() { t!("insufficient permission").to_owned() } else { body_msg }))
                }
            }
            404 => Err(GithubError::NotFound { repo: repo.full_name(), authenticated }),
            410 => Err(GithubError::Gone),
            other => Err(GithubError::Failed(t!("GitHub answered with HTTP {status}", status = other))),
        }
    }

    /// Downloads only the end of a job log (`GET /repos/{owner}/{repo}/actions/jobs/{job_id}/logs`).
    fn github_fetch_job_log(&self, repo: &RepoSlug, job_id: u64, token: Option<&SecretToken>) -> Result<JobLog, GithubError> {
        let Some(tok) = token else {
            return Err(GithubError::Failed(t!(
                "Downloading the full log requires signing in to GitHub. Push from the Git tab (opens the Git Credential Manager) or install gh."
            ).to_owned()));
        };
        self.check_rate_limit(true)?;
        let url = format!("https://api.github.com/repos/{}/{}/actions/jobs/{job_id}/logs", repo.owner, repo.repo);
        // Without following the redirect automatically: GitHub answers 302 to a pre-signed Azure Blob
        // Storage URL that does not accept the GitHub `Authorization` header.
        let agent = http_agent(false);
        let resp = agent
            .get(&url)
            .header("Accept", "application/vnd.github+json")
            .header("X-GitHub-Api-Version", "2022-11-28")
            .header("Authorization", format!("Bearer {}", tok.0))
            .call()
            .map_err(|e| GithubError::Network(e.to_string()))?;

        let status = resp.status().as_u16();
        let rl = self.record_rate_limit(resp.headers(), true);

        let final_resp = if matches!(status, 301 | 302 | 303 | 307 | 308) {
            let loc = resp
                .headers()
                .get("location")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned)
                .ok_or_else(|| GithubError::Failed(t!("log redirect without a Location header").to_owned()))?;
            http_agent(true).get(&loc).call().map_err(|e| GithubError::Network(e.to_string()))?
        } else {
            resp
        };

        let final_status = final_resp.status().as_u16();
        match final_status {
            200 => {
                let raw = read_stream_tail(final_resp.into_body().into_reader(), LOG_MAX_BYTES * 2, LOG_STREAM_LIMIT)
                    .map_err(|e| GithubError::Network(t!("error reading the log: {error}", error = e)))?;
                Ok(tail_log(&String::from_utf8_lossy(&raw), LOG_MAX_LINES, LOG_MAX_BYTES))
            }
            401 => {
                self.clear_github_token();
                Err(GithubError::Unauthorized)
            }
            403 | 429 => {
                let reset_in = rl.reset_at_epoch_s.map(|r| r.saturating_sub(now_epoch_s()));
                if final_status == 429 || rl.remaining == Some(0) {
                    Err(GithubError::RateLimited { reset_in_secs: reset_in, authenticated: true })
                } else {
                    Err(GithubError::Forbidden(t!("no permission to read the Actions logs").to_owned()))
                }
            }
            404 | 410 => Err(GithubError::Failed(t!("This job's log is no longer available on GitHub.").to_owned())),
            other => Err(GithubError::Failed(t!("failed to download the log (HTTP {status})", status = other))),
        }
    }

    /// Synchronous query of the overview (PRs, issues, Actions runs) of the project in `ws`.
    fn fetch_overview_blocking(&self, ws: &Path, force: bool) -> Result<GithubOverview, GithubError> {
        let repo = self.github_repo_at(ws)?;
        if !force {
            if let Some((at, cached)) = self.inner.github.overview_cache.lock().get(&repo) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(cached.clone());
                }
            }
        }
        let mut token = self.github_token(ws, force);
        let base = format!("/repos/{}/{}", repo.owner, repo.repo);

        let pulls_url = format!("{base}/pulls?state=open&per_page=30");
        let pulls_json = match self.github_get_json(&repo, &pulls_url, token.as_ref()) {
            // Stored credential expired: a public repository still answers without it.
            Err(GithubError::Unauthorized) if token.is_some() => {
                token = None;
                self.github_get_json(&repo, &pulls_url, None).map_err(|e| match e {
                    GithubError::NotFound { .. } => GithubError::Unauthorized,
                    other => other,
                })?
            }
            other => other?,
        };
        // Issues disabled in the repository answer 410: the tab shows the empty list instead of failing.
        let issues_json = match self.github_get_json(&repo, &format!("{base}/issues?state=open&per_page=50"), token.as_ref()) {
            Err(GithubError::Gone) => Value::Array(Vec::new()),
            other => other?,
        };
        let runs_json = self
            .github_get_json(&repo, &format!("{base}/actions/runs?per_page=30"), token.as_ref())
            .unwrap_or_else(|_| serde_json::json!({ "workflow_runs": [] }));

        let mut prs = parse_pulls(&pulls_json);
        let issues = parse_issues(&issues_json);
        let runs = parse_workflow_runs(&runs_json);
        attach_pr_checks(&mut prs, &runs);

        // For the first open PRs whose commit was not in the last 30 runs, queries the check-runs
        // of the commit when there is room left in the rate limit.
        let bucket = token.is_some() as usize;
        let remaining = self.inner.github.rate_limit.lock()[bucket].and_then(|r| r.remaining).unwrap_or(60);
        if remaining > 10 {
            for pr in prs.iter_mut().filter(|p| p.checks == CheckStatus::Neutral && !p.head_sha.is_empty()).take(5) {
                if let Ok(checks_json) =
                    self.github_get_json(&repo, &format!("{base}/commits/{}/check-runs?per_page=50", pr.head_sha), token.as_ref())
                {
                    pr.checks = parse_check_runs_status(&checks_json);
                }
            }
        }

        let overview = GithubOverview {
            repo: repo.clone(),
            authenticated: token.is_some(),
            prs,
            issues,
            runs,
            rate_limit: self.inner.github.rate_limit.lock()[bucket],
        };
        self.inner.github.overview_cache.lock().insert(repo, (Instant::now(), overview.clone()));
        Ok(overview)
    }

    /// Loads (in the background) the GitHub overview of the active project and emits [`CoreEvent::Github`].
    pub fn github_refresh(&self, force: bool) {
        let project = self.active_project_id();
        let ws = self.workspace();
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new().name("cd-github".into()).spawn(move || {
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            let result = core.fetch_overview_blocking(&ws, force);
            core.emit(CoreEvent::Github(GithubEvent { project, payload: GithubPayload::Overview(result) }));
        });
    }

    /// Loads (in the background) the runs, jobs and the log of the first red job of a PR.
    pub fn github_load_pr(&self, pr_number: u64, head_sha: String, force: bool) {
        let project = self.active_project_id();
        let ws = self.workspace();
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new().name("cd-github-pr".into()).spawn(move || {
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            let result = core.fetch_pr_detail_blocking(&ws, pr_number, &head_sha, force);
            core.emit(CoreEvent::Github(GithubEvent { project, payload: GithubPayload::PrDetail { pr_number, result } }));
        });
    }

    fn fetch_pr_detail_blocking(&self, ws: &Path, pr_number: u64, head_sha: &str, force: bool) -> Result<PrChecksDetail, GithubError> {
        let repo = self.github_repo_at(ws)?;
        let key = (repo.clone(), pr_number, head_sha.to_owned());
        if !force {
            if let Some((at, cached)) = self.inner.github.pr_cache.lock().get(&key) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(cached.clone());
                }
            }
        }
        let token = self.github_token(ws, false);
        let base = format!("/repos/{}/{}", repo.owner, repo.repo);
        let runs_json = self.github_get_json(&repo, &format!("{base}/actions/runs?head_sha={head_sha}&per_page=15"), token.as_ref())?;
        let runs = parse_workflow_runs(&runs_json);

        let mut jobs = Vec::new();
        // Fetches the jobs of the latest run of each workflow of this commit (up to 4).
        let mut seen_workflows = 0;
        for run in latest_per_workflow(&runs) {
            seen_workflows += 1;
            if let Ok(jobs_json) = self.github_get_json(&repo, &format!("{base}/actions/runs/{}/jobs?per_page=50", run.id), token.as_ref())
            {
                let parsed = parse_workflow_jobs(&jobs_json, run.id);
                self.inner.github.run_jobs_cache.lock().insert((repo.clone(), run.id), (Instant::now(), parsed.clone()));
                jobs.extend(parsed);
            }
            if seen_workflows >= 4 {
                break;
            }
        }

        let failed_log = jobs.iter().find(|j| j.status == CheckStatus::Failure).map(|failed| {
            let log_res = self.fetch_job_log_cached(&repo, failed.id, token.as_ref(), force);
            (failed.id, log_res)
        });

        let detail = PrChecksDetail { pr_number, head_sha: head_sha.to_owned(), runs, jobs, failed_log };
        self.inner.github.pr_cache.lock().insert(key, (Instant::now(), detail.clone()));
        Ok(detail)
    }

    /// Loads (in the background) the jobs of an Actions run (and the log of the first failed job).
    pub fn github_load_run_jobs(&self, run_id: u64, force: bool) {
        let project = self.active_project_id();
        let ws = self.workspace();
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new().name("cd-github-run".into()).spawn(move || {
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            let (result, failed_log) = match core.github_repo_at(&ws) {
                Ok(repo) => {
                    let token = core.github_token(&ws, false);
                    let jobs_res = core.fetch_run_jobs_cached(&repo, run_id, token.as_ref(), force);
                    let failed = jobs_res.as_ref().ok().and_then(|jobs| {
                        let job = jobs.iter().find(|j| j.status == CheckStatus::Failure)?;
                        Some((job.id, core.fetch_job_log_cached(&repo, job.id, token.as_ref(), force)))
                    });
                    (jobs_res, failed)
                }
                Err(e) => (Err(e), None),
            };
            core.emit(CoreEvent::Github(GithubEvent { project, payload: GithubPayload::RunJobs { run_id, result, failed_log } }));
        });
    }

    /// Loads (in the background) the end of the log of a specific job.
    pub fn github_load_job_log(&self, job_id: u64, force: bool) {
        let project = self.active_project_id();
        let ws = self.workspace();
        let weak = Arc::downgrade(&self.inner);
        let _ = std::thread::Builder::new().name("cd-github-log".into()).spawn(move || {
            let Some(inner) = weak.upgrade() else { return };
            let core = Core { inner };
            let result = match core.github_repo_at(&ws) {
                Ok(repo) => {
                    let token = core.github_token(&ws, false);
                    core.fetch_job_log_cached(&repo, job_id, token.as_ref(), force)
                }
                Err(e) => Err(e),
            };
            core.emit(CoreEvent::Github(GithubEvent { project, payload: GithubPayload::JobLog { job_id, result } }));
        });
    }

    fn fetch_run_jobs_cached(
        &self,
        repo: &RepoSlug,
        run_id: u64,
        token: Option<&SecretToken>,
        force: bool,
    ) -> Result<Vec<WorkflowJob>, GithubError> {
        let key = (repo.clone(), run_id);
        if !force {
            if let Some((at, cached)) = self.inner.github.run_jobs_cache.lock().get(&key) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(cached.clone());
                }
            }
        }
        let path = format!("/repos/{}/{}/actions/runs/{run_id}/jobs?per_page=50", repo.owner, repo.repo);
        let json = self.github_get_json(repo, &path, token)?;
        let jobs = parse_workflow_jobs(&json, run_id);
        self.inner.github.run_jobs_cache.lock().insert(key, (Instant::now(), jobs.clone()));
        Ok(jobs)
    }

    fn fetch_job_log_cached(&self, repo: &RepoSlug, job_id: u64, token: Option<&SecretToken>, force: bool) -> Result<JobLog, GithubError> {
        let key = (repo.clone(), job_id);
        if !force {
            if let Some((at, cached)) = self.inner.github.job_log_cache.lock().get(&key) {
                if at.elapsed() < CACHE_TTL {
                    return Ok(cached.clone());
                }
            }
        }
        let log = self.github_fetch_job_log(repo, job_id, token)?;
        self.inner.github.job_log_cache.lock().insert(key, (Instant::now(), log.clone()));
        Ok(log)
    }

    /// Opens a GitHub URL (`https://github.com/...`) in the system's default browser.
    pub fn open_github_url(&self, url: &str) -> anyhow::Result<()> {
        let u = url.trim();
        if !u.starts_with("https://github.com/")
            || u.chars().any(|c| c.is_control() || c.is_whitespace() || matches!(c, '"' | '<' | '>' | '^' | '`'))
        {
            anyhow::bail!(t!("URL refused (only https://github.com/ is allowed): {url}", url = u));
        }
        open::that_detached(u).map_err(|e| anyhow::anyhow!(t!("failed to open the browser: {error}", error = e)))
    }
}

/// Reads a stream keeping only the last `keep_bytes` (up to `max_stream` bytes read in total).
fn read_stream_tail<R: Read>(mut reader: R, keep_bytes: usize, max_stream: usize) -> std::io::Result<Vec<u8>> {
    let mut ring = Vec::with_capacity(keep_bytes.min(64 * 1024));
    let mut chunk = [0u8; 8192];
    let mut total = 0usize;
    loop {
        let n = reader.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        total = total.saturating_add(n);
        if n >= keep_bytes {
            ring.clear();
            ring.extend_from_slice(&chunk[n - keep_bytes..n]);
        } else {
            let overflow = (ring.len() + n).saturating_sub(keep_bytes);
            if overflow > 0 {
                ring.drain(..overflow);
            }
            ring.extend_from_slice(&chunk[..n]);
        }
        if total >= max_stream {
            break;
        }
    }
    Ok(ring)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_github_remote_urls() {
        let expected = Some(RepoSlug::new("owner", "repo"));
        assert_eq!(parse_github_remote("https://github.com/owner/repo"), expected);
        assert_eq!(parse_github_remote("https://github.com/owner/repo.git"), expected);
        assert_eq!(parse_github_remote("https://github.com/owner/repo.git/"), expected);
        assert_eq!(parse_github_remote("https://user:token@github.com/owner/repo.git\n"), expected);
        assert_eq!(parse_github_remote("http://www.github.com/owner/repo"), expected);
        assert_eq!(parse_github_remote("git@github.com:owner/repo.git"), expected);
        assert_eq!(parse_github_remote("git@github.com:owner/repo"), expected);
        assert_eq!(parse_github_remote("github.com:owner/repo.git"), expected);
        assert_eq!(parse_github_remote("ssh://git@github.com/owner/repo.git"), expected);
        assert_eq!(parse_github_remote("ssh://git@github.com:22/owner/repo.git"), expected);
        assert_eq!(parse_github_remote("git://github.com/owner/repo.git"), expected);
        assert_eq!(parse_github_remote("https://github.com/org-1/some.repo_2.git"), Some(RepoSlug::new("org-1", "some.repo_2")));

        // Not from GitHub, or malformed.
        assert_eq!(parse_github_remote(""), None);
        assert_eq!(parse_github_remote("https://gitlab.com/owner/repo.git"), None);
        assert_eq!(parse_github_remote("git@gitlab.com:owner/repo.git"), None);
        assert_eq!(parse_github_remote("https://github.com/owner"), None);
        assert_eq!(parse_github_remote("https://github.com/owner/repo/extra"), None);
        assert_eq!(parse_github_remote("C:\\repos\\local"), None);
        assert_eq!(parse_github_remote("/var/git/repo.git"), None);
    }

    #[test]
    fn a_remote_that_is_not_github_never_shows_its_credentials() {
        assert_eq!(redact_remote_url("https://oauth2:glpat-XYZ@gitlab.com/org/repo.git"), "https://***@gitlab.com/org/repo.git");
        assert_eq!(redact_remote_url("https://ghp_ABC@github.com/org"), "https://***@github.com/org");
        assert_eq!(redact_remote_url("https://gitlab.com/org/repo.git"), "https://gitlab.com/org/repo.git");
        assert_eq!(redact_remote_url("git@gitlab.com:org/repo.git"), "git@gitlab.com:org/repo.git");
        // `@` in the path is not a credential.
        assert_eq!(redact_remote_url("https://host/org/a@b.git"), "https://host/org/a@b.git");
        let shown = GithubError::NotGithubRemote { url: redact_remote_url("https://u:secret@gitlab.com/o/r") }.to_string();
        assert!(!shown.contains("secret"), "{shown}");
    }

    #[test]
    fn exhausting_the_anonymous_quota_does_not_block_the_token() {
        let tmp = tempfile::tempdir().expect("tmp");
        let root = tmp.path();
        let paths = crate::paths::AppPaths {
            app_dir: root.to_path_buf(),
            bin_dir: root.join("bin"),
            data_dir: root.join("data"),
            default_workspace: root.join("ws"),
            whisper_dir: root.join("whisper"),
            config_file: root.join("cfg").join("config.json"),
        };
        let opts = crate::StartOptions {
            search_path: Some(Default::default()),
            home_dir: Some(root.join("home")),
            watch_workspace: false,
            config: Some(crate::Config::with_project(paths.default_workspace.clone())),
            system_dirs: false,
        };
        let core = Core::start_with(paths, opts).expect("core").0;
        let mut headers = ureq::http::HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().unwrap());
        headers.insert("x-ratelimit-reset", (now_epoch_s() + 3600).to_string().parse().unwrap());
        core.record_rate_limit(&headers, false);
        assert!(matches!(core.check_rate_limit(false), Err(GithubError::RateLimited { authenticated: false, .. })));
        assert!(core.check_rate_limit(true).is_ok(), "the signed-in quota is a different one");
        core.shutdown();
    }

    #[test]
    fn parses_credential_fill_output() {
        let out = "protocol=https\r\nhost=github.com\r\nusername=pattr\r\npassword=gho_example123456\r\n\r\n";
        assert_eq!(parse_credential_fill(out), Some("gho_example123456".into()));

        let empty_pw = "protocol=https\nhost=github.com\npassword=\n";
        assert_eq!(parse_credential_fill(empty_pw), None);
        assert_eq!(parse_credential_fill(""), None);

        // The Debug of SecretToken never reveals the content.
        let secret = SecretToken("gho_super_secret".into());
        assert_eq!(format!("{secret:?}"), "SecretToken(***)");
    }

    #[test]
    fn parses_pull_requests_and_attaches_checks() {
        let sample = json!([
            {
                "number": 42,
                "title": "feat(github): PRs, issues and Actions tab",
                "draft": false,
                "html_url": "https://github.com/owner/repo/pull/42",
                "updated_at": "2026-10-16T03:00:00Z",
                "user": { "login": "ana" },
                "head": { "ref": "feat/github", "sha": "abc1234567890" },
                "base": { "ref": "dev" }
            },
            {
                "number": 43,
                "title": "wip: experiment",
                "draft": true,
                "html_url": "https://github.com/owner/repo/pull/43",
                "updated_at": "2026-10-16T02:00:00Z",
                "user": { "login": "bia" },
                "head": { "ref": "wip/test", "sha": "def9876543210" },
                "base": { "ref": "main" }
            }
        ]);
        let mut prs = parse_pulls(&sample);
        assert_eq!(prs.len(), 2);
        assert_eq!(prs[0].number, 42);
        assert_eq!(prs[0].author, "ana");
        assert_eq!(prs[0].head_branch, "feat/github");
        assert_eq!(prs[0].base_branch, "dev");
        assert!(!prs[0].draft);
        assert!(prs[1].draft);

        let runs_json = json!({
            "workflow_runs": [
                {
                    "id": 1001,
                    "name": "CI",
                    "display_title": "feat(github): PR tab",
                    "head_branch": "feat/github",
                    "head_sha": "abc1234567890",
                    "event": "pull_request",
                    "status": "completed",
                    "conclusion": "failure",
                    "html_url": "https://github.com/owner/repo/actions/runs/1001",
                    "created_at": "2026-10-16T03:01:00Z"
                }
            ]
        });
        let runs = parse_workflow_runs(&runs_json);
        attach_pr_checks(&mut prs, &runs);
        assert_eq!(prs[0].checks, CheckStatus::Failure);
        assert_eq!(prs[1].checks, CheckStatus::Neutral);
    }

    #[test]
    fn parses_issues_excluding_pull_requests() {
        let sample = json!([
            {
                "number": 10,
                "title": "Bug when resizing the panel",
                "user": { "login": "caio" },
                "labels": [{ "name": "bug" }, { "name": "ui" }],
                "comments": 3,
                "html_url": "https://github.com/owner/repo/issues/10",
                "updated_at": "2026-10-15T20:00:00Z",
                "body": "When I drag the border\r\nthe panel flickers."
            },
            {
                "number": 42,
                "title": "PR that shows up in the issues endpoint",
                "user": { "login": "ana" },
                "labels": [],
                "comments": 0,
                "html_url": "https://github.com/owner/repo/pull/42",
                "updated_at": "2026-10-16T03:00:00Z",
                "pull_request": { "url": "https://api.github.com/repos/owner/repo/pulls/42" }
            }
        ]);
        let issues = parse_issues(&sample);
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].number, 10);
        assert_eq!(issues[0].labels, vec!["bug", "ui"]);
        assert_eq!(issues[0].comments, 3);
        assert_eq!(issues[0].body_preview, "When I drag the border the panel flickers.");
    }

    #[test]
    fn parses_jobs_and_failed_steps() {
        let sample = json!({
            "jobs": [
                {
                    "id": 501,
                    "run_id": 1001,
                    "name": "clippy & test (windows-latest)",
                    "status": "completed",
                    "conclusion": "failure",
                    "html_url": "https://github.com/owner/repo/actions/runs/1001/job/501",
                    "steps": [
                        { "name": "Checkout", "status": "completed", "conclusion": "success" },
                        { "name": "cargo clippy", "status": "completed", "conclusion": "failure" }
                    ]
                },
                {
                    "id": 502,
                    "run_id": 1001,
                    "name": "fmt",
                    "status": "completed",
                    "conclusion": "success",
                    "html_url": "https://github.com/owner/repo/actions/runs/1001/job/502",
                    "steps": []
                }
            ]
        });
        let jobs = parse_workflow_jobs(&sample, 1001);
        assert_eq!(jobs.len(), 2);
        assert_eq!(jobs[0].status, CheckStatus::Failure);
        assert_eq!(jobs[0].failed_steps, vec!["cargo clippy"]);
        assert_eq!(jobs[1].status, CheckStatus::Success);
        assert!(jobs[1].failed_steps.is_empty());
    }

    #[test]
    fn tails_and_cleans_job_logs() {
        let raw = "\u{feff}2026-10-16T03:10:00.1234567Z Compiling agentdeck v0.3.0\r\n\
                   2026-10-16T03:10:01.0000000Z \x1b[31merror[E0308]\x1b[0m: mismatched types\r\n\
                   2026-10-16T03:10:02.0000000Z   --> crates/core/src/github.rs:10:5\r\n\
                   2026-10-16T03:10:03.0000000Z ##[error]Process completed with exit code 101.\r\n";
        let log = tail_log(raw, 2, 4096);
        assert!(log.truncated);
        assert_eq!(log.lines, vec!["  --> crates/core/src/github.rs:10:5", "##[error]Process completed with exit code 101."]);
    }

    #[test]
    fn formats_agent_prompts_cleanly() {
        let repo = RepoSlug::new("owner", "agentdeck");
        let pr = PullRequest {
            number: 12,
            title: "GitHub integration".into(),
            author: "ana".into(),
            head_branch: "feat/github".into(),
            base_branch: "dev".into(),
            head_sha: "61621aa998877".into(),
            draft: false,
            html_url: "https://github.com/owner/agentdeck/pull/12".into(),
            updated_at: String::new(),
            checks: CheckStatus::Failure,
        };
        let p_prompt = format_pr_prompt(&repo, &pr);
        assert!(p_prompt.contains("Review PR #12"));
        assert!(p_prompt.contains("owner/agentdeck"));
        assert!(p_prompt.contains("feat/github -> dev"));

        let job = WorkflowJob {
            id: 501,
            run_id: 1001,
            name: "clippy (windows)".into(),
            status: CheckStatus::Failure,
            conclusion: Some("failure".into()),
            html_url: "https://github.com/owner/agentdeck/actions/runs/1001/job/501".into(),
            failed_steps: vec!["cargo clippy".into()],
        };
        let log = JobLog { lines: vec!["error: unused variable `x`".into()], truncated: false };
        let j_prompt = format_failed_job_prompt(&repo, Some(&pr), None, &job, Some(&log));
        assert!(j_prompt.contains("Fix the job \"clippy (windows)\" that failed in PR #12"));
        assert!(j_prompt.contains("error: unused variable `x`"));
    }

    #[test]
    fn rerun_replaces_the_previous_attempt_of_the_same_workflow() {
        let runs_json = json!({
            "workflow_runs": [
                { "id": 3, "name": "CI", "head_sha": "aaa", "head_branch": "x", "status": "completed", "conclusion": "success" },
                { "id": 2, "name": "Lint", "head_sha": "aaa", "head_branch": "x", "status": "in_progress", "conclusion": null },
                { "id": 1, "name": "CI", "head_sha": "aaa", "head_branch": "x", "status": "completed", "conclusion": "failure" }
            ]
        });
        let runs = parse_workflow_runs(&runs_json);
        assert_eq!(runs.len(), 3);
        assert_eq!(runs[1].raw_status, "in_progress");
        assert_eq!(runs[1].conclusion, None);
        let latest = latest_per_workflow(&runs);
        assert_eq!(latest.iter().map(|r| r.id).collect::<Vec<_>>(), vec![3, 2]);

        let mut prs = vec![PullRequest {
            number: 1,
            title: "t".into(),
            author: "a".into(),
            head_branch: "x".into(),
            base_branch: "dev".into(),
            head_sha: "aaa".into(),
            draft: false,
            html_url: String::new(),
            updated_at: String::new(),
            checks: CheckStatus::Neutral,
        }];
        attach_pr_checks(&mut prs, &runs);
        // CI passed on the rerun and Lint is still running: the PR is in progress, not red.
        assert_eq!(prs[0].checks, CheckStatus::Pending);
        let detail = PrChecksDetail { runs, ..Default::default() };
        assert_eq!(detail.status(), CheckStatus::Pending);
    }

    #[test]
    fn maps_check_status_and_aggregates() {
        use CheckStatus::*;
        assert_eq!(map_check_status("queued", None), Pending);
        assert_eq!(map_check_status("completed", Some("success")), Success);
        assert_eq!(map_check_status("completed", Some("timed_out")), Failure);
        assert_eq!(map_check_status("completed", Some("skipped")), Neutral);
        assert_eq!(map_check_status("completed", None), Neutral);
        assert_eq!(aggregate_check_status([Success, Pending, Failure]), Failure);
        assert_eq!(aggregate_check_status([Success, Pending]), Pending);
        assert_eq!(aggregate_check_status([Success, Neutral]), Success);
        assert_eq!(aggregate_check_status([]), Neutral);

        let checks = json!({ "check_runs": [
            { "status": "completed", "conclusion": "success" },
            { "status": "completed", "conclusion": "failure" }
        ] });
        assert_eq!(parse_check_runs_status(&checks), Failure);
        assert_eq!(parse_check_runs_status(&json!({})), Neutral);
    }

    #[test]
    fn log_tail_respects_limits_and_utf8() {
        // "çãõ" is deliberate non-ASCII data: it checks that the byte cut never splits a character.
        let raw: String = (0..1000).map(|i| format!("line {i} çãõ\n")).collect();
        let log = tail_log(&raw, LOG_MAX_LINES, 2000);
        assert!(log.truncated);
        assert!(log.lines.len() < 200);
        assert_eq!(log.lines.last().map(String::as_str), Some("line 999 çãõ"));
        assert!(log.lines.iter().all(|l| l.starts_with("line ")), "does not start in the middle of a line");
        assert_eq!(tail_log("", 10, 10), JobLog::default());
        let small = tail_log("a\nb\n", 10, 1000);
        assert_eq!(small.lines, vec!["a", "b"]);
        assert!(!small.truncated);
    }

    #[test]
    fn stream_tail_keeps_only_the_end() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let tail = read_stream_tail(&data[..], 1000, 10_000_000).unwrap();
        assert_eq!(tail.len(), 1000);
        assert_eq!(tail, data[data.len() - 1000..]);
        let all = read_stream_tail(&b"short"[..], 1000, 10_000_000).unwrap();
        assert_eq!(all, b"short");
    }

    #[test]
    fn github_error_messages_are_clear() {
        let limited = GithubError::RateLimited { reset_in_secs: Some(125), authenticated: false }.to_string();
        assert!(limited.contains("3 min") && limited.contains("60/h"), "{limited}");
        let nf = GithubError::NotFound { repo: "owner/repo".into(), authenticated: false }.to_string();
        assert!(nf.contains("owner/repo") && nf.contains("private"), "{nf}");
        assert!(GithubError::Unauthorized.to_string().contains("401"));
        for e in [GithubError::Unauthorized, GithubError::Forbidden("x".into()), GithubError::Network("y".into())] {
            assert!(!e.to_string().to_lowercase().contains("bearer"));
        }
    }

    #[test]
    fn prompts_for_issue_and_failed_job_without_pr() {
        let repo = RepoSlug::new("owner", "repo");
        let issue = Issue {
            number: 7,
            title: "Crashes \u{1b}[31mwhen opening".into(),
            author: "x".into(),
            labels: vec!["bug".into()],
            comments: 0,
            html_url: "https://github.com/owner/repo/issues/7".into(),
            updated_at: String::new(),
            body_preview: "steps".into(),
        };
        let m = format_issue_prompt(&repo, &issue);
        assert!(m.contains("issue #7") && m.contains("Labels: bug") && m.contains("Summary: steps"));
        assert!(!m.contains('\u{1b}'), "no control characters");

        let job = WorkflowJob {
            id: 1,
            run_id: 2,
            name: "test".into(),
            status: CheckStatus::Failure,
            conclusion: Some("failure".into()),
            html_url: "https://github.com/owner/repo/actions/runs/2/job/1".into(),
            failed_steps: vec![],
        };
        let run = WorkflowRun {
            id: 2,
            name: "CI".into(),
            display_title: "Fixes X".into(),
            head_branch: "fix/x".into(),
            head_sha: String::new(),
            event: "push".into(),
            status: CheckStatus::Failure,
            raw_status: "completed".into(),
            conclusion: Some("failure".into()),
            html_url: String::new(),
            created_at: String::new(),
        };
        let m = format_failed_job_prompt(&repo, None, Some(&run), &job, None);
        assert!(m.contains("in the \"Fixes X\" run (branch fix/x)"), "{m}");
    }
}

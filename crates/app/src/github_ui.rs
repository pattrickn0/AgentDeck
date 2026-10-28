//! State of the "GitHub" tab in the right bar and bridge between [`Deck`] and [`agentdeck_core::github`].
//!
//! Every GitHub call (`git remote`, `git credential fill`, `gh auth token` and the REST API) runs
//! outside the interface thread and comes back through [`CoreEvent::Github`]; here live only the
//! screen state and the handling of those events. The painting is in `ui/github.rs`.

use std::collections::HashMap;
use std::time::Instant;

use agentdeck_core::github::{
    CheckStatus, GithubError, GithubEvent, GithubOverview, GithubPayload, Issue, JobLog, PrChecksDetail, PullRequest, RateLimitInfo,
    RepoSlug, WorkflowJob, WorkflowRun,
};
use agentdeck_core::t;
use agentdeck_core::Ident;
use egui::Rect;

use crate::deck::{Deck, RightView};

/// Active sub-tab inside the GitHub tab.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GithubTab {
    #[default]
    Prs,
    Issues,
    Actions,
}

/// What the tab shows for the active project.
#[derive(Debug, Clone, Default)]
pub enum GithubView {
    #[default]
    Loading,
    /// No project open in AgentDeck.
    NoProject,
    /// Failure with no previous data on screen (no GitHub remote, network, refused login, limit...).
    Error(GithubError),
    /// Data loaded from the repository.
    Ready(GithubOverview),
}

/// "Send to agent" menu: what will be sent and where the menu anchors.
#[derive(Debug, Clone)]
pub struct AgentMenuTarget {
    /// Rectangle of the button that opened the menu.
    pub anchor: Rect,
    /// Short label of what will be sent (e.g. `PR #42`).
    pub title: String,
    /// Ready-made message.
    pub prompt: String,
}

/// State of the GitHub tab in the [`Deck`].
#[derive(Default)]
pub struct GithubUi {
    pub tab: GithubTab,
    pub view: GithubView,
    /// Last known repository (for the title when the next read fails).
    pub repo: Option<RepoSlug>,
    /// A general read is in progress.
    pub refreshing: bool,
    /// The read in progress is forced (Refresh button): it also redoes the open detail.
    pub forced: bool,
    /// Recent notice or error at the top of the tab `(text, is_error)`.
    pub notice: Option<(String, bool)>,
    /// When the last read finished.
    pub last_refresh: Option<Instant>,

    pub selected_pr: Option<u64>,
    /// Checks and jobs per PR number.
    pub pr_details: HashMap<u64, PrChecksDetail>,
    pub pr_loading: Option<u64>,

    pub selected_issue: Option<u64>,

    pub selected_run: Option<u64>,
    /// Jobs per run.
    pub run_jobs: HashMap<u64, Vec<WorkflowJob>>,
    pub run_loading: Option<u64>,

    /// End of the log per job (or the reason it did not arrive).
    pub job_logs: HashMap<u64, Result<JobLog, GithubError>>,
    pub log_loading: Option<u64>,
    /// Job whose log is open `(job_id, name, originating PR)`.
    pub open_log: Option<(u64, String, Option<u64>)>,

    pub agent_menu: Option<AgentMenuTarget>,
    /// Model chosen to open/use an instance with a role.
    pub new_role_kind: String,
    /// Role typed by the user (e.g. `review`).
    pub new_role_name: String,

    /// DEBUG ONLY (`AGENTDECK_DEBUG_UI=github*`): ignores real events and reads.
    pub debug_hold: bool,
}

impl GithubUi {
    /// Loaded data, if any.
    pub fn overview(&self) -> Option<&GithubOverview> {
        match &self.view {
            GithubView::Ready(ov) => Some(ov),
            _ => None,
        }
    }

    /// Open PRs (tab badge in the right bar).
    pub fn open_prs_count(&self) -> u32 {
        self.overview().map(|o| o.prs.len() as u32).unwrap_or(0)
    }

    /// Tooltip of the tab icon.
    pub fn tab_tooltip(&self) -> String {
        match &self.view {
            GithubView::Ready(ov) => {
                let (prs, issues) = (ov.prs.len(), ov.issues.len());
                let prs_txt = if prs == 1 { t!("1 PR").to_owned() } else { t!("{count} PRs", count = prs) };
                let issues_txt = if issues == 1 { t!("1 issue").to_owned() } else { t!("{count} issues", count = issues) };
                t!("GitHub · {repo} ({prs}, {issues})", repo = ov.repo, prs = prs_txt, issues = issues_txt)
            }
            _ => match &self.repo {
                Some(r) => format!("GitHub · {r}"),
                None => t!("GitHub (PRs, issues and Actions)").to_string(),
            },
        }
    }
}

/// Role typed by the user becomes a valid instance word (lowercase, digits, `-`, `_`).
pub fn normalize_role(text: &str) -> String {
    text.trim()
        .to_lowercase()
        .chars()
        .map(|c| if c.is_whitespace() { '-' } else { c })
        .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || matches!(c, '-' | '_'))
        .take(32)
        .collect()
}

/// `model` or `model@role` from what the menu has.
pub fn instance_spec(kind: &str, role: &str) -> String {
    let role = normalize_role(role);
    if role.is_empty() {
        kind.to_owned()
    } else {
        format!("{kind}@{role}")
    }
}

impl Deck {
    /// Clears the state when switching projects; with the tab open, it already reads the new one.
    pub fn github_reset_for_project(&mut self) {
        if self.github.debug_hold {
            return;
        }
        let tab = self.github.tab;
        self.github = GithubUi {
            tab,
            view: if self.active_project.is_empty() { GithubView::NoProject } else { GithubView::Loading },
            ..Default::default()
        };
        if self.right_view == RightView::GitHub {
            self.github_refresh();
        }
    }

    /// Reads the active project's data in the background (uses the core's 60 s cache).
    pub fn github_refresh(&mut self) {
        self.github_load(false);
    }

    /// Refresh button: ignores the caches and redoes the open detail too.
    pub fn github_refresh_now(&mut self) {
        self.github_load(true);
    }

    fn github_load(&mut self, force: bool) {
        if self.github.debug_hold {
            return;
        }
        if self.active_project.is_empty() || self.workspace.as_os_str().is_empty() {
            self.github.view = GithubView::NoProject;
            self.github.refreshing = false;
            return;
        }
        if self.github.refreshing {
            self.github.forced |= force;
            return;
        }
        if force {
            self.github.pr_details.clear();
            self.github.run_jobs.clear();
            self.github.job_logs.clear();
        }
        self.github.refreshing = true;
        self.github.forced = force;
        self.github.notice = None;
        self.core.github_refresh(force);
    }

    /// Selects (or deselects) a PR and loads its checks if they are not there yet.
    pub fn github_select_pr(&mut self, number: u64) {
        if self.github.selected_pr == Some(number) {
            self.github.selected_pr = None;
            return;
        }
        self.github.selected_pr = Some(number);
        self.github.open_log = None;
        if self.github.debug_hold {
            return;
        }
        self.github_load_pr_detail(number, false);
    }

    fn github_load_pr_detail(&mut self, number: u64, force: bool) {
        if self.github.pr_loading == Some(number) {
            return;
        }
        if let Some(detail) = self.github.pr_details.get(&number) {
            let jobs = detail.jobs.clone();
            self.auto_open_failed_job_log(Some(number), &jobs);
            return;
        }
        let Some(sha) = self.github.overview().and_then(|ov| ov.prs.iter().find(|p| p.number == number)).map(|p| p.head_sha.clone()) else {
            return;
        };
        self.github.pr_loading = Some(number);
        self.core.github_load_pr(number, sha, force);
    }

    /// Selects (or deselects) an Actions run and loads its jobs.
    pub fn github_select_run(&mut self, run_id: u64) {
        if self.github.selected_run == Some(run_id) {
            self.github.selected_run = None;
            return;
        }
        self.github.selected_run = Some(run_id);
        self.github.open_log = None;
        if self.github.debug_hold {
            return;
        }
        self.github_load_run_detail(run_id, false);
    }

    fn github_load_run_detail(&mut self, run_id: u64, force: bool) {
        if self.github.run_loading == Some(run_id) {
            return;
        }
        if let Some(jobs) = self.github.run_jobs.get(&run_id) {
            let jobs = jobs.clone();
            self.auto_open_failed_job_log(None, &jobs);
            return;
        }
        self.github.run_loading = Some(run_id);
        self.core.github_load_run_jobs(run_id, force);
    }

    /// Opens the panel with the end of a job's log and downloads it if it is not there yet.
    pub fn github_request_job_log(&mut self, job_id: u64, job_name: String, pr_number: Option<u64>) {
        self.github.open_log = Some((job_id, job_name, pr_number));
        if self.github.debug_hold || self.github.job_logs.contains_key(&job_id) {
            return;
        }
        self.github.log_loading = Some(job_id);
        self.core.github_load_job_log(job_id, false);
    }

    /// With red jobs in the list, opens the log of the first one (if none of them is already open).
    fn auto_open_failed_job_log(&mut self, pr_number: Option<u64>, jobs: &[WorkflowJob]) {
        let Some(failed) = jobs.iter().find(|j| j.status == CheckStatus::Failure) else { return };
        let keep_current =
            self.github.open_log.as_ref().is_some_and(|(id, _, _)| jobs.iter().any(|j| j.id == *id && j.status == CheckStatus::Failure));
        if !keep_current {
            self.github_request_job_log(failed.id, failed.name.clone(), pr_number);
        }
    }

    /// Handles a core event. Events from another project or from an old read are ignored.
    pub fn on_github_event(&mut self, ev: GithubEvent) {
        if self.github.debug_hold || ev.project != self.active_project {
            return;
        }
        match ev.payload {
            GithubPayload::Overview(Ok(overview)) => {
                self.github.refreshing = false;
                self.github.last_refresh = Some(Instant::now());
                self.github.notice = None;
                self.github.repo = Some(overview.repo.clone());
                let forced = std::mem::take(&mut self.github.forced);
                self.github.view = GithubView::Ready(overview);
                // Forced: reloads what was open (the screen caches were cleared).
                if let Some(num) = self.github.selected_pr {
                    self.github_load_pr_detail(num, forced);
                }
                if let Some(id) = self.github.selected_run {
                    self.github_load_run_detail(id, forced);
                }
            }
            GithubPayload::Overview(Err(error)) => {
                self.github.refreshing = false;
                self.github.forced = false;
                if matches!(self.github.view, GithubView::Ready(_)) {
                    // Keeps what was already on screen and warns in the top strip.
                    self.github.notice = Some((error.to_string(), true));
                } else {
                    self.github.view = GithubView::Error(error);
                }
            }
            GithubPayload::PrDetail { pr_number, result } => {
                if self.github.pr_loading == Some(pr_number) {
                    self.github.pr_loading = None;
                }
                match result {
                    Ok(mut detail) => {
                        let status = detail.status();
                        if let GithubView::Ready(ov) = &mut self.github.view {
                            if let Some(pr) = ov.prs.iter_mut().find(|p| p.number == pr_number) {
                                if !detail.runs.is_empty() {
                                    pr.checks = status;
                                }
                            }
                        }
                        let failed_log = detail.failed_log.take();
                        let jobs = detail.jobs.clone();
                        self.github.pr_details.insert(pr_number, detail);
                        self.store_preloaded_log(failed_log, &jobs);
                        if self.github.selected_pr == Some(pr_number) {
                            self.auto_open_failed_job_log(Some(pr_number), &jobs);
                        }
                    }
                    Err(e) => self.github.notice = Some((t!("PR #{number}: {error}", number = pr_number, error = e), true)),
                }
            }
            GithubPayload::RunJobs { run_id, result, failed_log } => {
                if self.github.run_loading == Some(run_id) {
                    self.github.run_loading = None;
                }
                match result {
                    Ok(jobs) => {
                        self.store_preloaded_log(failed_log, &jobs);
                        self.github.run_jobs.insert(run_id, jobs.clone());
                        if self.github.selected_run == Some(run_id) {
                            self.auto_open_failed_job_log(None, &jobs);
                        }
                    }
                    Err(e) => self.github.notice = Some((t!("Run #{id}: {error}", id = run_id, error = e), true)),
                }
            }
            GithubPayload::JobLog { job_id, result } => {
                if self.github.log_loading == Some(job_id) {
                    self.github.log_loading = None;
                }
                self.github.job_logs.insert(job_id, result);
            }
        }
    }

    /// Stores the log the core already downloaded together with the jobs.
    fn store_preloaded_log(&mut self, log: Option<(u64, Result<JobLog, GithubError>)>, jobs: &[WorkflowJob]) {
        if let Some((job_id, result)) = log {
            if jobs.iter().any(|j| j.id == job_id) {
                self.github.job_logs.insert(job_id, result);
            }
        }
    }

    /// Opens a GitHub URL in the default browser.
    pub fn open_github_browser(&mut self, url: &str) {
        if let Err(e) = self.core.open_github_url(url) {
            self.github.notice = Some((t!("Could not open the browser: {error}", error = e), true));
        }
    }

    /// Opens the "Send to agent" menu with the ready-made message.
    pub fn open_github_agent_menu(&mut self, anchor: Rect, title: String, suggested_role: &str, prompt: String) {
        if self.github.new_role_kind.is_empty() {
            self.github.new_role_kind =
                self.active_tab().map(|t| t.kind.clone()).filter(|k| k != "shell").unwrap_or_else(|| self.agents.fallback_kind());
        }
        self.github.new_role_name = suggested_role.to_owned();
        self.github.agent_menu = Some(AgentMenuTarget { anchor, title, prompt });
    }

    /// Sends the ready-made message to instance `spec` (`claude`, `claude@review`...), opening the
    /// tab if it does not exist. Closes the menu and reports the result in the top strip.
    pub fn send_github_prompt_to_agent(&mut self, spec: &str, prompt: String) {
        self.github.agent_menu = None;
        let Some(want) = Ident::parse(spec) else {
            self.github.notice = Some((t!("Invalid agent name: \"{spec}\".", spec = spec), true));
            return;
        };
        self.github.notice = Some(match self.send_to_instance(&want, false, prompt) {
            Ok(label) => (t!("Sent to {agent}.", agent = label), false),
            Err(msg) => (msg, true),
        });
    }
}

// ---------------------------------------------------------------------------
// Fake data for screenshots (`AGENTDECK_DEBUG_UI=github-demo`)
// ---------------------------------------------------------------------------

const DEMO_SHA_42: &str = "61621aa9c4f01b2e3d4a5f60718293a4b5c6d7e8";

pub fn demo_overview() -> GithubOverview {
    let pr = |number, title: &str, author: &str, head: &str, sha: &str, draft, checks| PullRequest {
        number,
        title: title.into(),
        author: author.into(),
        head_branch: head.into(),
        base_branch: "dev".into(),
        head_sha: sha.into(),
        draft,
        html_url: format!("https://github.com/pattr/agentdeck/pull/{number}"),
        updated_at: "2026-10-16T03:30:00Z".into(),
        checks,
    };
    let issue = |number, title: &str, author: &str, labels: &[&str], comments, body: &str| Issue {
        number,
        title: title.into(),
        author: author.into(),
        labels: labels.iter().map(|l| (*l).to_owned()).collect(),
        comments,
        html_url: format!("https://github.com/pattr/agentdeck/issues/{number}"),
        updated_at: "2026-10-15T21:10:00Z".into(),
        body_preview: body.into(),
    };
    let run = |id, title: &str, branch: &str, sha: &str, status, conclusion: Option<&str>| WorkflowRun {
        id,
        name: "CI".into(),
        display_title: title.into(),
        head_branch: branch.into(),
        head_sha: sha.into(),
        event: "pull_request".into(),
        status,
        raw_status: if conclusion.is_some() { "completed".into() } else { "in_progress".into() },
        conclusion: conclusion.map(str::to_owned),
        html_url: format!("https://github.com/pattr/agentdeck/actions/runs/{id}"),
        created_at: "2026-10-16T03:28:00Z".into(),
    };
    GithubOverview {
        repo: RepoSlug::new("pattr", "agentdeck"),
        authenticated: true,
        prs: vec![
            pr(42, "feat(github): pull requests, issues and Actions", "pattr", "feat/github", DEMO_SHA_42, false, CheckStatus::Failure),
            pr(
                39,
                "feat(git): commit history and branch selector",
                "ana-dev",
                "feat/git-panel",
                "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890",
                false,
                CheckStatus::Success,
            ),
            pr(
                44,
                "wip(terminal): incremental scrollback search",
                "carlos",
                "feat/term-search",
                "11223344556677889900aabbccddeeff00112233",
                true,
                CheckStatus::Pending,
            ),
            pr(45, "docs: contributing guide", "bia", "docs/contrib", "", false, CheckStatus::Neutral),
        ],
        issues: vec![
            issue(
                38,
                "Shortcut to switch between the Git and GitHub tabs",
                "pattr",
                &["enhancement", "ui"],
                3,
                "It would be useful to have dedicated keyboard shortcuts to open the right bar tabs quickly, without using the mouse.",
            ),
            issue(
                31,
                "ConPTY on Windows 11 24H2: cursor blinks on resize",
                "marcos",
                &["bug", "windows"],
                5,
                "When I drag the right bar divider quickly, ConPTY emits extra DECSCUSR sequences and the cursor blinks.",
            ),
        ],
        runs: vec![
            run(9001, "feat(github): pull requests, issues and Actions", "feat/github", DEMO_SHA_42, CheckStatus::Failure, Some("failure")),
            run(
                9000,
                "feat(git): commit history and branch selector",
                "feat/git-panel",
                "89ab12cd34ef56ab78cd90ef12ab34cd56ef7890",
                CheckStatus::Success,
                Some("success"),
            ),
            run(
                8999,
                "wip(terminal): incremental scrollback search",
                "feat/term-search",
                "11223344556677889900aabbccddeeff00112233",
                CheckStatus::Pending,
                None,
            ),
        ],
        rate_limit: Some(RateLimitInfo { remaining: Some(4892), limit: Some(5000), reset_at_epoch_s: None }),
    }
}

pub fn demo_pr_detail() -> PrChecksDetail {
    let job = |id, name: &str, status, conclusion: &str, steps: &[&str]| WorkflowJob {
        id,
        run_id: 9001,
        name: name.into(),
        status,
        conclusion: Some(conclusion.into()),
        html_url: format!("https://github.com/pattr/agentdeck/actions/runs/9001/job/{id}"),
        failed_steps: steps.iter().map(|s| (*s).to_owned()).collect(),
    };
    PrChecksDetail {
        pr_number: 42,
        head_sha: DEMO_SHA_42.into(),
        runs: vec![demo_overview().runs[0].clone()],
        jobs: vec![
            job(501, "clippy (windows-latest)", CheckStatus::Failure, "failure", &["cargo clippy --workspace -- -D warnings"]),
            job(502, "test (windows-latest)", CheckStatus::Success, "success", &[]),
            job(503, "fmt", CheckStatus::Success, "success", &[]),
        ],
        failed_log: None,
    }
}

pub fn demo_job_log() -> JobLog {
    JobLog {
        lines: [
            "Checking agentdeck-core v0.3.0 (D:\\a\\agentdeck\\crates\\core)",
            "Checking agentdeck v0.3.0 (D:\\a\\agentdeck\\crates\\app)",
            "error: redundant closure",
            "  --> crates/core/src/github.rs:312:44",
            "   |",
            "312|     let slug = url.and_then(|u| parse_github_remote(&u));",
            "   |                             ^^^^^^^^^^^^^^^^^^^^^^^^^^^ help: replace the closure with the function itself",
            "   = note: `-D clippy::redundant-closure` implied by `-D warnings`",
            "error: could not compile `agentdeck-core` (lib) due to 1 previous error",
            "##[error]Process completed with exit code 101.",
        ]
        .iter()
        .map(|l| (*l).to_owned())
        .collect(),
        truncated: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roles_become_valid_words() {
        assert_eq!(normalize_role("  Review "), "review");
        assert_eq!(normalize_role("code review!"), "code-review");
        assert_eq!(normalize_role("@ci"), "ci");
        assert_eq!(normalize_role("???"), "");
        assert_eq!(normalize_role(&"a".repeat(50)).len(), 32);
    }

    #[test]
    fn instance_specs_parse_as_idents() {
        assert_eq!(instance_spec("claude", ""), "claude");
        assert_eq!(instance_spec("claude", "Review"), "claude@review");
        let id = Ident::parse(&instance_spec("opencode", "ci")).expect("valid id");
        assert_eq!(id.kind, "opencode");
        assert_eq!(id.role.as_deref(), Some("ci"));
    }

    #[test]
    fn demo_data_is_consistent() {
        let ov = demo_overview();
        let detail = demo_pr_detail();
        assert!(ov.prs.iter().any(|p| p.number == detail.pr_number && p.head_sha == detail.head_sha));
        assert_eq!(detail.status(), CheckStatus::Failure);
        assert!(detail.jobs.iter().any(|j| j.status == CheckStatus::Failure));
        let ui = GithubUi { view: GithubView::Ready(ov), ..Default::default() };
        assert_eq!(ui.open_prs_count(), 4);
        assert!(ui.tab_tooltip().contains("pattr/agentdeck"));
    }
}

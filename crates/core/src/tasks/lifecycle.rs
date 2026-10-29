use std::path::PathBuf;

use super::git_ops::{
    branch_exists, delete_branch, diff_stat, git_out, merge_check, safe_worktree, short_hash, union_files, valid_task_branch,
    worktree_dirty, MergeCheck,
};
use super::text::{cap_text, short_list};
use super::{
    boot_line, branch_name, changed_files, clean_title, conflict_fix, git_error, memory_block, return_message, start_message, MergeStop,
    Task, TaskError, TaskSpec, TaskStatus, HOLT_DIR, MAX_OUTSIDE, MAX_TEXT,
};
use crate::capability;
use crate::events::{CoreEvent, Delivery, FeedEntry};
use crate::git::{run, Repo, COMMIT_TIMEOUT, WRITE_TIMEOUT};
use crate::instances::Ident;
use crate::t;
use crate::util::{self, now_ms};
use crate::{models, Core};

/// The new content of `info/exclude` with `/.holt/`, or `None` if it is already there.
pub fn exclude_with_holt(existing: &str) -> Option<String> {
    if existing.lines().map(str::trim).any(|l| matches!(l, "/.holt/" | "/.holt" | ".holt/" | ".holt")) {
        return None;
    }
    let mut text = existing.to_owned();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    text.push_str("# Holt: task worktrees\n/.holt/\n");
    Some(text)
}

impl Core {
    // ---------- Create ----------

    /// Creates a draft task. Checks the agent, the model and the repository (the current branch is the
    /// base, if none is requested). Runs git: call it off the interface thread.
    pub fn task_create(&self, project: &str, spec: TaskSpec) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let title = clean_title(&spec.title)?;
        let brief = cap_text(spec.brief.trim(), MAX_TEXT);
        let kinds = self.inner.agents.peers();

        // Agent: installed, with a tab (no `shell`) and without `@` (the role is the task id).
        let automatic_choice = spec.kind.trim().is_empty();
        let selected = automatic_choice.then(|| capability::select(&self.capability_candidates(&kinds, spec.effort), spec.level)).flatten();
        let asked = match spec.kind.trim().to_lowercase() {
            k if k.is_empty() => selected
                .as_ref()
                .map(|choice| choice.kind.clone())
                .or_else(|| kinds.iter().find(|kind| kind.as_str() == "claude").cloned())
                .or_else(|| kinds.first().cloned())
                .unwrap_or_else(|| "claude".to_owned()),
            k => k,
        };
        if asked.contains('@') {
            return Err(TaskError::Invalid(t!(
                "invalid agent \"{agent}\": give only the model (claude, antigravity…)",
                agent = asked.escape_debug()
            )));
        }
        let kind = crate::agents::resolve_alias(&asked).to_owned();
        if !kinds.contains(&kind) || Ident::parse(&kind).is_none() {
            return Err(TaskError::Invalid(t!(
                "unknown or not installed agent \"{agent}\". Available: {available}",
                agent = asked.escape_debug(),
                available = kinds.join(", ")
            )));
        }
        // Model: the requested one (validated) or the role's default; only for agents that let you choose.
        let requested = spec.model.as_deref().map(str::trim).filter(|m| !m.is_empty());
        if let Some(m) = requested {
            if !models::valid_model(m) {
                return Err(TaskError::Invalid(t!(
                    "invalid model: \"{model}\"",
                    model = m.chars().take(40).collect::<String>().escape_debug()
                )));
            }
            if !models::accepts_model(&kind) {
                return Err(TaskError::Invalid(t!("the agent {kind} does not accept choosing a model", kind = &kind)));
            }
        }
        let model = models::resolve_model(&Ident::main(&kind), requested, None, &self.config().role_models);
        // Effort: only where the CLI accepts it. In Antigravity it changes the model suffix, so the saved model
        // is already the one that will open.
        let (model, effort) = match spec.effort {
            Some(e) => match models::effort_unavailable(&kind, model.as_deref()) {
                Some(why) => return Err(TaskError::Invalid(t!("effort not applicable to {kind}: {why}", kind = &kind, why))),
                None => (model.map(|m| models::effort_model(&kind, &m, e)), Some(e)),
            },
            None => (model, None),
        };

        // Repository and base.
        let repo = self.task_repo(project)?;
        if repo.read(&["rev-parse", "--verify", "-q", "HEAD"]).is_err() {
            return Err(TaskError::Invalid(t!("the repository has no commits yet; make the first commit before creating tasks").into()));
        }
        let base = match spec.base.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
            Some(b) => b.to_owned(),
            None => match repo.read(&["symbolic-ref", "--short", "-q", "HEAD"]) {
                Ok(out) => String::from_utf8_lossy(&out.stdout).trim().to_owned(),
                Err(_) => return Err(TaskError::Invalid(t!("the repository has no branch (detached HEAD); give the task's base").into())),
            },
        };
        if !crate::git::valid_branch_name(&base) || repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{base}")]).is_err() {
            return Err(TaskError::Invalid(t!("the base \"{base}\" is not a local branch of this repository", base = base.escape_debug())));
        }

        let scope = crate::scope::normalize_all(&spec.scope).map_err(TaskError::Invalid)?;

        let number = self.with_tasks(project, false, |p| p.next.max(1));
        let id = format!("t{number}");
        let instance = Ident::parse(&format!("{kind}@{id}"))
            .map(|i| i.id())
            .ok_or_else(|| TaskError::Invalid(t!("agent \"{kind}\" cannot have a task instance", kind = &kind)))?;
        let mut task = Task {
            branch: branch_name(&id, &title),
            worktree: util::absolutize(&repo.root.join(HOLT_DIR).join("worktrees").join(&id)),
            id: id.clone(),
            title,
            brief,
            status: TaskStatus::Draft,
            scope,
            base,
            level: spec.level,
            kind,
            model,
            effort,
            instance,
            creator: spec.creator.map(|c| c.trim().to_owned()).filter(|c| !c.is_empty()),
            created_ms: now_ms(),
            ..Task::default()
        };
        task.log(match &task.creator {
            Some(c) => t!("created by {creator}", creator = c),
            None => t!("created").to_owned(),
        });
        if automatic_choice {
            if let Some(choice) = selected {
                task.log(t!("agent chosen automatically by capacity and quota: {reason}", reason = choice.reason));
                if let Some(warning) = choice.warning {
                    task.log(t!("capacity warning: {warning}", warning));
                }
            } else {
                task.log(t!("agent chosen automatically: {kind} (no quota reading)", kind = &task.kind));
            }
        } else {
            if let Some(note) = self.quota_advisory(&task.kind, &kinds) {
                task.log(t!("quota warning: {note}", note));
            }
            if let Some(note) = self.capability_advisory(&task.kind, task.model.as_deref(), task.level, task.effort) {
                task.log(t!("capacity warning: {warning}", warning = note));
            }
        }
        self.with_tasks(project, true, |p| {
            p.next = number + 1;
            p.tasks.push(task.clone());
        });
        Ok(task)
    }

    // ---------- Start ----------

    /// Starts the task: worktree + branch, the agent's instance with the terminal in the worktree and the brief
    /// delivered to the tab. Valid for `Draft` and for `Failed` without a report (retry). Only with the agents' tabs
    /// on (tasks do not run in headless mode). If the task has a scope and it overlaps that of a task in progress or
    /// in review (or of another that is already waiting), it goes to `Queued` instead of starting (the `Ok` carries
    /// the task already in the queue; one that failed to start does not enter the queue: it is refused with
    /// `Conflict`). It also accepts a `Queued` task, to check again. Runs git: off the interface thread.
    pub fn task_start(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let result = self.start_locked(project, id);
        if result.is_err() {
            // A task that failed to start no longer holds its scope.
            self.advance_queue_locked(project);
        }
        result
    }

    /// Starts the queue that can already start, in the project (see [`Core::advance_queue_locked`]). The interface
    /// calls it when opening the project: it covers what was left on the way if the app closed between a task
    /// leaving and the next one starting. Runs git: off the interface thread.
    pub fn tasks_advance_queue(&self, project: &str) {
        let _op = self.inner.tasks.op.lock();
        self.advance_queue_locked(project);
    }

    /// Goes through the project's queue in order of arrival and starts each task whose scope no longer overlaps that
    /// of anyone ahead of it; those still blocked only update the "waits for". The base is the one of now: the
    /// worktree comes out of the base tip at this moment, already with what the previous task merged. The caller
    /// already holds the operations lock. A failure (tab off, git) stays in the task's history and does not stop the
    /// following ones.
    pub(super) fn advance_queue_locked(&self, project: &str) {
        let mut queue: Vec<Task> = self.tasks(project).into_iter().filter(|t| t.status == TaskStatus::Queued).collect();
        queue.sort_by_key(queue_key);
        for task in queue {
            if let Err(e) = self.start_locked(project, &task.id) {
                log::warn!("queue: task {} did not start: {e}", task.id);
                self.update_task(project, &task.id, |t| {
                    if t.status == TaskStatus::Queued {
                        t.log(t!("queue: could not start: {error}", error = &e));
                    }
                });
            }
        }
    }

    fn start_locked(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let task = self.task_or_404(project, id)?;
        let retry = task.status == TaskStatus::Failed && task.report.is_none();
        if !matches!(task.status, TaskStatus::Draft | TaskStatus::Queued) && !retry {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a draft (or a task that failed to start) can be started",
                id = &task.id,
                status = task.status.label()
            )));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(
                t!("tasks run in the agents' tabs: turn on \"Open agent tabs\" in Settings (in headless mode there is no tab or worktree per agent)").into(),
            ));
        }
        if let Some(holder) = queue_holder(&self.tasks(project), &task) {
            if retry {
                return Err(TaskError::Conflict(t!(
                    "the scope of task {id} overlaps that of {holder}, which is in progress: try again when it finishes",
                    id = &task.id,
                    holder = &holder
                )));
            }
            let now = now_ms();
            return self
                .update_task(project, &task.id, |t| {
                    if t.waiting_for.as_deref() != Some(holder.as_str()) {
                        t.log(t!("queued: the scope overlaps that of {holder}; it starts when that one finishes", holder = &holder));
                    }
                    t.status = TaskStatus::Queued;
                    t.queued_ms.get_or_insert(now);
                    t.waiting_for = Some(holder);
                    t.clone()
                })
                .ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)));
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
            t.queued_ms = None;
            t.waiting_for = None;
            t.log(t!("started: worktree {worktree} on branch {branch}", worktree = t.worktree.display(), branch = &t.branch));
            t.clone()
        });
        let Some(task) = started else { return Err(TaskError::NotFound(t!("task {id} vanished", id))) };
        let ident = self.open_task_instance(project, &task);
        let boss = self.task_boss(project, &task);
        let prompt = start_message(&task, boss.as_deref(), &self.task_memory(project, &task));
        let boot = self.task_boot_prompt(project, &task, "brief", "the full request is in the file", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, t!("brief delivered to the agent"));
        self.feed(
            FeedEntry::system(t!("Task {id} started: {title} ({instance})", id = &task.id, title = &task.title, instance = &task.instance))
                .in_project(project),
        );
        Ok(task)
    }

    /// The memory block at the start of the task ([`memory_block`]): the learnings that apply to the request (read
    /// from the worktree, which already has the base) and the current situation. Any failure only leaves that part
    /// out: a task never fails to start because of memory.
    fn task_memory(&self, project: &str, task: &Task) -> String {
        // Agent-facing text: the situation shows task statuses (`TaskStatus::label`), which must read in English
        // whatever the language of the user who clicked "Start".
        crate::i18n::english(|| {
            let brief = if task.brief.trim().is_empty() { task.title.as_str() } else { task.brief.as_str() };
            let cited = crate::learnings::paths_in_text(brief, &task.worktree);
            let learnings = crate::learnings::render(&crate::learnings::load(&task.worktree), &cited, crate::learnings::START_BUDGET);
            memory_block(&learnings, &self.task_situation(project, task, &cited))
        })
    }

    /// The learnings specific to the files the new `feedback` cites and the agent has not seen yet (neither in the
    /// request nor in the earlier feedbacks of `before`), read from the worktree. The index and the global ones it
    /// already received at the start of the task. Empty if there is no new file or matching learning.
    pub(super) fn feedback_learnings(&self, before: &Task, feedback: &str) -> String {
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

    /// The "current situation" ([`crate::situation::render`]): the project's other tasks in progress, with the files
    /// their branch has already touched, and the latest commits of the base. Empty if there is no repository.
    fn task_situation(&self, project: &str, task: &Task, cited: &[String]) -> String {
        let Ok(repo) = self.task_repo(project) else { return String::new() };
        let others = self.busy_tasks(project, &repo, &task.instance);
        let commits = crate::situation::base_commits(&repo, &task.base);
        crate::situation::render(&others, &commits, &task.base, cited, crate::situation::BUDGET)
    }

    /// Worktree + branch of the task: excludes `/.holt/` from git, cleans up what a previous attempt left
    /// and creates the worktree from the base.
    fn prepare_worktree(&self, project: &str, task: &Task, retry: bool) -> Result<(), TaskError> {
        let repo = self.task_repo(project)?;
        self.ensure_exclude(&repo)?;
        if !safe_worktree(&task.worktree, &repo.root) || !valid_task_branch(&task.branch) {
            return Err(TaskError::Failed(t!(
                "data of task {id} outside what was expected (folder {folder} / branch {branch}); I touched nothing",
                id = &task.id,
                folder = task.worktree.display(),
                branch = &task.branch
            )));
        }
        if retry {
            let _ = self.remove_task_worktree(&repo, task);
            let _ = delete_branch(&repo, &task.branch, true);
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.base)]).is_err() {
            return Err(TaskError::Failed(t!("the base \"{base}\" no longer exists in this repository", base = &task.base)));
        }
        if repo.read(&["rev-parse", "--verify", "-q", &format!("refs/heads/{}", task.branch)]).is_ok() {
            return Err(TaskError::Failed(t!(
                "the branch {branch} already exists in this repository; remove it or discard the task",
                branch = &task.branch
            )));
        }
        if task.worktree.exists() {
            return Err(TaskError::Failed(t!(
                "the folder {folder} already exists; remove it or discard the task",
                folder = task.worktree.display()
            )));
        }
        let path = task.worktree.to_string_lossy().into_owned();
        run(repo.cmd(&["worktree", "add", "-b", &task.branch, &path, &task.base]), None, COMMIT_TIMEOUT)
            .map_err(|e| TaskError::Failed(t!("could not create the worktree: {error}", error = e)))?;
        Ok(())
    }

    /// Writes `/.holt/` into `<common git dir>/info/exclude`, once (never into `.gitignore`).
    fn ensure_exclude(&self, repo: &Repo) -> Result<(), TaskError> {
        let common = repo
            .read(&["rev-parse", "--path-format=absolute", "--git-common-dir"])
            .ok()
            .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or_else(|| repo.git_dir.clone());
        let file = common.join("info").join("exclude");
        let existing = std::fs::read_to_string(&file).unwrap_or_default();
        if let Some(text) = exclude_with_holt(&existing) {
            std::fs::create_dir_all(common.join("info"))
                .and_then(|_| std::fs::write(&file, text))
                .map_err(|e| TaskError::Failed(t!("could not write {file}: {error}", file = file.display(), error = e)))?;
        }
        Ok(())
    }

    /// Registers the task's instance (model, boss) and returns it; idempotent.
    pub(super) fn open_task_instance(&self, project: &str, task: &Task) -> Ident {
        let ident = Ident::parse(&task.instance).unwrap_or_else(|| Ident::main(&task.kind));
        self.with_instances(project, |list| list.register(ident.clone()));
        self.assign_model(project, &ident, task.model.as_deref());
        if let Some(boss) = self.task_boss(project, task) {
            if let Err(e) = self.set_boss(project, &task.instance, Some(&boss)) {
                log::warn!("boss of task {}: {e}", task.id);
            }
        }
        ident
    }

    /// Who created the task, when it is an agent with an open tab (the user is nobody's boss).
    fn task_boss(&self, project: &str, task: &Task) -> Option<String> {
        let creator = task.creator.as_deref()?;
        (creator != task.instance && self.project_instance_ids(project).iter().any(|o| o == creator)).then(|| creator.to_owned())
    }

    /// Initial prompt for opening the task's process: writes `prompt` to `data/tasks/<project>-<id>-<stem>.md`
    /// and returns the short line that tells the agent to read it. `None` if the CLI does not accept an initial
    /// prompt, if the file could not be written or if the line does not fit in ASCII: the delivery stays typed, as
    /// always.
    pub(super) fn task_boot_prompt(&self, project: &str, task: &Task, stem: &str, intro: &str, prompt: &str) -> Option<String> {
        models::boot_prompt_flag(Ident::parse(&task.instance)?.kind.as_str())?;
        let dir = self.paths().data_dir.join("tasks");
        let file = util::absolutize(&dir.join(format!("{project}-{}-{stem}.md", task.id)));
        let file = util::strip_verbatim(&file);
        match std::fs::create_dir_all(&dir).and_then(|_| std::fs::write(&file, prompt)) {
            Ok(()) => boot_line(task, intro, &file),
            Err(e) => {
                log::warn!("could not write {}: {e}", file.display());
                None
            }
        }
    }

    /// Stores the initial prompt for the next opening of `instance`'s process: `agents::spawn_spec_for` carries it
    /// on the command line (once). The interface calls it before opening or reopening the tab.
    pub fn set_boot_prompt(&self, project: &str, instance: &str, line: String) {
        self.inner.tasks.boot.lock().insert(format!("{project}/{instance}"), line);
    }

    /// A task's tab has to start again from scratch (its saved conversation could not be resumed): stores the initial prompt
    /// that sends the agent back to the brief written when the task started. `false` = not a task's tab, a CLI without
    /// an initial prompt, or no brief file: the tab opens empty.
    pub fn set_restart_boot_prompt(&self, project: &str, instance: &str) -> bool {
        let Some(task) = self.task_of_instance(project, instance) else { return false };
        if models::boot_prompt_flag(&task.kind).is_none() {
            return false;
        }
        let file = util::absolutize(&self.paths().data_dir.join("tasks").join(format!("{project}-{}-brief.md", task.id)));
        let file = util::strip_verbatim(&file);
        let intro = "your earlier session could not be resumed, so this is a fresh one; the full request is in the file";
        match file.is_file().then(|| boot_line(&task, intro, &file)).flatten() {
            Some(line) => {
                self.set_boot_prompt(project, instance, line);
                true
            }
            None => false,
        }
    }

    /// Takes (and forgets) the initial prompt stored for `instance`.
    pub fn take_boot_prompt(&self, project: &str, instance: &str) -> Option<String> {
        self.inner.tasks.boot.lock().remove(&format!("{project}/{instance}"))
    }

    /// Sends `prompt` to the task's tab (opens the tab if needed) through the path of messages between agents. If
    /// the tab has to be opened and the CLI accepts an initial prompt, the interface uses `boot` on the command line
    /// instead of typing `prompt` (see [`Delivery::boot`]).
    pub(super) fn deliver_to_task(&self, project: &str, task: &Task, ident: Ident, prompt: String, boot: Option<String>, log: &str) {
        let from = task.creator.clone().unwrap_or_else(|| crate::instances::USER.to_owned());
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

    // ---------- Report ----------

    /// The agent finished: stores the report and the `git diff --stat base...branch` and puts the task in
    /// review. Requires `Running`, or `Validating` with new commits on the branch (the user fixed something in the
    /// tab while waiting for validation and the agent delivers what is missing; without new commits, it refuses).
    /// It also refuses when the branch would not merge cleanly into the base (`git merge-tree`; skipped on a git without
    /// it): the message tells the agent to merge the base into its branch, resolve, check and deliver again. If the
    /// merge was already approved ([`Task::merge_approved`], a conflict found at merge time), the app merges by itself
    /// right after accepting the report. The bridge only lets the task's own agent call it.
    pub fn task_done(&self, project: &str, id: &str, report: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        let after_merge = task.status == TaskStatus::Validating;
        if task.status != TaskStatus::Running && !after_merge {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a task in progress receives the report",
                id = &task.id,
                status = task.status.label()
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
                return Err(TaskError::Conflict(t!(
                    "task {id} was already merged and awaits the user's validation; there are no new commits to deliver. If something is still missing, the user decides (Reject)",
                    id = &task.id
                )));
            }
        }
        let report = report.trim();
        if report.is_empty() {
            return Err(TaskError::Invalid(t!("the report is empty: say what was done, the files, the tests and what is missing").into()));
        }
        let report = cap_text(report, MAX_TEXT);
        if let Ok(repo) = self.task_repo(project) {
            if valid_task_branch(&task.branch) && branch_exists(&repo, &task.branch) {
                if let MergeCheck::Conflicts(files) = merge_check(&repo, &task.base, &task.branch) {
                    self.update_task(project, &task.id, |t| {
                        t.log(t!("delivery refused: the branch conflicts with {base}", base = &t.base))
                    });
                    return Err(TaskError::Conflict(format!(
                        "task {id} was not delivered: merging your branch ({branch}) into {base} would conflict, because {base} changed while you worked. {fix}",
                        id = task.id,
                        branch = task.branch,
                        base = task.base,
                        fix = conflict_fix(&task, &files),
                    )));
                }
            }
        }
        let (stat, warning, outside) = match self.task_repo(project) {
            Ok(repo) => {
                let dirty = worktree_dirty(&repo, &task.worktree).filter(|d| !d.is_empty());
                let outside = match task.scope.is_empty() {
                    true => Vec::new(),
                    false => {
                        crate::scope::outside(&task.scope, &union_files(changed_files(&repo, &task.base, &task.branch), dirty.clone()))
                    }
                };
                let warning = dirty.map(|d| {
                    t!(
                        "warning: there are uncommitted changes in the worktree ({files}); the agent must commit before the merge",
                        files = short_list(&d)
                    )
                });
                (diff_stat(&repo, &task.base, &task.branch), warning, outside)
            }
            Err(e) => (Err(e.message().to_owned()), None, Vec::new()),
        };
        let updated = self.update_task(project, &task.id, |t| {
            t.report = Some(report);
            match &stat {
                Ok(s) => t.diff_stat = Some(s.clone()),
                Err(e) => t.log(t!("could not get the diff: {error}", error = e)),
            }
            if let Some(w) = &warning {
                t.log(w.clone());
            }
            if !outside.is_empty() {
                t.log(t!("touched files outside the scope: {files}", files = short_list(&outside)));
            }
            t.outside_scope = outside.into_iter().take(MAX_OUTSIDE).collect();
            t.status = TaskStatus::Review;
            t.finished_ms = Some(now_ms());
            t.merge_stopped = None;
            t.log(if after_merge {
                t!("the agent delivered more work after the merge; in review")
            } else {
                t!("report received; in review")
            });
            t.clone()
        });
        let task = updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id)))?;
        self.feed(FeedEntry::system(t!("Task {id} ready for review: {title}", id = &task.id, title = &task.title)).in_project(project));
        if task.merge_approved {
            return Ok(self.auto_merge_locked(project, task));
        }
        Ok(task)
    }

    /// The agent delivered again after a conflict found at merge time and the merge was already approved: merges now,
    /// with no call from the reviewer. A refusal leaves the task in review with the reason in [`Task::merge_stopped`]
    /// (the card shows it); a new conflict asks the agent again, up to [`super::MAX_CONFLICT_ASKS`] times. Returns the
    /// task as it stands afterwards. Holds the operations lock (called by `task_done`).
    fn auto_merge_locked(&self, project: &str, task: Task) -> Task {
        self.update_task(project, &task.id, |t| t.log(t!("merge already approved: merging the new delivery")));
        match self.merge_locked(project, &task.id, false) {
            Ok(outcome) => outcome.task,
            Err(refusal) => {
                let stop = MergeStop {
                    reason: refusal.error.message().to_owned(),
                    conflict_files: match &refusal.block {
                        Some(super::MergeBlock::Conflict(files)) => Some(files.clone()),
                        _ => None,
                    },
                };
                self.feed(
                    FeedEntry::system(t!(
                        "Task {id} was not merged automatically, a person has to look: {reason}",
                        id = &task.id,
                        reason = &stop.reason
                    ))
                    .in_project(project),
                );
                self.update_task(project, &task.id, |t| {
                    t.merge_approved = false;
                    t.merge_stopped = Some(stop);
                    t.clone()
                })
                .unwrap_or(task)
            }
        }
    }

    /// Cancels the automatic merge of a task whose conflict was handed to the agent: the task stays with the agent
    /// (`Running`) and, when it delivers, waits in review like any other. `Conflict` if there is nothing to cancel.
    pub fn task_cancel_auto_merge(&self, project: &str, id: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        if !task.merge_approved {
            return Err(TaskError::Conflict(t!("task {id} has no automatic merge waiting", id = &task.id).into()));
        }
        let updated = self.update_task(project, &task.id, |t| {
            t.merge_approved = false;
            t.conflict_asks = 0;
            t.log(t!("automatic merge cancelled; it will wait for the review"));
            t.clone()
        });
        updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id = &task.id)))
    }

    // ---------- Diff ----------

    /// The task's full diff (`git diff base...branch`, only what the branch brought), in the same format as the Git
    /// tab's diff, for the interface to show. In `Validating` and `Merged` with `merge_commit`, it is what the merge
    /// brought (`git diff <hash>^1 <hash>`): after the alignment the branch is the base itself and the range diff
    /// would be empty. Read-only: it does not take the operations lock. Fails with `Conflict` if the branch no
    /// longer exists (discarded task) or if the merge commit vanished (rewritten history).
    pub fn task_diff(&self, project: &str, id: &str) -> Result<crate::git::GitDiff, TaskError> {
        let task = self.task_or_404(project, id)?;
        let repo = self.task_repo(project)?;
        if !valid_task_branch(&task.branch) || !crate::git::valid_branch_name(&task.base) {
            return Err(TaskError::Failed(t!("data of task {id} outside what was expected; I did not read the diff", id = &task.id)));
        }
        let merged = matches!(task.status, TaskStatus::Validating | TaskStatus::Merged);
        let args: Vec<String> = if let Some(hash) = task.merge_commit.as_deref().filter(|_| merged) {
            let well_formed = !hash.is_empty() && hash.len() <= 64 && hash.bytes().all(|b| b.is_ascii_hexdigit());
            if !well_formed || repo.read(&["rev-parse", "--verify", "-q", &format!("{hash}^{{commit}}")]).is_err() {
                return Err(TaskError::Conflict(t!(
                    "the merge commit ({hash}) no longer exists in the repository (was the history rewritten?): there is no diff to show",
                    hash = short_hash(hash)
                )));
            }
            let first_parent = format!("{hash}^1");
            ["diff", "--no-color", "--no-ext-diff", "-M", first_parent.as_str(), hash, "--"].map(str::to_owned).to_vec()
        } else {
            if !branch_exists(&repo, &task.branch) {
                return Err(TaskError::Conflict(t!(
                    "the branch {branch} no longer exists: there is no diff to show",
                    branch = &task.branch
                )));
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

    // ---------- Return ----------

    /// The review asked for changes: `Review` (or `Failed` with a report, from a merge that failed earlier) goes back
    /// to `Running` and the feedback is delivered to the agent (the tab is reopened if it was closed).
    pub fn task_return(&self, project: &str, id: &str, feedback: &str) -> Result<Task, TaskError> {
        let _op = self.inner.tasks.op.lock();
        let task = self.task_or_404(project, id)?;
        // `Failed` with a report is a merge that failed in an earlier version (today it stays in `Review`).
        if task.status != TaskStatus::Review && !(task.status == TaskStatus::Failed && task.report.is_some()) {
            return Err(TaskError::Conflict(t!(
                "task {id} is {status}: only a task in review can be returned",
                id = &task.id,
                status = task.status.label()
            )));
        }
        let feedback = feedback.trim();
        if feedback.is_empty() {
            return Err(TaskError::Invalid(t!("the feedback is empty: say what needs to change").into()));
        }
        if !self.tab_delivery() {
            return Err(TaskError::Conflict(t!("tasks run in the agents' tabs: turn on \"Open agent tabs\" in Settings").into()));
        }
        self.return_locked(project, &task, feedback, None)
    }

    /// Returns the task to its agent with `feedback` (the operations lock is held). `auto` = `Some(asks)` is the
    /// app's own request to resolve a conflict found at merge time: the merge stays approved and `asks` counts the
    /// requests so far. `None` is a review decision: any earlier approval is dropped.
    pub(super) fn return_locked(&self, project: &str, task: &Task, feedback: &str, auto: Option<u32>) -> Result<Task, TaskError> {
        let feedback = cap_text(feedback, MAX_TEXT);
        let updated = self.update_task(project, &task.id, |t| {
            t.status = TaskStatus::Running;
            t.finished_ms = None;
            t.error = None;
            t.merge_approved = auto.is_some();
            t.conflict_asks = auto.unwrap_or(0);
            t.merge_stopped = None;
            t.log(if auto.is_some() {
                t!("conflict with {base}; asked the agent to resolve it (the merge is already approved)", base = &t.base)
            } else {
                t!("returned to the agent with feedback").to_owned()
            });
            // `before` does not have the new feedback: the message shows only the earlier ones.
            let before = t.clone();
            t.add_feedback("review", &feedback);
            (before, t.clone())
        });
        let (before, task) = updated.ok_or_else(|| TaskError::NotFound(t!("task {id} vanished", id = &task.id)))?;
        let ident = self.open_task_instance(project, &task);
        let prompt = return_message(&before, &feedback, &self.feedback_learnings(&before, &feedback));
        let boot = self.task_boot_prompt(project, &task, "feedback", "was returned in review with changes, described in the file", &prompt);
        self.deliver_to_task(project, &task, ident, prompt, boot, t!("feedback delivered to the agent"));
        Ok(task)
    }
}

/// The number in the id (`t12` → 12); 0 if the id does not have that format.
fn task_number(id: &str) -> u64 {
    id.strip_prefix('t').and_then(|n| n.parse().ok()).unwrap_or(0)
}

/// The order in the queue: whoever entered first, and by number in case of a tie. Whoever is not in the queue goes to the end.
fn queue_key(task: &Task) -> (u64, u64) {
    (task.queued_ms.unwrap_or(u64::MAX), task_number(&task.id))
}

/// Which task `task` waits for, if it waits: one in progress or in review whose scope overlaps its own (the first
/// ones to be cited) or, if there is none, one that was already waiting in the queue before it and has an overlapping
/// scope. `Validating` does not hold: the code is already in the base. Without a reserved scope (see
/// [`crate::scope`]) nobody waits.
pub(super) fn queue_holder(tasks: &[Task], task: &Task) -> Option<String> {
    let clash = |o: &&Task| o.id != task.id && crate::scope::overlaps(&o.scope, &task.scope);
    let active = tasks.iter().filter(clash).find(|o| matches!(o.status, TaskStatus::Running | TaskStatus::Review));
    let ahead = || {
        tasks.iter().filter(clash).filter(|o| o.status == TaskStatus::Queued && queue_key(o) < queue_key(task)).min_by_key(|o| queue_key(o))
    };
    active.or_else(ahead).map(|o| o.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exclude_is_written_once() {
        let first = exclude_with_holt("# rules\n*.log").unwrap();
        assert_eq!(first, "# rules\n*.log\n# Holt: task worktrees\n/.holt/\n");
        assert_eq!(exclude_with_holt(&first), None);
        assert!(exclude_with_holt("").unwrap().ends_with("/.holt/\n"));
        assert_eq!(exclude_with_holt("a\n/.holt/\n"), None);
    }
}
